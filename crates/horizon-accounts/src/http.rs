//! EN: HTTP abstraction. The whole crate talks to the network only through
//! the [`Http`] trait, so tests can drive the MSA/Xbox chain with fake
//! responses and the crate compiles without any TLS dependency when the
//! `client` feature is off.
//!
//! RU: абстракция HTTP. Вся сеть в крейте — только через трейт [`Http`],
//! поэтому тесты гоняют MSA/Xbox-цепочку на фейковых ответах, а без feature
//! `client` крейт компилируется вообще без TLS-зависимостей.

use crate::error::AcctError;

/// EN: blocking HTTP transport for JSON endpoints.
/// All calls have a 30-second timeout, mirroring the Python original.
/// RU: блокирующий HTTP-транспорт для JSON-эндпоинтов, таймаут 30 секунд
/// (как в исходном Python-скрипте).
pub trait Http {
    /// EN: POST with a JSON body (Content-Type: application/json),
    /// returns the response body on 2xx.
    /// RU: POST с JSON-телом, возвращает тело ответа при 2xx.
    fn post_json(&self, url: &str, body: &str) -> Result<String, AcctError>;

    /// EN: POST with an x-www-form-urlencoded body, returns the body on 2xx.
    /// RU: POST с формой urlencoded, возвращает тело при 2xx.
    fn post_form(&self, url: &str, body: &str) -> Result<String, AcctError>;

    /// EN: GET with an optional `Authorization` header, returns the body on 2xx.
    /// RU: GET с опциональным заголовком `Authorization`, тело при 2xx.
    fn get_json(&self, url: &str, auth: Option<&str>) -> Result<String, AcctError>;
}

/// EN: percent-encode a form value (application/x-www-form-urlencoded).
/// Only the unreserved set `A-Za-z0-9-._~` plus space-as-`%20` goes through raw.
/// RU: процентное кодирование значения формы.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// EN: build a urlencoded body from pairs. RU: собрать urlencoded-тело из пар.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// EN: default transport. With the `client` feature this is `UreqHttp`
/// over TLS; without it, every call fails (the crate stays offline-buildable).
/// RU: транспорт по умолчанию: с feature `client` — `UreqHttp` через TLS,
/// без неё — заглушка-ошибка (крейт собирается оффлайн).
pub fn default_http() -> Box<dyn Http> {
    #[cfg(feature = "client")]
    {
        Box::new(UreqHttp::new())
    }
    #[cfg(not(feature = "client"))]
    {
        Box::new(NoHttp)
    }
}

/// EN: transport used when the `client` feature is disabled.
/// RU: транспорт при выключенной feature `client`.
#[cfg(not(feature = "client"))]
struct NoHttp;

#[cfg(not(feature = "client"))]
impl Http for NoHttp {
    fn post_json(&self, _: &str, _: &str) -> Result<String, AcctError> {
        Err(AcctError::Http("client feature is disabled; rebuild with --features client".into()))
    }
    fn post_form(&self, _: &str, _: &str) -> Result<String, AcctError> {
        Err(AcctError::Http("client feature is disabled; rebuild with --features client".into()))
    }
    fn get_json(&self, _: &str, _: Option<&str>) -> Result<String, AcctError> {
        Err(AcctError::Http("client feature is disabled; rebuild with --features client".into()))
    }
}

/// EN: ureq-backed TLS transport (feature "client").
/// RU: TLS-транспорт на ureq (feature "client").
#[cfg(feature = "client")]
pub struct UreqHttp {
    agent: ureq::Agent,
}

#[cfg(feature = "client")]
impl UreqHttp {
    /// EN: new agent with a 30s timeout and gzip off (bodies are small JSON).
    /// RU: новый агент с таймаутом 30 c и выключенным gzip.
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        UreqHttp { agent }
    }
}

#[cfg(feature = "client")]
impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "client")]
impl Http for UreqHttp {
    fn post_json(&self, url: &str, body: &str) -> Result<String, AcctError> {
        read_body(self.agent.post(url).set("Content-Type", "application/json").send_string(body))
    }

    fn post_form(&self, url: &str, body: &str) -> Result<String, AcctError> {
        read_body(
            self.agent
                .post(url)
                .set("Content-Type", "application/x-www-form-urlencoded")
                .send_string(body),
        )
    }

    fn get_json(&self, url: &str, auth: Option<&str>) -> Result<String, AcctError> {
        let mut req = self.agent.get(url).set("Accept", "application/json");
        if let Some(a) = auth {
            req = req.set("Authorization", a);
        }
        read_body(req.call())
    }
}

/// EN: run a ureq call, map the status/transport error, read the body.
/// RU: выполнить вызов ureq, смаппить ошибку, прочитать тело.
#[cfg(feature = "client")]
fn read_body(r: Result<ureq::Response, ureq::Error>) -> Result<String, AcctError> {
    let resp = r.map_err(ureq_err)?;
    resp.into_string().map_err(|e| AcctError::Http(e.to_string()))
}

/// EN: map a ureq error to [`AcctError`], keeping the HTTP status and a body
/// snippet for error-classification (authorization_pending / slow_down).
/// RU: превратить ошибку ureq в [`AcctError`], сохранив статус и кусок тела
/// (для классификации authorization_pending / slow_down).
#[cfg(feature = "client")]
fn ureq_err(e: ureq::Error) -> AcctError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            AcctError::Status(code, clip(&body))
        }
        ureq::Error::Transport(t) => AcctError::Http(t.to_string()),
    }
}

/// EN: keep error bodies short. RU: обрезать тело ошибки.
#[cfg(feature = "client")]
fn clip(s: &str) -> String {
    if s.len() > 512 {
        let mut end = 512;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_keeps_unreserved_and_escapes_rest() {
        assert_eq!(urlencode("XboxLive.signin offline_access"), "XboxLive.signin%20offline_access");
        assert_eq!(urlencode("a=b&c"), "a%3Db%26c");
        assert_eq!(urlencode("-._~"), "-._~");
        // non-ascii -> UTF-8 percent bytes
        assert_eq!(urlencode("é"), "%C3%A9");
    }

    #[test]
    fn form_body_joins_pairs() {
        let body = form_body(&[("grant_type", "refresh_token"), ("client_id", "abc def")]);
        assert_eq!(body, "grant_type=refresh_token&client_id=abc%20def");
    }

    #[test]
    fn no_http_stub_when_feature_off() {
        // This test only exercises the stub path when the feature is off;
        // in default builds (feature on) it checks nothing beyond compilation
        // of the trait object. Keep it cheap either way.
        if cfg!(not(feature = "client")) {
            let h = default_http();
            assert!(h.get_json("https://example.test", None).is_err());
        }
    }
}
