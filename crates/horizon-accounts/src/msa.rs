//! EN: Microsoft Account (MSA) OAuth: device-code sign-in and refresh-token
//! exchange, ported from `horizon-account-switcher.py` (`device_code_sign_in`,
//! `refresh_msa`). Endpoint base: `login.microsoftonline.com/consumers`,
//! scope `XboxLive.signin offline_access`.
//!
//! The library never prints: user-facing instructions (the "go to
//! microsoft.com/link and enter CODE" message) go through a `notify` callback.
//! Polling can be aborted with the `is_cancelled` closure.
//!
//! RU: OAuth для аккаунтов Microsoft: device-code вход и обмен refresh-токена
//! (порт `device_code_sign_in` / `refresh_msa` из horizon-account-switcher.py).
//! Библиотека ничего не печатает: инструкция для пользователя — через
//! callback `notify`, прерывание опроса — через `is_cancelled`.

use crate::error::AcctError;
use crate::http::{form_body, Http};
use crate::json::{parse, Json};

/// EN: consumer tenant (personal Microsoft accounts).
/// RU: тенант потребителей (личные аккаунты Microsoft).
pub const LIVE_BASE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
/// EN: the one scope the chain needs (Xbox Live sign-in + offline refresh).
/// RU: единственная нужная цепочке область (вход в Xbox Live + offline-refresh).
pub const SCOPE: &str = "XboxLive.signin offline_access";

/// EN: MSA token pair. RU: пара MSA-токенов.
#[derive(Debug, Clone, PartialEq)]
pub struct MsaTokens {
    /// EN: short-lived MSA access token (input for the XBL/XSTS chain).
    /// RU: короткоживущий access-токен MSA (вход для цепочки XBL/XSTS).
    pub access: String,
    /// EN: long-lived refresh token (stored on disk).
    /// RU: долгоживущий refresh-токен (хранится на диске).
    pub refresh: String,
}

/// EN: what the device-code endpoint told us. RU: ответ device-code endpoint.
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    /// EN: the code the user must enter. RU: код для ввода пользователем.
    pub user_code: String,
    /// EN: verification URL. RU: URL подтверждения.
    pub verification_uri: String,
    /// EN: full localized instruction message, if the endpoint sent one.
    /// RU: полная локализованная инструкция, если endpoint её прислал.
    pub message: Option<String>,
    /// EN: device_code blob for the token poll. RU: device_code для опроса token.
    pub device_code: String,
    /// EN: recommended poll interval, seconds. RU: рекомендуемый интервал опроса.
    pub interval: u64,
    /// EN: total lifetime, seconds. RU: общее время жизни, секунды.
    pub expires_in: u64,
}

/// EN: request a device code (step 1 of the flow; does not block).
/// RU: запросить device code (шаг 1; не блокирует).
pub fn request_device_code(http: &dyn Http, client_id: &str) -> Result<DeviceCodeInfo, AcctError> {
    let body = form_body(&[("client_id", client_id), ("scope", SCOPE)]);
    let resp = http.post_form(&format!("{LIVE_BASE}/devicecode"), &body)?;
    let j = parse(&resp).map_err(|e| AcctError::BadJson(e.to_string()))?;
    Ok(DeviceCodeInfo {
        user_code: field(&j, "user_code")?,
        verification_uri: field(&j, "verification_uri")?,
        message: j.get("message").and_then(Json::as_str).map(str::to_owned),
        device_code: field(&j, "device_code")?,
        interval: int_field(&j, "interval").unwrap_or(5),
        expires_in: int_field(&j, "expires_in").unwrap_or(900),
    })
}

/// EN: run the whole device-code sign-in: request the code, notify the user,
/// poll `/token` until it is granted, times out, or is cancelled.
///
/// * `notify` — receives the human instruction (never printed by the lib).
/// * `is_cancelled` — polled between requests; return `true` to abort
///   (checked before the first sleep too).
///
/// RU: полный device-code вход: запросить код, показать инструкцию через
/// `notify`, опрашивать `/token` до подтверждения, таймаута или отмены.
pub fn device_code(
    http: &dyn Http,
    client_id: &str,
    notify: &dyn Fn(&str),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<MsaTokens, AcctError> {
    device_code_with_sleep(http, client_id, notify, is_cancelled, std::thread::sleep)
}

/// EN: `device_code` with an injectable sleep — this is what tests drive with
/// a no-op sleeper. RU: `device_code` с подставляемым сном — для тестов.
pub fn device_code_with_sleep<S: Fn(std::time::Duration)>(
    http: &dyn Http,
    client_id: &str,
    notify: &dyn Fn(&str),
    is_cancelled: &dyn Fn() -> bool,
    sleep: S,
) -> Result<MsaTokens, AcctError> {
    let info = request_device_code(http, client_id)?;
    let msg = info.message.clone().unwrap_or_else(|| {
        format!("Go to {} and enter the code {}", info.verification_uri, info.user_code)
    });
    notify(&msg);

    let mut interval = info.interval.max(1);
    // The endpoint itself returns `expired_token` when the device code dies,
    // so no local deadline is enforced; a cancelled poll is the caller's exit.

    loop {
        if is_cancelled() {
            return Err(AcctError::Cancelled);
        }
        sleep(std::time::Duration::from_secs(interval));
        if is_cancelled() {
            return Err(AcctError::Cancelled);
        }
        let body = form_body(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", &info.device_code),
        ]);
        match http.post_form(&format!("{LIVE_BASE}/token"), &body) {
            Ok(resp) => return parse_tokens(&resp),
            Err(e) => {
                if e.is_authorization_pending() {
                    continue;
                } else if e.is_slow_down() {
                    interval += 5;
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

/// EN: exchange a stored refresh token for fresh MSA tokens.
/// RU: обменять сохранённый refresh-токен на свежие MSA-токены.
pub fn refresh(http: &dyn Http, client_id: &str, refresh_token: &str) -> Result<MsaTokens, AcctError> {
    let body = form_body(&[
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("scope", SCOPE),
    ]);
    let resp = http.post_form(&format!("{LIVE_BASE}/token"), &body)?;
    let tokens = parse_tokens(&resp)?;
    // MSA sometimes omits refresh_token on refresh: then keep the old one.
    Ok(MsaTokens {
        access: tokens.access,
        refresh: if tokens.refresh.is_empty() {
            refresh_token.to_string()
        } else {
            tokens.refresh
        },
    })
}

/// EN: parse a `/token` success body. RU: разобрать успешный ответ `/token`.
pub fn parse_tokens(body: &str) -> Result<MsaTokens, AcctError> {
    let j = parse(body).map_err(|e| AcctError::BadJson(e.to_string()))?;
    let access = field(&j, "access_token")?;
    let refresh = field(&j, "refresh_token").unwrap_or_default();
    Ok(MsaTokens { access, refresh })
}

fn field(j: &Json, key: &str) -> Result<String, AcctError> {
    j.get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AcctError::BadField(format!("missing field `{key}`")))
}

fn int_field(j: &Json, key: &str) -> Option<u64> {
    j.get(key).and_then(Json::as_f64).map(|v| v.max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// scripted fake: pops one canned response per call. Panics on an
    /// unexpected extra call (tests assert the exact script length).
    struct FakeHttp {
        posts: RefCell<VecDeque<Result<String, AcctError>>>,
    }

    impl Http for FakeHttp {
        fn post_json(&self, _: &str, _: &str) -> Result<String, AcctError> {
            Err(AcctError::Http("unexpected post_json".into()))
        }
        fn post_form(&self, _: &str, _: &str) -> Result<String, AcctError> {
            self.posts.borrow_mut().pop_front().expect("call not scripted")
        }
        fn get_json(&self, _: &str, _: Option<&str>) -> Result<String, AcctError> {
            Err(AcctError::Http("unexpected get_json".into()))
        }
    }

    impl FakeHttp {
        /// number of scripted responses still unconsumed
        fn remaining(&self) -> usize {
            self.posts.borrow().len()
        }
    }

    const DEVICE_CODE_RESP: &str = r#"{
        "user_code":"BCDSTUV",
        "device_code":"FQABAAEAAAP...",
        "verification_uri":"https://www.microsoft.com/link",
        "expires_in":900,
        "interval":5,
        "message":"To sign in, use a web browser to open the page https://www.microsoft.com/link and enter the code BCDSTUV to authenticate."
    }"#;

    const TOKEN_OK: &str = r#"{"token_type":"Bearer","scope":"XboxLive.signin offline_access","expires_in":86399,"access_token":"EwAoA0l...","refresh_token":"MC.MC.MC..."}"#;

    #[test]
    fn parse_tokens_reads_pair() {
        let t = parse_tokens(TOKEN_OK).unwrap();
        assert_eq!(t.access, "EwAoA0l...");
        assert_eq!(t.refresh, "MC.MC.MC...");
    }

    #[test]
    fn request_device_code_parses_fields() {
        let h = FakeHttp { posts: RefCell::new(VecDeque::from([Ok(DEVICE_CODE_RESP.to_string())])) };
        let info = request_device_code(&h, "cid").unwrap();
        assert_eq!(info.user_code, "BCDSTUV");
        assert_eq!(info.verification_uri, "https://www.microsoft.com/link");
        assert_eq!(info.interval, 5);
        assert_eq!(info.expires_in, 900);
        assert!(info.message.as_deref().unwrap().contains("BCDSTUV"));
        assert_eq!(h.remaining(), 0);
    }

    #[test]
    fn device_code_polls_until_granted() {
        let h = FakeHttp {
            posts: RefCell::new(VecDeque::from([
                Ok(DEVICE_CODE_RESP.to_string()),
                Err(AcctError::Status(400, r#"{"error":"authorization_pending"}"#.into())),
                Err(AcctError::Status(400, r#"{"error":"slow_down"}"#.into())),
                Ok(TOKEN_OK.to_string()),
            ])),
        };
        let notified = RefCell::new(Vec::new());
        let t = device_code_with_sleep(&h, "cid", &|m| notified.borrow_mut().push(m.to_string()), &|| false, |_| {})
            .unwrap();
        assert_eq!(t.access, "EwAoA0l...");
        let notes = notified.borrow();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("https://www.microsoft.com/link"));
        assert!(notes[0].contains("BCDSTUV"));
        // all 4 scripted calls consumed
        assert_eq!(h.remaining(), 0);
    }

    #[test]
    fn device_code_propagates_hard_error() {
        let h = FakeHttp {
            posts: RefCell::new(VecDeque::from([
                Ok(DEVICE_CODE_RESP.to_string()),
                Err(AcctError::Status(400, r#"{"error":"expired_token"}"#.into())),
            ])),
        };
        let err = device_code_with_sleep(&h, "cid", &|_| {}, &|| false, |_| {}).unwrap_err();
        assert!(matches!(err, AcctError::Status(400, _)));
        assert_eq!(h.remaining(), 0);
    }

    #[test]
    fn device_code_respects_cancel() {
        let h = FakeHttp { posts: RefCell::new(VecDeque::from([Ok(DEVICE_CODE_RESP.to_string())])) };
        let err = device_code_with_sleep(&h, "cid", &|_| {}, &|| true, |_| {}).unwrap_err();
        assert!(matches!(err, AcctError::Cancelled));
        assert_eq!(h.remaining(), 0); // only the devicecode request happened
    }

    #[test]
    fn refresh_keeps_old_token_when_omitted() {
        let resp = r#"{"access_token":"newA"}"#;
        let h = FakeHttp { posts: RefCell::new(VecDeque::from([Ok(resp.to_string())])) };
        let t = refresh(&h, "cid", "old-refresh").unwrap();
        assert_eq!(t.access, "newA");
        assert_eq!(t.refresh, "old-refresh");
    }

    #[test]
    fn refresh_maps_expired_token() {
        let h = FakeHttp {
            posts: RefCell::new(VecDeque::from([
                Err(AcctError::Status(400, r#"{"error":"invalid_grant"}"#.into())),
            ])),
        };
        let err = refresh(&h, "cid", "dead").unwrap_err();
        assert!(matches!(err, AcctError::Status(400, _)));
    }
}
