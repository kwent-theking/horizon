//! Languages (ClientLanguage / LanguageManager): `lang/<code>.json` from the
//! resource root and enabled packs layered over `en_us`, the language list
//! from `pack.mcmeta` `language` sections, and a process-wide current
//! language used by every screen.
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageInfo {
    pub code: String,
    pub name: String,
    pub region: String,
    pub bidirectional: bool,
}
impl LanguageInfo {
    /// LanguageInfo.toComponent: "Name (Region)".
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.region)
    }
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Language {
    pub code: String,
    /// `LanguageInfo.bidirectional`: text without a strong character reads
    /// right to left (`ClientLanguage.defaultRightToLeft`).
    bidirectional: bool,
    map: HashMap<String, String>,
}
pub const DEFAULT: &str = "en_us";

static CURRENT: RwLock<Option<Arc<Language>>> = RwLock::new(None);
/// The current language's `bidirectional`, read for every drawn string.
static BIDIRECTIONAL: AtomicBool = AtomicBool::new(false);

/// A source of resource files: the prepared root plus packs, highest priority last.
pub trait ResourceSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// A filesystem tree whose stamp covers all reads from this source.
    /// In-memory and other dynamic sources continue through the normal loader.
    fn cache_root(&self) -> Option<&std::path::Path> {
        None
    }
    /// Whether `path` is there, without reading it where the source can tell.
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }
    /// The namespaces under `assets/` (resource packs may add their own).
    fn namespaces(&self) -> Vec<String> {
        vec!["minecraft".into()]
    }
}
/// Horizon's own strings, compiled in so they need no prepared resources.
struct Bundled;
impl ResourceSource for Bundled {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let body = match path {
            "assets/horizon/lang/en_us.json" => {
                include_str!("../../horizon-client/assets/horizon/lang/en_us.json")
            }
            "assets/horizon/lang/ru_ru.json" => {
                include_str!("../../horizon-client/assets/horizon/lang/ru_ru.json")
            }
            "assets/horizon/lang/pl_pl.json" => {
                include_str!("../../horizon-client/assets/horizon/lang/pl_pl.json")
            }
            "assets/horizon/lang/uk_ua.json" => {
                include_str!("../../horizon-client/assets/horizon/lang/uk_ua.json")
            }
            _ => return None,
        };
        Some(body.as_bytes().to_vec())
    }
    fn namespaces(&self) -> Vec<String> {
        vec!["horizon".into()]
    }
}
impl ResourceSource for std::path::PathBuf {
    fn cache_root(&self) -> Option<&std::path::Path> {
        Some(self.as_path())
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.join(path)).ok()
    }
    fn exists(&self, path: &str) -> bool {
        self.join(path).is_file()
    }
    fn namespaces(&self) -> Vec<String> {
        std::fs::read_dir(self.join("assets"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect()
    }
}

/// DeprecatedTranslationsInfo.applyToMap: drop removed keys, then move each
/// renamed key's text to its new key (a missing old key removes the new one).
pub fn apply_deprecated(map: &mut HashMap<String, String>, json: &[u8]) {
    let Ok(info) = serde_json::from_slice::<serde_json::Value>(json) else {
        return;
    };
    for key in info["removed"].as_array().into_iter().flatten() {
        if let Some(k) = key.as_str() {
            map.remove(k);
        }
    }
    for (from, to) in info["renamed"].as_object().into_iter().flatten() {
        let Some(to) = to.as_str() else { continue };
        match map.remove(from) {
            Some(value) => {
                map.insert(to.to_owned(), value);
            }
            None => {
                map.remove(to);
            }
        }
    }
}

impl Language {
    /// ClientLanguage.loadFrom: en_us first, then the selected language, each
    /// from every namespace and every source in priority order (later sources
    /// override).
    pub fn load(sources: &[&dyn ResourceSource], code: &str) -> Self {
        let started = std::time::Instant::now();
        let cache = language_cache(sources, code);
        if let Some(language) = cache
            .as_ref()
            .and_then(|c| c.read_value::<Self>())
            .filter(|language| language.code == code)
        {
            log::info!(
                "Horizon: stage=language cache=hit elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return language;
        }
        let language = Self::load_uncached(sources, code);
        if let Some(cache) = cache
            .as_ref()
            .filter(|cache| Some(*cache) == language_cache(sources, code).as_ref())
        {
            let _ = cache.write_value(&language);
        }
        log::info!(
            "Horizon: stage=language cache=miss elapsed_ms={}",
            started.elapsed().as_millis()
        );
        language
    }

    fn load_uncached(sources: &[&dyn ResourceSource], code: &str) -> Self {
        let mut map = HashMap::new();
        let mut codes = vec![DEFAULT];
        if code != DEFAULT {
            codes.push(code);
        }
        // Horizon's own strings first, so a pack or the prepared root can still
        // override them. Vanilla has no key for "Controller vibration".
        let bundled = Bundled;
        let mut all: Vec<&dyn ResourceSource> = vec![&bundled];
        all.extend_from_slice(sources);
        let sources = all.as_slice();
        let namespaces: std::collections::BTreeSet<String> =
            sources.iter().flat_map(|s| s.namespaces()).collect();
        for c in codes {
            for namespace in &namespaces {
                for source in sources {
                    let Some(bytes) = source.read(&format!("assets/{namespace}/lang/{c}.json"))
                    else {
                        continue;
                    };
                    if let Ok(entries) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
                        map.extend(entries);
                    }
                }
            }
        }
        // DeprecatedTranslationsInfo.loadFromDefaultResource().applyToMap: the
        // game's own deprecated.json, not a resource pack's.
        if let Some(bytes) = sources
            .iter()
            .find_map(|s| s.read("assets/minecraft/lang/deprecated.json"))
        {
            apply_deprecated(&mut map, &bytes);
        }
        let bidirectional = available(sources)
            .iter()
            .any(|info| info.code == code && info.bidirectional);
        Self {
            code: code.to_owned(),
            bidirectional,
            map,
        }
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
    /// The resolved map, for `horizon_protocol::text::plain`: a server's
    /// components carry keys, and the network layer resolves them with this.
    pub fn map(&self) -> &HashMap<String, String> {
        &self.map
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn language_cache(
    sources: &[&dyn ResourceSource],
    code: &str,
) -> Option<horizon_world::resource_cache::Cache> {
    use horizon_world::resource_cache::Cache;
    let root = sources.first()?.cache_root()?;
    // Resource order is significant: later sources override earlier ones.
    let mut identity = Vec::new();
    identity.extend_from_slice(&(code.len() as u64).to_le_bytes());
    identity.extend_from_slice(code.as_bytes());
    identity.extend_from_slice(&(sources.len() as u64).to_le_bytes());
    for source in sources {
        let tree = Cache::for_tree(source.cache_root()?, "language-source-v1", &[])?;
        identity.extend_from_slice(tree.identity());
    }
    Cache::new(root, "language-v1", &[], &identity)
}

/// Languages declared by `pack.mcmeta` files (the vanilla asset root first).
pub fn available(sources: &[&dyn ResourceSource]) -> Vec<LanguageInfo> {
    let mut found: BTreeMap<String, LanguageInfo> = BTreeMap::new();
    found.insert(
        DEFAULT.into(),
        LanguageInfo {
            code: DEFAULT.into(),
            name: "English".into(),
            region: "US".into(),
            bidirectional: false,
        },
    );
    for source in sources {
        for path in ["assets/pack.mcmeta", "pack.mcmeta"] {
            let Some(bytes) = source.read(path) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            for (code, info) in value["language"].as_object().into_iter().flatten() {
                // Only languages whose file exists somewhere are selectable.
                let file = format!("assets/minecraft/lang/{code}.json");
                if code != DEFAULT && !sources.iter().any(|s| s.exists(&file)) {
                    continue;
                }
                found.insert(
                    code.clone(),
                    LanguageInfo {
                        code: code.clone(),
                        name: info["name"].as_str().unwrap_or(code).to_owned(),
                        region: info["region"].as_str().unwrap_or("").to_owned(),
                        bidirectional: info["bidirectional"].as_bool().unwrap_or(false),
                    },
                );
            }
        }
    }
    found.into_values().collect()
}

pub fn set(language: Language) {
    BIDIRECTIONAL.store(language.bidirectional, Ordering::Relaxed);
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(language));
}
pub fn current() -> Option<Arc<Language>> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}
/// `ClientLanguage.defaultRightToLeft` of the current language.
pub fn bidirectional() -> bool {
    BIDIRECTIONAL.load(Ordering::Relaxed)
}
pub fn code() -> String {
    current().map_or_else(|| DEFAULT.to_owned(), |l| l.code.clone())
}
/// Language.getOrDefault: the translation, or the key itself.
pub fn get(key: &str) -> String {
    current()
        .and_then(|l| l.get(key).map(str::to_owned))
        .unwrap_or_else(|| key.to_owned())
}
/// The translation, or `fallback` when the key is missing.
pub fn get_or(key: &str, fallback: &str) -> String {
    current()
        .and_then(|l| l.get(key).map(str::to_owned))
        .unwrap_or_else(|| fallback.to_owned())
}
pub fn has(key: &str) -> bool {
    current().is_some_and(|l| l.get(key).is_some())
}
/// I18n.get with plain string arguments.
pub fn tr(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    crate::text::Text::tr(key, args).plain()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_preserves_language_layering_and_falls_back_after_changes_or_corruption() {
        use std::fs;
        let root =
            std::env::temp_dir().join(format!("horizon-language-snapshot-{}", std::process::id()));
        let base = root.join("base");
        let pack = root.join("pack");
        for dir in [&base, &pack] {
            fs::create_dir_all(dir.join("assets/minecraft/lang")).unwrap();
            fs::write(dir.join("horizon-mod-content.stamp"), b"content").unwrap();
        }
        let file = "assets/minecraft/lang/en_us.json";
        fs::write(base.join(file), br#"{"a":"base","fallback":"English"}"#).unwrap();
        fs::write(pack.join(file), br#"{"a":"pack"}"#).unwrap();
        fs::write(
            pack.join("assets/minecraft/lang/fr_fr.json"),
            br#"{"a":"French"}"#,
        )
        .unwrap();
        let sources: [&dyn ResourceSource; 2] = [&base, &pack];
        for code in ["en_us", "fr_fr"] {
            let cold = Language::load(&sources, code);
            let cache = language_cache(&sources, code).unwrap();
            assert!(cache.read_value::<Language>().is_some());
            let warm = Language::load(&sources, code);
            assert_eq!(cold.map(), warm.map());
            assert_eq!(warm.get("fallback"), Some("English"));
            assert_eq!(
                warm.get("a"),
                Some(if code == "en_us" { "pack" } else { "French" })
            );
            cache.write(b"corrupt typed snapshot").unwrap();
            assert_eq!(Language::load(&sources, code).map(), cold.map());
        }
        fs::write(pack.join(file), br#"{"a":"updated pack"}"#).unwrap();
        assert_eq!(
            Language::load(&sources, "en_us").get("a"),
            Some("updated pack")
        );
        let reversed: [&dyn ResourceSource; 2] = [&pack, &base];
        assert_eq!(Language::load(&reversed, "en_us").get("a"), Some("base"));
        fs::write(base.join("horizon-mod-content.stamp"), b"new content").unwrap();
        assert!(language_cache(&sources, "en_us").unwrap().read().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    struct Mem(HashMap<String, Vec<u8>>);
    impl ResourceSource for Mem {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
    }
    #[test]
    fn bundled_horizon_strings_follow_the_language() {
        // No prepared resources at all: the namespace is compiled in.
        let empty = Mem(HashMap::new());
        let en = Language::load(&[&empty], "en_us");
        assert_eq!(en.get("horizon.options.controller"), Some("Controller"));
        let ru = Language::load(&[&empty], "ru_ru");
        assert_eq!(ru.get("horizon.options.controller"), Some("Геймпад"));
        // Every bundled string carries both languages: a key added to one file
        // and forgotten in the other shows as a key name on screen.
        for key in en.map().keys() {
            assert!(ru.get(key).is_some(), "{key} has no ru_ru value");
        }
        assert_eq!(en.get("horizon.pages.mods"), Some("Mods"));
        assert_eq!(ru.get("horizon.pages.mods"), Some("Моды"));
        let pl = Language::load(&[&empty], "pl_pl");
        assert_eq!(pl.get("horizon.options.controller"), Some("Kontroler"));
        assert_eq!(pl.get("horizon.options.vibration"), Some("Wibracje"));
        let uk = Language::load(&[&empty], "uk_ua");
        assert_eq!(
            uk.get("horizon.options.on_screen_keyboard"),
            Some("Екранна клавіатура")
        );
    }

    #[test]
    fn layers_selected_language_over_english() {
        let base = Mem(HashMap::from([
            ("assets/minecraft/lang/en_us.json".into(), br#"{"a":"Apple","b":"Banana"}"#.to_vec()),
            ("assets/minecraft/lang/fr_fr.json".into(), br#"{"a":"Pomme"}"#.to_vec()),
            ("assets/pack.mcmeta".into(), r#"{"language":{"fr_fr":{"name":"Français","region":"France"},"de_de":{"name":"Deutsch","region":"Deutschland"}}}"#.as_bytes().to_vec()),
        ]));
        let pack = Mem(HashMap::from([(
            "assets/minecraft/lang/fr_fr.json".into(),
            br#"{"b":"Banane"}"#.to_vec(),
        )]));
        let sources: [&dyn ResourceSource; 2] = [&base, &pack];
        let fr = Language::load(&sources, "fr_fr");
        assert_eq!(fr.get("a"), Some("Pomme"));
        assert_eq!(fr.get("b"), Some("Banane"));
        let langs = available(&sources);
        // de_de has no file, so only English and French are offered.
        assert_eq!(
            langs.iter().map(|l| l.code.as_str()).collect::<Vec<_>>(),
            ["en_us", "fr_fr"]
        );
        assert_eq!(langs[1].label(), "Français (France)");
    }

    #[test]
    fn languages_are_listed_without_reading_their_files() {
        struct NoLangReads;
        impl ResourceSource for NoLangReads {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                assert!(!path.contains("/lang/"), "read {path}");
                (path == "assets/pack.mcmeta").then(|| {
                    br#"{"language":{"fr_fr":{"name":"French"},"de_de":{"name":"German"}}}"#
                        .to_vec()
                })
            }
            fn exists(&self, path: &str) -> bool {
                path == "assets/minecraft/lang/fr_fr.json"
            }
        }
        let langs = available(&[&NoLangReads]);
        assert_eq!(
            langs.iter().map(|l| l.code.as_str()).collect::<Vec<_>>(),
            ["en_us", "fr_fr"]
        );
    }
}
