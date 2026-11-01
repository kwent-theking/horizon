//! EN: the Xbox Live -> XSTS -> Minecraft token chain, ported from
//! `horizon-account-switcher.py` (`xsts_chain`). Takes an MSA access token,
//! returns the Minecraft services bearer token plus the account XUID.
//!
//! RU: цепочка Xbox Live -> XSTS -> Minecraft-токен (порт `xsts_chain`):
//! на входе access-токен MSA, на выходе bearer-токен Minecraft services и XUID.

use crate::error::AcctError;
use crate::http::Http;
use crate::json::{parse, Json};

/// EN: XBL user authentication. RU: аутентификация пользователя XBL.
pub const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
/// EN: XSTS authorization. RU: авторизация XSTS.
pub const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
/// EN: Minecraft launcher login (primary). RU: вход в Minecraft (основной).
pub const MC_LAUNCHER_LOGIN: &str = "https://api.minecraftservices.com/launcher/login";
/// EN: Minecraft login with xbox (legacy fallback). RU: legacy-фолбэк входа.
pub const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";

/// EN: XErr codes that mean "this account cannot play" (no Xbox account,
/// region/ban, adult verification) — surfaced as [`AcctError::XboxRejected`].
/// RU: коды XErr «аккаунт играть не может» — возвращаются как [`AcctError::XboxRejected`].
const XERR_REJECTED: &[&str] = &[
    "2148916233", "2148916234", "2148916235", "2148916236", "2148916237", "2148916238",
];

/// EN: run the whole chain: MSA access token -> XBL -> XSTS -> Minecraft.
/// Returns `(mc_token, xuid)`; XUID may be empty when XSTS sent no `xid`.
/// RU: вся цепочка: access-токен MSA -> XBL -> XSTS -> Minecraft.
/// Возвращает `(mc_token, xuid)`; XUID может быть пустым, если XSTS не прислал `xid`.
pub fn minecraft_token(http: &dyn Http, ms_access: &str) -> Result<(String, String), AcctError> {
    let xbl = xbl_token(http, ms_access)?;
    let (xsts_token, uhs, xuid) = xsts_token(http, &xbl)?;
    let xtoken = format!("XBL3.0 x={uhs};{xsts_token}");
    let mc = mc_login(http, &xtoken)?;
    Ok((mc, xuid))
}

/// EN: step 1 — exchange the MSA token for an XBL user token.
/// RU: шаг 1 — обменять токен MSA на пользовательский токен XBL.
pub fn xbl_token(http: &dyn Http, ms_access: &str) -> Result<String, AcctError> {
    let body = r#"{"Properties":{"AuthMethod":"RPS","SiteName":"user.auth.xboxlive.com","RpsTicket":"d=<TOK>"},"RelyingParty":"http://auth.xboxlive.com","TokenType":"JWT"}"#
        .replace("<TOK>", &json_escape(ms_access));
    let resp = http.post_json(XBL_URL, &body)?;
    let j = parse(&resp).map_err(|e| AcctError::BadJson(e.to_string()))?;
    j.get("Token")
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AcctError::BadField("missing XBL `Token`".into()))
}

/// EN: step 2 — exchange the XBL token for an XSTS token.
/// Returns `(xsts_token, uhs, xuid)`.
/// RU: шаг 2 — обменять XBL-токен на XSTS-токен. Возвращает `(token, uhs, xuid)`.
pub fn xsts_token(http: &dyn Http, xbl_token: &str) -> Result<(String, String, String), AcctError> {
    let body = format!(
        r#"{{"Properties":{{"SandboxId":"RETAIL","UserTokens":["{}"]}},"RelyingParty":"rp://api.minecraftservices.com/","TokenType":"JWT"}}"#,
        json_escape(xbl_token)
    );
    let resp = match http.post_json(XSTS_URL, &body) {
        Ok(r) => r,
        Err(e) => {
            let text = e.to_string();
            if XERR_REJECTED.iter().any(|c| text.contains(c)) {
                return Err(AcctError::XboxRejected(text));
            }
            return Err(e);
        }
    };
    let j = parse(&resp).map_err(|e| AcctError::BadJson(e.to_string()))?;
    let token = j
        .get("Token")
        .and_then(Json::as_str)
        .ok_or_else(|| AcctError::BadField("missing XSTS `Token`".into()))?
        .to_owned();
    let xui = j
        .get("DisplayClaims")
        .and_then(|d| d.get("xui"))
        .and_then(Json::as_arr)
        .and_then(|a| a.first())
        .ok_or_else(|| AcctError::BadField("missing XSTS DisplayClaims.xui[0]".into()))?;
    let uhs = xui
        .get("uhs")
        .and_then(Json::as_str)
        .ok_or_else(|| AcctError::BadField("missing XSTS `uhs`".into()))?
        .to_owned();
    let xuid = xui.get("xid").and_then(Json::as_str).unwrap_or("").to_owned();
    Ok((token, uhs, xuid))
}

/// EN: step 3 — Minecraft services login with the XSTS token
/// (`launcher/login` first, `login_with_xbox` as legacy fallback).
/// RU: шаг 3 — вход в Minecraft services с XSTS-токеном
/// (сначала `launcher/login`, фолбэк — `login_with_xbox`).
pub fn mc_login(http: &dyn Http, xtoken: &str) -> Result<String, AcctError> {
    let body = format!(
        r#"{{"xtoken":"{}","platform":"PC_LAUNCHER"}}"#,
        json_escape(xtoken)
    );
    let resp = match http.post_json(MC_LAUNCHER_LOGIN, &body) {
        Ok(r) => r,
        Err(_) => {
            let legacy = format!(
                r#"{{"identityToken":"{}"}}"#,
                json_escape(xtoken)
            );
            http.post_json(MC_LOGIN, &legacy)?
        }
    };
    let j = parse(&resp).map_err(|e| AcctError::BadJson(e.to_string()))?;
    j.get("access_token")
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AcctError::BadField("missing `access_token` in Minecraft login response".into()))
}

/// EN: minimal JSON string escaper for token interpolation into request bodies.
/// RU: минимальный эскейпер для вставки токенов в тела запросов.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// EN: one scripted response. `Err(String)` carries a body for a
    /// synthesized HTTP error status. RU: один запрограммированный ответ.
    type Scripted = (String, Result<String, u16>);

    struct FakeHttp {
        /// (url, outcome) per call, consumed in order
        script: RefCell<Vec<Scripted>>,
        calls: RefCell<Vec<(String, String)>>,
    }

    impl FakeHttp {
        fn new(script: Vec<(&str, Result<&str, u16>)>) -> Self {
            FakeHttp {
                script: RefCell::new(
                    script
                        .into_iter()
                        .map(|(u, r)| {
                            let out = match r {
                                Ok(body) => Ok(body.to_string()),
                                Err(code) => Err(code),
                            };
                            (u.to_string(), out)
                        })
                        .collect(),
                ),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn take(&self, url: &str) -> Result<String, AcctError> {
            let next = self.script.borrow_mut().remove(0);
            match next {
                (want, Ok(body)) => {
                    assert_eq!(want, url, "unexpected call order");
                    Ok(body)
                }
                (want, Err(code)) => {
                    assert_eq!(want, url, "unexpected call order");
                    Err(AcctError::Status(code, format!("scripted {code} for {url}")))
                }
            }
        }
    }

    impl Http for FakeHttp {
        fn post_json(&self, url: &str, body: &str) -> Result<String, AcctError> {
            self.calls.borrow_mut().push((url.to_string(), body.to_string()));
            self.take(url)
        }
        fn post_form(&self, _: &str, _: &str) -> Result<String, AcctError> {
            unreachable!("xbox chain never uses forms")
        }
        fn get_json(&self, _: &str, _: Option<&str>) -> Result<String, AcctError> {
            unreachable!("xbox chain never uses GET")
        }
    }

    const XBL_RESP: &str =
        r#"{"Token":"XBL.TOK","DisplayClaims":{"xui":[{"uhs":"UHS123","xid":"2535401"}]}}"#;
    const XSTS_RESP: &str =
        r#"{"Token":"XSTS.TOK","DisplayClaims":{"xui":[{"uhs":"UHS123","xid":"2535401"}]}}"#;
    const MC_RESP: &str = r#"{"access_token":"MC.A","expires_in":86400}"#;

    #[test]
    fn full_chain_launcher_login() {
        let h = FakeHttp::new(vec![
            (XBL_URL, Ok(XBL_RESP)),
            (XSTS_URL, Ok(XSTS_RESP)),
            (MC_LAUNCHER_LOGIN, Ok(MC_RESP)),
        ]);
        let (mc, xuid) = minecraft_token(&h, "msa-access").unwrap();
        assert_eq!(mc, "MC.A");
        assert_eq!(xuid, "2535401");
        let calls = h.calls.borrow();
        // XBL body carries the RPS ticket
        assert!(calls[0].1.contains(r#""RpsTicket":"d=msa-access""#));
        // XSTS body carries the XBL token
        assert!(calls[1].1.contains(r#""UserTokens":["XBL.TOK"]"#));
        // MC body carries the combined xtoken
        assert!(calls[2].1.contains(r#""xtoken":"XBL3.0 x=UHS123;XSTS.TOK""#));
        assert!(calls[2].1.contains(r#""platform":"PC_LAUNCHER""#));
    }

    #[test]
    fn full_chain_falls_back_to_legacy_login() {
        let h = FakeHttp::new(vec![
            (XBL_URL, Ok(XBL_RESP)),
            (XSTS_URL, Ok(XSTS_RESP)),
            (MC_LAUNCHER_LOGIN, Err(404)),
            (MC_LOGIN, Ok(MC_RESP)),
        ]);
        let (mc, xuid) = minecraft_token(&h, "msa-access").unwrap();
        assert_eq!(mc, "MC.A");
        assert_eq!(xuid, "2535401");
        let calls = h.calls.borrow();
        assert!(calls[3].1.contains(r#""identityToken":"XBL3.0 x=UHS123;XSTS.TOK""#));
    }

    #[test]
    fn xsts_xerr_rejected_maps_to_xbox_rejected() {
        // plain 401 status error (no XErr body) still propagates as a Status
        let h = FakeHttp::new(vec![(XSTS_URL, Err(401))]);
        let err = xsts_token(&h, "XBL.TOK").unwrap_err();
        assert!(matches!(err, AcctError::Status(401, _)));
        // a body carrying XErr 2148916233 maps to XboxRejected — exercised
        // via the public matcher on the real error text
        let e2 = AcctError::Status(401, r#"{"XErr":2148916233}"#.into());
        let text = e2.to_string();
        assert!(XERR_REJECTED.iter().any(|c| text.contains(c)));
    }

    #[test]
    fn xsts_xuid_optional() {
        let resp = r#"{"Token":"XSTS.TOK","DisplayClaims":{"xui":[{"uhs":"UHS123"}]}}"#;
        let h = FakeHttp::new(vec![(XSTS_URL, Ok(resp))]);
        let (tok, uhs, xuid) = xsts_token(&h, "XBL.TOK").unwrap();
        assert_eq!((tok.as_str(), uhs.as_str(), xuid.as_str()), ("XSTS.TOK", "UHS123", ""));
    }

    #[test]
    fn tokens_are_escaped_in_bodies() {
        let h = FakeHttp::new(vec![(XBL_URL, Ok(XBL_RESP))]);
        xbl_token(&h, r#"we"ird\token"#).unwrap();
        let calls = h.calls.borrow();
        let body = &calls[0].1;
        assert!(body.contains(r#""RpsTicket":"d=we\"ird\\token""#));
    }
}
