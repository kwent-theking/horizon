//! EN: Minecraft profile lookup: `GET /minecraft/profile` with the bearer
//! token, ported from `mc_profile` in `horizon-account-switcher.py`.
//!
//! RU: профиль Minecraft: `GET /minecraft/profile` с bearer-токеном
//! (порт `mc_profile` из horizon-account-switcher.py).

use crate::error::AcctError;
use crate::http::Http;
use crate::json::{parse, Json};

/// EN: profile endpoint. RU: эндпоинт профиля.
pub const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// EN: the player identity the client needs. RU: идентичность игрока.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// EN: in-game name. RU: имя в игре.
    pub name: String,
    /// EN: undashed (32 hex chars) profile UUID, the format horizon-client expects.
    /// RU: UUID без дефисов (32 hex-символа) — формат для horizon-client.
    pub uuid: String,
    /// EN: active skin URL, if any. RU: URL активного скина, если есть.
    pub skin: Option<String>,
}

/// EN: fetch the profile with the Minecraft bearer token.
/// RU: получить профиль по Minecraft bearer-токену.
pub fn fetch(http: &dyn Http, mc_token: &str) -> Result<Profile, AcctError> {
    let auth = format!("Bearer {mc_token}");
    let resp = http.get_json(PROFILE_URL, Some(&auth))?;
    parse_profile(&resp)
}

/// EN: parse a profile response body (golden strings in tests).
/// RU: разобрать тело ответа профиля (golden-строки в тестах).
pub fn parse_profile(body: &str) -> Result<Profile, AcctError> {
    let j = parse(body).map_err(|e| AcctError::BadJson(e.to_string()))?;
    let name = j
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| AcctError::BadField("missing profile `name`".into()))?
        .to_owned();
    let id = j
        .get("id")
        .and_then(Json::as_str)
        .ok_or_else(|| AcctError::BadField("missing profile `id`".into()))?;
    let uuid = undash(id);
    if uuid.len() != 32 || !uuid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AcctError::BadField(format!("bad profile uuid: {id}")));
    }
    let skin = j
        .get("skins")
        .and_then(Json::as_arr)
        .and_then(|s| s.first())
        .and_then(|s| s.get("url"))
        .and_then(Json::as_str)
        .map(str::to_owned);
    Ok(Profile { name, uuid, skin })
}

/// EN: strip dashes from a dashed or undashed UUID and lowercase it.
/// RU: убрать дефисы из UUID (dashed или нет) и привести к нижнему регистру.
pub fn undash(uuid: &str) -> String {
    uuid.chars().filter(|c| *c != '-').flat_map(|c| c.to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE_RESP: &str = r#"{
        "id" : "a83f14e8-4a1f-4b34-a1c6-9f2b3e8b7e01",
        "name" : "Steve_Miner",
        "skins" : [
            {
                "id" : "6a6e65e4-47dd-499f-9d31-5c6f6bd21ac1",
                "state" : "ACTIVE",
                "url" : "https://textures.minecraft.net/texture/1a4af718",
                "variant" : "CLASSIC",
                "alias" : "Steve"
            }
        ]
    }"#;

    struct FakeHttp {
        calls: std::cell::RefCell<Vec<(String, Option<String>)>>,
        resp: String,
    }

    impl Http for FakeHttp {
        fn post_json(&self, _: &str, _: &str) -> Result<String, AcctError> {
            unreachable!("profile never POSTs")
        }
        fn post_form(&self, _: &str, _: &str) -> Result<String, AcctError> {
            unreachable!("profile never POSTs")
        }
        fn get_json(&self, url: &str, auth: Option<&str>) -> Result<String, AcctError> {
            self.calls.borrow_mut().push((url.to_string(), auth.map(str::to_owned)));
            Ok(self.resp.clone())
        }
    }

    #[test]
    fn parse_profile_golden() {
        let p = parse_profile(PROFILE_RESP).unwrap();
        assert_eq!(p.name, "Steve_Miner");
        assert_eq!(p.uuid, "a83f14e84a1f4b34a1c69f2b3e8b7e01");
        assert_eq!(
            p.skin.as_deref(),
            Some("https://textures.minecraft.net/texture/1a4af718")
        );
    }

    #[test]
    fn parse_profile_without_skins() {
        let body = r#"{"id":"A83F14E84A1F4B34A1C69F2B3E8B7E01","name":"Alex"}"#;
        let p = parse_profile(body).unwrap();
        assert_eq!(p.uuid, "a83f14e84a1f4b34a1c69f2b3e8b7e01");
        assert_eq!(p.skin, None);
    }

    #[test]
    fn parse_profile_rejects_bad_uuid() {
        let body = r#"{"id":"too-short","name":"X"}"#;
        let err = parse_profile(body).unwrap_err();
        assert!(matches!(err, AcctError::BadField(_)));
    }

    #[test]
    fn fetch_sends_bearer_header() {
        let h = FakeHttp {
            calls: std::cell::RefCell::new(Vec::new()),
            resp: r#"{"id":"a83f14e84a1f4b34a1c69f2b3e8b7e01","name":"N"}"#.into(),
        };
        let p = fetch(&h, "MC.TOKEN").unwrap();
        assert_eq!(p.name, "N");
        let calls = h.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, PROFILE_URL);
        assert_eq!(calls[0].1.as_deref(), Some("Bearer MC.TOKEN"));
    }

    #[test]
    fn undash_variants() {
        assert_eq!(undash("a83f14e8-4a1f-4b34-a1c6-9f2b3e8b7e01"), "a83f14e84a1f4b34a1c69f2b3e8b7e01");
        assert_eq!(undash("A83F14E84A1F4B34A1C69F2B3E8B7E01"), "a83f14e84a1f4b34a1c69f2b3e8b7e01");
    }
}
