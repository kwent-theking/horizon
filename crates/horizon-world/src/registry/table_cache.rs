//! The checked table without its redundant state descriptors. The registry
//! identity in the key is checked once per registry, rather than parsing the
//! same hundreds of thousands of NBT descriptors in every table on every start.
use super::*;
use crate::{nbt, resource_cache::Cache};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn cache(path: &Path, registry: &BlockRegistry) -> Option<Cache> {
    let root = path.parent()?;
    if !root.join("horizon-mod-content.stamp").is_file() {
        return None;
    }
    Cache::new(
        root,
        &format!("{}-checked-v1", path.file_name()?.to_str()?),
        &[path],
        registry.cache_identity(),
    )
}

pub(super) fn identity(registry: &BlockRegistry) -> &[u8; 32] {
    registry.cache_identity.get_or_init(|| {
        let mut hash = Sha256::new();
        fn field(hash: &mut Sha256, text: &str) {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        for state in &registry.states {
            field(&mut hash, &state.name);
            hash.update((state.properties.len() as u64).to_le_bytes());
            for (key, value) in &state.properties {
                field(&mut hash, key);
                field(&mut hash, value);
            }
        }
        hash.finalize().into()
    })
}

pub(super) fn read(cache: &Cache) -> Option<Tag> {
    let bytes = cache.read()?;
    let (tag, used) = nbt::read_network(&bytes, nbt::Limits::tables()).ok()?;
    (used == bytes.len()).then_some(tag)
}

pub(super) fn write(cache: &Cache, tag: &Tag) {
    if let Ok(bytes) = nbt::write_network(tag, nbt::Limits::tables()) {
        let _ = cache.write(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn checked_table_round_trip_rejects_a_different_registry_and_changed_source() {
        let root =
            std::env::temp_dir().join(format!("horizon-checked-table-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("horizon-mod-content.stamp"), "fixture").unwrap();
        let registry = |name: &str| {
            BlockRegistry::from_states(
                vec![BlockState {
                    name: name.into(),
                    properties: BTreeMap::new(),
                }],
                BTreeMap::from([(name.into(), 0)]),
            )
            .unwrap()
        };
        let a = registry("minecraft:air");
        let table = Tag::compound([
            (
                "states",
                Tag::List(vec![a.state(BlockStateId(0)).unwrap().to_nbt()]),
            ),
            ("rows", Tag::IntArray(vec![42])),
        ]);
        let path = root.join("test.nbt");
        let bytes = nbt::write_named(
            &nbt::NamedTag {
                name: "".into(),
                tag: table,
            },
            nbt::Limits::tables(),
        )
        .unwrap();
        fs::write(&path, &bytes).unwrap();
        let cold = BlockTableFile::open(&path, &a).unwrap();
        let cache = cache(&path, &a).unwrap();
        assert!(cache.read().is_some());
        let warm = BlockTableFile::open(&path, &a).unwrap();
        assert_eq!(cold.tag, warm.tag);
        assert_eq!(warm.tag.get("states"), Some(&Tag::List(Vec::new())));
        assert_eq!(
            warm.table("test").unwrap().get("rows"),
            Some(&Tag::IntArray(vec![42]))
        );
        // An intact envelope can still hold invalid or trailing NBT. Neither
        // may prevent the authoritative source from loading and repairing it.
        cache.write(b"invalid NBT").unwrap();
        assert!(read(&cache).is_none());
        assert_eq!(BlockTableFile::open(&path, &a).unwrap().tag, cold.tag);
        assert_eq!(read(&cache), Some(cold.tag.clone()));
        let mut trailing = nbt::write_network(&cold.tag, nbt::Limits::tables()).unwrap();
        trailing.push(0);
        cache.write(&trailing).unwrap();
        assert!(read(&cache).is_none());
        assert_eq!(BlockTableFile::open(&path, &a).unwrap().tag, cold.tag);

        let b = registry("minecraft:stone");
        assert!(BlockTableFile::open(&path, &b).is_err());
        fs::write(&path, b"changed invalid source").unwrap();
        assert!(BlockTableFile::open(&path, &a).is_err());

        // A cache directory that cannot be created must not turn a valid
        // source table into an error (also works without OS permission tricks).
        fs::write(&path, &bytes).unwrap();
        let derived = root.join(".horizon-derived");
        fs::remove_dir_all(&derived).unwrap();
        fs::write(&derived, b"not a directory").unwrap();
        assert_eq!(BlockTableFile::open(&path, &a).unwrap().tag, cold.tag);
        fs::remove_dir_all(root).unwrap();
    }
}
