//! Check packaged mods by metadata before opening their central directories or
//! parsing declarations. Directory mods deliberately use the ordinary validator.
use super::*;
use horizon_mods::{Capability, Content, Package};
use horizon_world::resource_cache::Cache;

fn input_key(
    base: &Path,
    paths: &[PathBuf],
    enabled: &BTreeSet<String>,
    dev: &[PathBuf],
) -> Option<Vec<u8>> {
    let mut key = serde_json::to_vec(&(enabled, dev)).ok()?;
    key.extend_from_slice(
        stamp(base, &[], server::table().as_deref())
            .ok()?
            .as_bytes(),
    );
    for path in paths {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        key.extend_from_slice(&serde_json::to_vec(&fs::canonicalize(path).ok()?).ok()?);
        key.extend_from_slice(&meta.len().to_le_bytes());
        key.extend_from_slice(
            &meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
                .to_le_bytes(),
        );
    }
    // Keep validation changes independent of the content builder's stamp.
    key.extend_from_slice(include_bytes!("reuse.rs"));
    Some(key)
}

/// The installed content root, with a cheap warm check for packaged mods.
/// The key covers all candidates (including refused/disabled mods), their order,
/// selection, base tables and server states. The existing content stamp remains
/// authoritative and participates in the cache key.
pub fn installed_resource_root(
    base: &Path,
    paths: &[PathBuf],
    enabled: &BTreeSet<String>,
    dev: &[PathBuf],
    cache_dir: &Path,
) -> Result<PathBuf> {
    let out = cache_dir.join("mod-content");
    let key = input_key(base, paths, enabled, dev);
    if let Some(cache) = key
        .as_deref()
        .and_then(|key| Cache::new(&out, "installed-mods-v1", &[], key))
        && cache.read().as_deref() == Some(b"validated".as_slice())
    {
        let warnings = fs::read_to_string(out.join(WARNINGS)).unwrap_or_default();
        warnings.lines().for_each(|w| log::warn!("Horizon: {w}"));
        return Ok(out);
    }
    let mut mods: Vec<ContentMod> = Vec::new();
    let mut reusable = true;
    for path in paths {
        let Ok(package) = Package::read(path) else {
            reusable = false;
            continue;
        };
        let m = &package.manifest;
        if !(enabled.contains(&m.id) || dev.contains(path))
            || !m.capabilities.contains(&Capability::Registries)
            || mods.iter().any(|c| c.id == m.id)
        {
            continue;
        }
        let content = Content::read(path, m).and_then(|content| {
            let own = (m.id.as_str(), content.namespaces.as_slice());
            match mods
                .iter()
                .find_map(|c| horizon_mods::content::clash((&c.id, &c.content.namespaces), own))
            {
                Some(e) => Err(e),
                None => Ok(content),
            }
        });
        match content {
            Ok(content) => mods.push(ContentMod {
                id: m.id.clone(),
                content,
            }),
            Err(e) => {
                reusable = false;
                log::warn!("Horizon: mod {} content not loaded: {e}", m.id);
            }
        }
    }
    let root = resource_root(base, &mods, cache_dir)?;
    // Don't bless an input modified during validation. No-content results keep
    // returning the base through the ordinary path.
    if reusable
        && root == out
        && key == input_key(base, paths, enabled, dev)
        && let Some(key) = key.as_deref()
        && let Some(cache) = Cache::new(&out, "installed-mods-v1", &[], key)
    {
        let _ = cache.write(b"validated");
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_manifest_changes_content_stamp() {
        let root =
            std::env::temp_dir().join(format!("horizon-mod-manifest-stamp-{}", std::process::id()));
        let dir = root.join("ruby");
        fs::create_dir_all(&dir).unwrap();
        fs::write(root.join("horizon-resources.json"), "{}").unwrap();
        let text = "id = \"ruby\"\nname = \"Ruby\"\nversion = \"0.1.0\"\napi = \"0.2\"\ncapabilities = [\"registries\"]\n";
        let manifest = dir.join(horizon_mods::manifest::MANIFEST);
        fs::write(&manifest, text).unwrap();
        let parsed = horizon_mods::Manifest::parse(text).unwrap();
        let mods = [ContentMod {
            id: "ruby".into(),
            content: Content::read(&dir, &parsed).unwrap(),
        }];
        let before = stamp(&root, &mods, None).unwrap();
        fs::write(&manifest, format!("{text}namespaces = [\"extra\"]\n")).unwrap();
        assert_ne!(before, stamp(&root, &mods, None).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_inputs_include_selection_order_and_changes() {
        let dir =
            std::env::temp_dir().join(format!("horizon-installed-mod-key-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("horizon-resources.json"), b"{}").unwrap();
        let a = dir.join("a.hmod");
        let b = dir.join("b.hmod");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let paths = [a.clone(), b.clone()];
        let enabled = BTreeSet::from(["a".to_owned()]);
        let key = input_key(&dir, &paths, &enabled, &[]).unwrap();
        assert_ne!(
            Some(key.clone()),
            input_key(&dir, &paths, &BTreeSet::new(), &[])
        );
        assert_ne!(
            Some(key.clone()),
            input_key(&dir, &[b, a.clone()], &enabled, &[])
        );
        assert_ne!(
            Some(key.clone()),
            input_key(&dir, &paths, &enabled, std::slice::from_ref(&a))
        );
        let out = dir.join("mod-content");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join(STAMP), "built root").unwrap();
        let cache = Cache::new(&out, "installed-mods-v1", &[], &key).unwrap();
        cache.write(b"validated").unwrap();
        assert_eq!(
            installed_resource_root(&dir, &paths, &enabled, &[], &dir).unwrap(),
            out
        );
        fs::write(out.join(STAMP), "a rebuilt root").unwrap();
        assert_eq!(
            installed_resource_root(&dir, &paths, &enabled, &[], &dir).unwrap(),
            dir
        );
        assert_eq!(
            installed_resource_root(&dir, &paths, &BTreeSet::new(), &[], &dir).unwrap(),
            dir
        );
        fs::write(&a, b"changed size").unwrap();
        assert_ne!(Some(key), input_key(&dir, &paths, &enabled, &[]));
        assert!(input_key(&dir, std::slice::from_ref(&dir), &enabled, &[]).is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
