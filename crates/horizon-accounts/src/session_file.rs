//! EN: session-file emission, ported from `write_session` in
//! `horizon-account-switcher.py`. The Horizon client reads
//! `--session-file <path>` (compact JSON `{"name","uuid","access_token",
//! "user_type":"msa","xuid"}`), refuses files older than 120 seconds or
//! malformed, and DELETES the file after reading it.
//!
//! So this module writes a fresh `session-<hex>.json` into
//! `<game_home>/sessions/` and never reuses one.
//!
//! RU: запись session-файла (порт `write_session`). Клиент горизонта читает
//! `--session-file`, требует возраст <= 120 c, корректный JSON и УДАЛЯЕТ файл
//! после чтения — поэтому каждый раз пишется свежий `session-<hex>.json`
//! в `<game_home>/sessions/`.

use crate::error::AcctError;
use crate::json::Json;
use crate::store::Account;
use std::path::{Path, PathBuf};

/// EN: build the exact session JSON body the client expects.
/// RU: собрать ровно тот JSON session-файла, который ждёт клиент.
pub fn session_json(name: &str, uuid: &str, access_token: &str, xuid: &str) -> String {
    Json::Obj(vec![
        ("name".into(), Json::s(name)),
        ("uuid".into(), Json::s(uuid)),
        ("access_token".into(), Json::s(access_token)),
        ("user_type".into(), Json::s("msa")),
        ("xuid".into(), Json::s(xuid)),
    ])
    .to_string_compact()
}

/// EN: write a fresh session file for `account` into
/// `<game_home>/sessions/session-<random hex>.json` and return its path.
/// The random hex comes from time + address-space entropy — good enough for
/// a filename that must simply be unique per write.
/// RU: записать свежий session-файл в `<game_home>/sessions/session-<hex>.json`
/// и вернуть путь. hex из времени + энтропии адресов — достаточно для уникальности имени.
pub fn write(game_home: &Path, a: &Account, mc_token: &str) -> Result<PathBuf, AcctError> {
    let dir = game_home.join("sessions");
    std::fs::create_dir_all(&dir)?;
    let hex = random_hex();
    let path = dir.join(format!("session-{hex}.json"));
    let body = session_json(&a.name, &a.uuid, mc_token, &a.xuid);
    std::fs::write(&path, body)?;
    Ok(path)
}

/// EN: 12 hex chars of best-effort entropy (no external crates).
/// RU: 12 hex-символов без внешних крейтов.
fn random_hex() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // address of a local box: ASLR-randomized per process
    let probe = 0u8;
    let addr = &probe as *const u8 as usize;
    mix(t as u64 ^ (addr as u64).rotate_left(32))
}

/// EN: splitmix64. RU: splitmix64.
fn mix(mut x: u64) -> String {
    let mut out = String::with_capacity(16);
    for _ in 0..2 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.push_str(&format!("{:08x}", (z >> 32) as u32));
        // second half
        let z2 = z.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        out.push_str(&format!("{:08x}", (z2 >> 32) as u32));
    }
    out.truncate(12);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn acct() -> Account {
        Account {
            uuid: "a83f14e84a1f4b34a1c69f2b3e8b7e01".into(),
            name: "Steve_Miner".into(),
            skin: None,
            refresh_token: "RT".into(),
            mc_token: "MC".into(),
            mc_expiry: 0,
            xuid: "2535401".into(),
            client_id: "cid".into(),
        }
    }

    #[test]
    fn session_json_shape_is_exact() {
        let s = session_json("Steve_Miner", "a83f14e84a1f4b34a1c69f2b3e8b7e01", "MC.A", "2535401");
        assert_eq!(
            s,
            r#"{"name":"Steve_Miner","uuid":"a83f14e84a1f4b34a1c69f2b3e8b7e01","access_token":"MC.A","user_type":"msa","xuid":"2535401"}"#
        );
        // all five fields the client validates are present, in order
        let j: Json = s.parse().unwrap();
        assert_eq!(j.as_obj().map(|p| p.len()), Some(5));
        assert_eq!(j.get("user_type").and_then(|v| v.as_str()), Some("msa"));
    }

    #[test]
    fn writes_fresh_unique_files() {
        let mut home = temp_dir();
        home.push(format!(
            "horizon-sess-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let p1 = write(&home, &acct(), "MC.A").unwrap();
        let p2 = write(&home, &acct(), "MC.B").unwrap();
        assert!(p1.starts_with(home.join("sessions")));
        assert_ne!(p1, p2, "two writes must produce two distinct files");
        let body = std::fs::read_to_string(&p1).unwrap();
        assert_eq!(body, session_json("Steve_Miner", "a83f14e84a1f4b34a1c69f2b3e8b7e01", "MC.A", "2535401"));
        let body2 = std::fs::read_to_string(&p2).unwrap();
        assert!(body2.contains("\"access_token\":\"MC.B\""));
        // filenames: session-<12 hex chars>.json
        let f1 = p1.file_name().unwrap().to_str().unwrap();
        assert!(f1.starts_with("session-") && f1.ends_with(".json") && f1.len() == "session-".len() + 12 + ".json".len());
        std::fs::remove_dir_all(&home).ok();
    }
}
