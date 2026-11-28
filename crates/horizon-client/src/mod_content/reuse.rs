//! Check packaged mods by metadata before opening their central directories or
//! parsing declarations. Directory mods deliberately use the ordinary validator.
use super::*;
use horizon_mods::{Capability, Content, Package};
use horizon_world::resource_cache::Cache;

fn fast_base_and_server_meta(base: &Path, server_table: Option<&Path>) -> Option<Vec<u8>> {
    let mut out = fs::read(base.join("horizon-resources.json")).ok()?;
    // Each pack selection's tables are byte-identical copies of vanilla's.
    out.extend_from_slice(b"base\0");
    out.extend_from_slice(base_identity(base).as_bytes());
    out.push(0);
    let meta_into = |out: &mut Vec<u8>, label: &str, path: &Path| -> Option<()> {
        let m = fs::metadata(path).ok()?;
        let modified = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        out.extend_from_slice(label.as_bytes());
        out.push(0);
        out.extend_from_slice(&m.len().to_le_bytes());
        out.extend_from_slice(&modified.to_le_bytes());
        Some(())
    };
    for name in JSON_TABLES.iter().chain(&NBT_TABLES).chain(&TAG_BUNDLES) {
        let path = base.join(name);
        if path.exists() {
            meta_into(&mut out, name, &path)?;
        }
    }
    if let Some(table) = server_table.filter(|t| t.is_file()) {
        meta_into(&mut out, "server-states", table)?;
    }
    Some(out)
}

fn input_key(
    base: &Path,
    paths: &[PathBuf],
    enabled: &BTreeSet<String>,
    dev: &[PathBuf],
) -> Option<Vec<u8>> {
    let mut key = serde_json::to_vec(&(enabled, dev)).ok()?;
    key.extend_from_slice(&fast_base_and_server_meta(
        base,
        server::table().as_deref(),
    )?);
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
    // Warm validation must also miss when the builder would change the root.
    use sha2::{Digest, Sha256};
    key.extend_from_slice(&Sha256::digest(include_bytes!("../mod_content.rs")));
    key.extend_from_slice(&Sha256::digest(include_bytes!("../mod_content_server.rs")));
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
    let cache_validated = |name: &str, identity: &[u8]| {
        Cache::new(&out, name, &[], identity)
            .is_some_and(|cache| cache.read().as_deref() == Some(b"validated".as_slice()))
    };
    if root_has_base(&out)
        && key.as_deref().is_some_and(|key| {
            cache_validated("installed-mods-reuse-v1", key)
                || cache_validated("installed-mods-v1", key)
        })
    {
        // The installed mods are the archives its packaged files are served from.
        match mount_saved(&out, paths) {
            Ok(()) => {
                let warnings = fs::read_to_string(out.join(WARNINGS)).unwrap_or_default();
                warnings.lines().for_each(|w| log::warn!("Horizon: {w}"));
                return Ok(out);
            }
            Err(e) => log::warn!("Horizon: mod content checked again: {e}"),
        }
    }
    let mut candidates: Vec<(&PathBuf, Package)> = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut reusable = true;
    for path in paths {
        let Ok(package) = Package::read(path) else {
            reusable = false;
            continue;
        };
        let m = &package.manifest;
        if !(enabled.contains(&m.id) || dev.contains(path))
            || !m.capabilities.contains(&Capability::Registries)
            || !seen_ids.insert(m.id.clone())
        {
            continue;
        }
        candidates.push((path, package));
    }
    let loaded = horizon_core::par::par_map_with(
        horizon_core::par::threads(),
        &candidates,
        |(path, package)| Content::read(path, &package.manifest),
    );
    let mut mods: Vec<ContentMod> = Vec::new();
    for ((_, package), content) in candidates.into_iter().zip(loaded) {
        let id = package.manifest.id;
        let content = content.and_then(|content| {
            let own = (id.as_str(), content.namespaces.as_slice());
            match mods
                .iter()
                .find_map(|c| horizon_mods::content::clash((&c.id, &c.content.namespaces), own))
            {
                Some(e) => Err(e),
                None => Ok(content),
            }
        });
        match content {
            Ok(content) => mods.push(ContentMod { id, content }),
            Err(e) => {
                reusable = false;
                log::warn!("Horizon: mod {} content not loaded: {e}", id);
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
    {
        if let Some(cache) = Cache::new(&out, "installed-mods-reuse-v1", &[], key) {
            let _ = cache.write(b"validated");
        }
        // Keep `installed-mods-v1` portable (keyed by the content stamp rather
        // than machine paths/mtimes) and preserve an already-valid prebuilt copy.
        if let Ok(stamp_bytes) = fs::read(out.join(STAMP))
            && let Some(cache) = Cache::new(&out, "installed-mods-v1", &[], &stamp_bytes)
            && cache.read().as_deref() != Some(b"validated".as_slice())
        {
            let _ = cache.write(b"validated");
        }
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
        // A built root holds its base's files (linked in by the overlay).
        fs::write(out.join("horizon-resources.json"), b"{}").unwrap();
        // And the index of the files it serves from the mods' archives.
        Index::default().save(&out).unwrap();
        let cache = Cache::new(&out, "installed-mods-v1", &[], &key).unwrap();
        cache.write(b"validated").unwrap();
        assert_eq!(
            installed_resource_root(&dir, &paths, &enabled, &[], &dir).unwrap(),
            out
        );
        // An index naming an archive that is not installed serves nothing.
        let gone = dir.join("gone.hmod");
        fs::write(&gone, b"gone").unwrap();
        let mut stale = Index::default();
        stale.add_archive(&gone).unwrap();
        stale.save(&out).unwrap();
        assert_ne!(
            installed_resource_root(&dir, &paths, &enabled, &[], &dir).unwrap(),
            out
        );
        Index::default().save(&out).unwrap();
        // A copied root with the stamp and blessing but without the base is not reused.
        fs::remove_file(out.join("horizon-resources.json")).unwrap();
        assert_ne!(
            installed_resource_root(&dir, &paths, &enabled, &[], &dir).unwrap(),
            out
        );
        fs::write(out.join("horizon-resources.json"), b"{}").unwrap();
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

    #[test]
    fn portable_stamp_ignores_base_and_table_locations_and_overlay_preserves_derived() {
        let dir =
            std::env::temp_dir().join(format!("horizon-portable-stamp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let base1 = dir.join("base1");
        let base2 = dir.join("base2");
        let build_base = |base: &Path| {
            fs::create_dir_all(base.join("worldgen")).unwrap();
            fs::write(base.join("horizon-resources.json"), b"{}").unwrap();
            fs::write(base.join("facts.json"), b"{\"blocks\":[]}").unwrap();
            fs::write(base.join("server.tsv"), b"id\tstate\n").unwrap();
            let file = fs::File::options()
                .write(true)
                .open(base.join("facts.json"))
                .unwrap();
            let _ =
                file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(7));
        };
        build_base(&base1);
        build_base(&base2);

        let s1 = stamp(&base1, &[], Some(&base1.join("server.tsv"))).unwrap();
        let s2 = stamp(&base2, &[], Some(&base2.join("server.tsv"))).unwrap();
        assert_eq!(s1, s2);

        fs::write(base2.join("server.tsv"), b"id\tstate\n1\ta\n").unwrap();
        let s3 = stamp(&base2, &[], Some(&base2.join("server.tsv"))).unwrap();
        assert_ne!(s1, s3);

        // Overlay::new wipes stale root files/symlinks while preserving `.horizon-derived`.
        let out = dir.join("out");
        fs::create_dir_all(out.join(".horizon-derived")).unwrap();
        fs::write(out.join(".horizon-derived/prebaked-v1"), b"keep-me").unwrap();
        fs::write(out.join(".horizon-derived/installed-mods-v1"), b"stale").unwrap();
        fs::write(out.join(".horizon-derived/installed-mods-reuse-v1"), b"stale").unwrap();
        fs::write(out.join("stale.txt"), b"remove-me").unwrap();
        fs::create_dir_all(base1.join(".horizon-derived")).unwrap();
        fs::write(base1.join(".horizon-derived/base-only"), b"ignore").unwrap();

        let _overlay = Overlay::new(&base1, &out).unwrap();
        assert!(!out.join("stale.txt").exists());
        assert_eq!(
            fs::read(out.join(".horizon-derived/prebaked-v1")).unwrap(),
            b"keep-me"
        );
        assert!(!out.join(".horizon-derived/base-only").exists());
        // A rebuilt root is not blessed until it is validated again.
        assert!(!out.join(".horizon-derived/installed-mods-v1").exists());
        assert!(!out.join(".horizon-derived/installed-mods-reuse-v1").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    /// `packs::resource_root` gives each pack selection its own overlay of
    /// byte-identical vanilla tables: another selection must rebuild, while the
    /// same selection in another install still reuses.
    #[test]
    fn pack_selection_changes_stamp_and_reuse_key_but_not_its_location() {
        let dir =
            std::env::temp_dir().join(format!("horizon-pack-selection-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let selections = |install: &str| dir.join(install).join(".horizon-cache/resourcepacks");
        // Same bytes and times everywhere: only the selection tells them apart.
        let built = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let build_base = |base: &Path| {
            fs::create_dir_all(base).unwrap();
            fs::write(base.join("horizon-resources.json"), b"{}").unwrap();
            fs::write(base.join("facts.json"), b"{\"blocks\":[]}").unwrap();
            fs::File::options()
                .write(true)
                .open(base.join("facts.json"))
                .unwrap()
                .set_modified(built)
                .unwrap();
        };
        let vanilla = dir.join("install-a/resources");
        let pack_a = selections("install-a").join("0123456789abcdef");
        let pack_a_elsewhere = selections("install-b").join("0123456789abcdef");
        let pack_b = selections("install-a").join("fedcba9876543210");
        for base in [&vanilla, &pack_a, &pack_a_elsewhere, &pack_b] {
            build_base(base);
        }
        assert_eq!(base_identity(&vanilla), "vanilla");
        assert_eq!(base_identity(&pack_a), "selection 0123456789abcdef");
        // A folder named like a hash outside the selections is not one.
        assert_eq!(base_identity(&dir.join("0123456789abcdef")), "vanilla");

        let stamp_of = |base: &Path| stamp(base, &[], None).unwrap();
        let key_of = |base: &Path| input_key(base, &[], &BTreeSet::new(), &[]).unwrap();
        assert_eq!(stamp_of(&pack_a), stamp_of(&pack_a_elsewhere));
        assert_eq!(key_of(&pack_a), key_of(&pack_a_elsewhere));
        for other in [&vanilla, &pack_b] {
            assert_ne!(stamp_of(&pack_a), stamp_of(other));
            assert_ne!(key_of(&pack_a), key_of(other));
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
