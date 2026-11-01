//! Error type shared by the whole crate. EN docs; short RU notes inline.

use std::fmt;

/// EN: everything that can go wrong in the account switcher.
/// RU: всё, что может пойти не так в переключателе аккаунтов.
#[derive(Debug)]
pub enum AcctError {
    /// EN: transport-level failure (DNS, TLS, connection, malformed response).
    /// RU: сбой транспорта (DNS, TLS, соединение, кривой ответ).
    Http(String),
    /// EN: HTTP status + body snippet, e.g. `authorization_pending` polls.
    /// RU: HTTP-статус + кусок тела (например, poll `authorization_pending`).
    Status(u16, String),
    /// EN: response was not valid JSON or lacked an expected field.
    /// RU: ответ не JSON или в нём нет нужного поля.
    BadJson(String),
    /// EN: a field had the right name but the wrong shape.
    /// RU: поле есть, но не того типа.
    BadField(String),
    /// EN: XBL/XSTS rejected the account (XErr codes 2148916233..238:
    /// no Xbox account, region/ban, adult verification).
    /// RU: Xbox/XSTS отклонил аккаунт (XErr 2148916233..238: нет Xbox-аккаунта,
    /// регион/бан, проверка совершеннолетия).
    XboxRejected(String),
    /// EN: the MSA refresh token is no longer valid — the user must sign in again.
    /// RU: refresh-токен MSA больше не действителен — нужен повторный вход.
    RefreshExpired,
    /// EN: device-code flow timed out before the user approved it.
    /// RU: истёк таймаут device-code входа, пока пользователь не подтвердил.
    DeviceCodeTimeout,
    /// EN: caller-requested abort (the `is_cancelled` closure returned true).
    /// RU: вызывающий код запросил отмену (`is_cancelled` вернул true).
    Cancelled,
    /// EN: an account with the given reference was not found in the store.
    /// RU: аккаунт по ссылке не найден в хранилище.
    NotFound(String),
    /// EN: I/O on the store or session file.
    /// RU: ошибка ввода-вывода хранилища или session-файла.
    Io(std::io::Error),
}

impl fmt::Display for AcctError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcctError::Http(m) => write!(f, "network error: {m}"),
            AcctError::Status(code, body) => {
                write!(f, "HTTP {code}: {body}")
            }
            AcctError::BadJson(m) => write!(f, "bad JSON: {m}"),
            AcctError::BadField(m) => write!(f, "bad field: {m}"),
            AcctError::XboxRejected(m) => write!(f, "Xbox/XSTS rejected this account: {m}"),
            AcctError::RefreshExpired => write!(f, "MSA refresh token expired, sign in again (add)"),
            AcctError::DeviceCodeTimeout => write!(f, "device code flow timed out"),
            AcctError::Cancelled => write!(f, "cancelled"),
            AcctError::NotFound(r) => write!(f, "account not found: {r}"),
            AcctError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for AcctError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AcctError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AcctError {
    fn from(e: std::io::Error) -> Self {
        AcctError::Io(e)
    }
}

impl AcctError {
    /// EN: true when a `/token` poll error means "keep polling"
    /// (`authorization_pending`) — used by the msa module and its tests.
    /// RU: true, если ошибка poll `/token` означает «продолжай опрашивать».
    pub fn is_authorization_pending(&self) -> bool {
        matches!(self, AcctError::Status(400, b) if b.contains("authorization_pending"))
    }

    /// EN: true when the poll must slow down (`slow_down`).
    /// RU: true, если poll должен замедлиться (`slow_down`).
    pub fn is_slow_down(&self) -> bool {
        matches!(self, AcctError::Status(400, b) if b.contains("slow_down"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_human_readable() {
        assert_eq!(
            AcctError::Status(400, "authorization_pending".into()).to_string(),
            "HTTP 400: authorization_pending"
        );
        assert_eq!(
            AcctError::NotFound("Steve".into()).to_string(),
            "account not found: Steve"
        );
    }

    #[test]
    fn pending_and_slow_down_classification() {
        assert!(AcctError::Status(400, r#"{"error":"authorization_pending"}"#.into())
            .is_authorization_pending());
        assert!(AcctError::Status(400, r#"{"error":"slow_down"}"#.into()).is_slow_down());
        assert!(!AcctError::Status(500, "slow_down".into()).is_slow_down());
        assert!(!AcctError::Status(400, "expired_token".into()).is_authorization_pending());
    }
}
