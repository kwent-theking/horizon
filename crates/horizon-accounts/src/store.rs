//! EN: on-disk account store (`accounts.json`), ported from
//! `load_accounts`/`save_accounts`/`resolve` in `horizon-account-switcher.py`.
//!
//! Tokens are stored as-is in the JSON file next to the session directory.
//! There is no DPAPI layer here (the Python original had one): the file lives
//! in the user's `%LOCALAPPDATA%` profile, so the OS-level per-user ACL is the
//! protection boundary. If you need more, wrap the whole directory with
//! BitLocker/FS encryption — do not store refresh tokens anywhere shared.
//!
//! RU: хранилище аккаунтов на диске (`accounts.json`). Токены лежат как есть
//! в JSON рядом с каталогом сессий; защита — права доступа ОС на уровне
//! профиля пользователя (%LOCALAPPDATA%), DPAPI здесь нет (см. доку выше).

use crate::error::AcctError;
use crate::json::{parse, Json};
use std::path::{Path, PathBuf};

/// EN: one stored account. RU: один сохранённый аккаунт.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// EN: undashed profile UUID (32 hex). RU: UUID без дефисов (32 hex).
    pub uuid: String,
    /// EN: in-game name. RU: игровое имя.
    pub name: String,
    /// EN: active skin URL, if known. RU: URL скина, если известен.
    pub skin: Option<String>,
    /// EN: MSA refresh token (the long-lived secret). RU: refresh-токен MSA.
    pub refresh_token: String,
    /// EN: cached Minecraft bearer token. RU: кэшированный MC bearer-токен.
    pub mc_token: String,
    /// EN: when `mc_token` expires (unix seconds). RU: срок mc_token (unix).
    pub mc_expiry: i64,
    /// EN: Xbox user id, may be empty. RU: Xbox id, может быть пустым.
    pub xuid: String,
    /// EN: the MSA client_id this account was signed in with.
    /// RU: client_id, с которым аккаунт входил.
    pub client_id: String,
}

impl Account {
    /// EN: true when the cached MC token is still usable (with a 10-minute
    /// safety margin, like the Python `ensure_token`).
    /// RU: кэшированный токен ещё жив (запас 10 минут, как в `ensure_token`).
    pub fn mc_token_fresh(&self, now: i64) -> bool {
        !self.mc_token.is_empty() && self.mc_expiry > now + 600
    }

    fn to_json(&self) -> Json {
        let mut pairs: Vec<(String, Json)> = vec![
            ("uuid".into(), Json::s(self.uuid.clone())),
            ("name".into(), Json::s(self.name.clone())),
        ];
        if let Some(skin) = &self.skin {
            pairs.push(("skin".into(), Json::s(skin.clone())));
        }
        pairs.push(("refresh_token".into(), Json::s(self.refresh_token.clone())));
        pairs.push(("mc_token".into(), Json::s(self.mc_token.clone())));
        pairs.push(("mc_expiry".into(), Json::i(self.mc_expiry)));
        pairs.push(("xuid".into(), Json::s(self.xuid.clone())));
        pairs.push(("client_id".into(), Json::s(self.client_id.clone())));
        Json::Obj(pairs)
    }

    fn from_json(j: &Json) -> Result<Account, AcctError> {
        let s = |k: &str| -> Result<String, AcctError> {
            j.get(k)
                .and_then(Json::as_str)
                .map(str::to_owned)
                .ok_or_else(|| AcctError::BadField(format!("account missing `{k}`")))
        };
        Ok(Account {
            uuid: s("uuid")?,
            name: s("name")?,
            skin: j.get("skin").and_then(Json::as_str).map(str::to_owned),
            refresh_token: s("refresh_token")?,
            mc_token: j.get("mc_token").and_then(Json::as_str).unwrap_or("").to_owned(),
            mc_expiry: j.get("mc_expiry").and_then(Json::as_f64).map(|v| v as i64).unwrap_or(0),
            xuid: j.get("xuid").and_then(Json::as_str).unwrap_or("").to_owned(),
            client_id: s("client_id")?,
        })
    }
}

/// EN: the whole store: account list + selected uuid.
/// RU: всё хранилище: список аккаунтов + выбранный uuid.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountStore {
    /// EN: accounts, newest last. RU: аккаунты, новые в конце.
    pub accounts: Vec<Account>,
    /// EN: uuid of the selected account. RU: uuid выбранного аккаунта.
    pub selected: Option<String>,
    path: PathBuf,
}

impl AccountStore {
    /// EN: load the store from `path`; a missing file yields an empty store.
    /// RU: загрузить хранилище; нет файла — пустое хранилище.
    pub fn load(path: &Path) -> Result<AccountStore, AcctError> {
        if !path.exists() {
            return Ok(AccountStore {
                accounts: Vec::new(),
                selected: None,
                path: path.to_path_buf(),
            });
        }
        let text = std::fs::read_to_string(path)?;
        if text.trim().is_empty() {
            return Ok(AccountStore {
                accounts: Vec::new(),
                selected: None,
                path: path.to_path_buf(),
            });
        }
        Self::from_str(&text, path)
    }

    /// EN: parse store JSON (golden tests). RU: разобрать JSON хранилища.
    pub fn from_str(text: &str, path: &Path) -> Result<AccountStore, AcctError> {
        let j = parse(text).map_err(|e| AcctError::BadJson(format!("{}: {e}", path.display())))?;
        let mut accounts = Vec::new();
        if let Some(arr) = j.get("accounts").and_then(Json::as_arr) {
            for a in arr {
                accounts.push(Account::from_json(a)?);
            }
        }
        let selected = j.get("selected").and_then(Json::as_str).map(str::to_owned);
        Ok(AccountStore {
            accounts,
            selected,
            path: path.to_path_buf(),
        })
    }

    /// EN: add or replace an account (keyed by uuid) and select it.
    /// RU: добавить/заменить аккаунт (ключ — uuid) и выбрать его.
    pub fn add(&mut self, a: Account) {
        self.accounts.retain(|x| x.uuid != a.uuid);
        self.accounts.push(a.clone());
        self.selected = Some(a.uuid);
    }

    /// EN: persist to the store path (pretty JSON, parent dirs created).
    /// RU: сохранить по пути хранилища (pretty JSON, каталоги создаются).
    pub fn save(&self) -> Result<(), AcctError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, self.to_json().to_string_pretty())?;
        Ok(())
    }

    /// EN: serialize (tests). RU: сериализовать (для тестов).
    pub fn to_json(&self) -> Json {
        Json::Obj(vec![
            (
                "accounts".to_string(),
                Json::Arr(self.accounts.iter().map(Account::to_json).collect()),
            ),
            (
                "selected".to_string(),
                match &self.selected {
                    Some(s) => Json::s(s.clone()),
                    None => Json::Null,
                },
            ),
        ])
    }

    /// EN: resolve by name (case-insensitive) or UUID (dashed or undashed).
    /// RU: найти по имени (без регистра) или UUID (с дефисами и без).
    pub fn find(&self, reference: &str) -> Option<&Account> {
        let r = reference.to_lowercase();
        let r_undashed: String = r.chars().filter(|c| *c != '-').collect();
        self.accounts.iter().find(|a| {
            a.name.to_lowercase() == r
                || a.uuid == r_undashed
                || a.uuid.to_lowercase() == r_undashed
        })
    }

    /// EN: mutable variant of [`AccountStore::find`].
    /// RU: мутабельный вариант [`AccountStore::find`].
    pub fn find_mut(&mut self, reference: &str) -> Option<&mut Account> {
        let r = reference.to_lowercase();
        let r_undashed: String = r.chars().filter(|c| *c != '-').collect();
        self.accounts
            .iter_mut()
            .find(|a| a.name.to_lowercase() == r || a.uuid == r_undashed)
    }

    /// EN: remove an account by reference; returns the removed account.
    /// RU: удалить аккаунт по ссылке; возвращает удалённый аккаунт.
    pub fn remove(&mut self, reference: &str) -> Result<Account, AcctError> {
        let matches_ref =
            |a: &Account, r: &str, r_undashed: &str| {
                a.name.to_lowercase() == r || a.uuid == r_undashed
            };
        let r = reference.to_lowercase();
        let r_undashed: String = r.chars().filter(|c| *c != '-').collect();
        let idx = self
            .accounts
            .iter()
            .position(|a| matches_ref(a, &r, &r_undashed))
            .ok_or_else(|| AcctError::NotFound(reference.to_string()))?;
        let removed = self.accounts.remove(idx);
        if self.selected.as_deref() == Some(removed.uuid.as_str()) {
            self.selected = self.accounts.last().map(|a| a.uuid.clone());
        }
        Ok(removed)
    }

    /// EN: mark `reference` selected. RU: пометить `reference` выбранным.
    pub fn select(&mut self, reference: &str) -> Result<(), AcctError> {
        let uuid = self
            .find(reference)
            .map(|a| a.uuid.clone())
            .ok_or_else(|| AcctError::NotFound(reference.to_string()))?;
        self.selected = Some(uuid);
        Ok(())
    }

    /// EN: store path. RU: путь к хранилищу.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn acct(uuid: &str, name: &str) -> Account {
        Account {
            uuid: uuid.into(),
            name: name.into(),
            skin: Some("https://textures.minecraft.net/texture/abc".into()),
            refresh_token: "RT.1".into(),
            mc_token: "MC.1".into(),
            mc_expiry: 1_000_000_000,
            xuid: "2535401".into(),
            client_id: "cid-1".into(),
        }
    }

    fn tmp_path(tag: &str) -> PathBuf {
        let mut p = temp_dir();
        p.push(format!(
            "horizon-accounts-test-{}-{}.json",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        p
    }

    #[test]
    fn roundtrip_in_temp_dir() {
        let path = tmp_path("roundtrip");
        let mut st = AccountStore::load(&path).unwrap();
        assert!(st.accounts.is_empty());
        st.add(acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "Steve_Miner"));
        st.add(acct("b74f14e84a1f4b34a1c69f2b3e8b7e01", "Alex"));
        st.save().unwrap();

        let back = AccountStore::load(&path).unwrap();
        assert_eq!(back.accounts.len(), 2);
        assert_eq!(back.accounts[0].name, "Steve_Miner");
        assert_eq!(back.accounts[0].mc_expiry, 1_000_000_000);
        assert_eq!(back.accounts[1].xuid, "2535401");
        assert_eq!(back.selected.as_deref(), Some("b74f14e84a1f4b34a1c69f2b3e8b7e01"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn add_replaces_same_uuid() {
        let mut st = AccountStore::load(Path::new("NUL-does-not-exist.json")).unwrap();
        st.add(acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "Old"));
        st.add(acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "New"));
        assert_eq!(st.accounts.len(), 1);
        assert_eq!(st.accounts[0].name, "New");
    }

    #[test]
    fn find_by_name_uuid_and_dashed_uuid() {
        let mut st = AccountStore::load(Path::new("NUL-does-not-exist.json")).unwrap();
        st.add(acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "Steve_Miner"));
        assert!(st.find("steve_miner").is_some());
        assert!(st.find("A83F14E8-4A1F-4B34-A1C6-9F2B3E8B7E01").is_some());
        assert!(st.find("a83f14e84a1f4b34a1c69f2b3e8b7e01").is_some());
        assert!(st.find("Nobody").is_none());
    }

    #[test]
    fn remove_re_selects_last() {
        let mut st = AccountStore::load(Path::new("NUL-does-not-exist.json")).unwrap();
        st.add(acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "Steve_Miner"));
        st.add(acct("b74f14e84a1f4b34a1c69f2b3e8b7e01", "Alex"));
        let removed = st.remove("steve_miner").unwrap();
        assert_eq!(removed.name, "Steve_Miner");
        assert_eq!(st.selected.as_deref(), Some("b74f14e84a1f4b34a1c69f2b3e8b7e01"));
        assert!(st.remove("gone").is_err());
    }

    #[test]
    fn golden_store_json_parses() {
        // mirrors the shape horizon-account-switcher.py writes (secret
        // inlined here instead of DPAPI blob)
        let text = r#"{
  "accounts": [
    {
      "uuid": "a83f14e84a1f4b34a1c69f2b3e8b7e01",
      "name": "Steve_Miner",
      "skin": "https://textures.minecraft.net/texture/1a4af718",
      "refresh_token": "MC.MC.MC",
      "mc_token": "MC.A",
      "mc_expiry": 1770000000,
      "xuid": "2535401",
      "client_id": "azure-cid"
    }
  ],
  "selected": "a83f14e84a1f4b34a1c69f2b3e8b7e01"
}"#;
        let st = AccountStore::from_str(text, Path::new("golden.json")).unwrap();
        assert_eq!(st.accounts.len(), 1);
        assert_eq!(st.accounts[0].name, "Steve_Miner");
        assert_eq!(st.accounts[0].mc_expiry, 1770000000);
        assert_eq!(st.selected.as_deref(), Some("a83f14e84a1f4b34a1c69f2b3e8b7e01"));
    }

    #[test]
    fn token_freshness_margin() {
        // base expiry is 1_000_000_000 (see acct helper)
        let a = acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "S");
        assert!(a.mc_token_fresh(999_999_000)); // 1000s left, above the 600s margin
        assert!(!a.mc_token_fresh(999_999_500)); // 500s left, inside the margin: renew
        assert!(!a.mc_token_fresh(1_000_000_500)); // past expiry
        let mut b = acct("a83f14e84a1f4b34a1c69f2b3e8b7e01", "S");
        b.mc_token = String::new();
        assert!(!b.mc_token_fresh(0)); // empty token is never fresh
    }
}
