//! Compact, deduplicated disk representation of deferred model source bytes.
use super::{ModelSnapshot, ParsedModels};
use crate::compact_bytes::CompactBytes;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Serialize, Deserialize)]
pub(super) struct SnapshotCache {
    pool: Vec<Arc<CompactBytes>>,
    models: std::collections::BTreeMap<String, ModelRecord>,
    unreadable_count: usize,
    unreadable: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ModelRecord {
    json: usize,
    /// `None` records a failed read, which is part of the frozen resource view.
    files: std::collections::BTreeMap<(String, String), Option<usize>>,
    obj_error: Option<String>,
}

impl SnapshotCache {
    pub(super) fn capture(models: &ParsedModels) -> Option<Self> {
        let mut pool = Vec::new();
        let mut indices = HashMap::<Arc<CompactBytes>, usize>::new();
        let mut intern = |bytes: &Arc<CompactBytes>| {
            if let Some(index) = indices.get(bytes) {
                return Some(*index);
            }
            let index = pool.len();
            pool.push(bytes.clone());
            indices.insert(bytes.clone(), index);
            Some(index)
        };
        let mut records = std::collections::BTreeMap::new();
        for (name, snapshot) in &models.snapshots {
            let json = intern(&snapshot.json)?;
            let mut files = std::collections::BTreeMap::new();
            for (key, bytes) in &snapshot.files {
                let index = match bytes {
                    Some(bytes) => Some(intern(bytes)?),
                    None => None,
                };
                files.insert(key.clone(), index);
            }
            records.insert(
                name.clone(),
                ModelRecord {
                    json,
                    files,
                    obj_error: snapshot.obj_error.clone(),
                },
            );
        }
        pool.shrink_to_fit();
        let mut unreadable = models.unreadable.clone();
        unreadable.shrink_to_fit();
        Some(Self {
            pool,
            models: records,
            unreadable_count: models.unreadable_count,
            unreadable,
        })
    }

    pub(super) fn restore(self) -> Option<ParsedModels> {
        // CompactBytes validates each backing during deserialization; capture
        // also produces trusted backing. Re-inflating the pool here doubles
        // the warm-load work and its temporary allocations.
        if self.unreadable.len() > self.unreadable_count {
            return None;
        }
        let pool = self.pool;
        let mut snapshots = std::collections::BTreeMap::new();
        for (name, record) in self.models {
            let json = pool.get(record.json)?.clone();
            let files = record
                .files
                .into_iter()
                .map(|(key, index)| {
                    Some((
                        key,
                        match index {
                            Some(i) => Some(pool.get(i)?.clone()),
                            None => None,
                        },
                    ))
                })
                .collect::<Option<_>>()?;
            snapshots.insert(
                name,
                Arc::new(ModelSnapshot {
                    json,
                    files,
                    obj_error: record.obj_error,
                }),
            );
        }
        let mut unreadable = self.unreadable;
        unreadable.shrink_to_fit();
        Some(ParsedModels {
            snapshots,
            unreadable_count: self.unreadable_count,
            unreadable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn bytes(value: &str) -> Arc<CompactBytes> {
        Arc::new(CompactBytes::new(value.as_bytes().to_vec()))
    }

    #[test]
    fn snapshot_reconstruction_keeps_shared_pool_and_failed_reads() {
        let shared_json = bytes("same model json");
        let shared_obj = bytes("same obj");
        let mut files = BTreeMap::new();
        files.insert(
            ("m".into(), "models/a.obj".into()),
            Some(shared_obj.clone()),
        );
        files.insert(("m".into(), "models/missing.mtl".into()), None);
        let models = ParsedModels {
            snapshots: BTreeMap::from([
                (
                    "m:item/a".into(),
                    Arc::new(ModelSnapshot {
                        json: shared_json.clone(),
                        files: files.clone(),
                        obj_error: Some("invalid OBJ".into()),
                    }),
                ),
                (
                    "m:item/b".into(),
                    Arc::new(ModelSnapshot {
                        json: shared_json,
                        files,
                        obj_error: Some("invalid OBJ".into()),
                    }),
                ),
            ]),
            unreadable_count: 1,
            unreadable: vec!["m:broken".into()],
        };
        let restored = SnapshotCache::capture(&models).unwrap().restore().unwrap();
        assert!(Arc::ptr_eq(
            &restored.snapshots["m:item/a"].json,
            &restored.snapshots["m:item/b"].json,
        ));
        assert!(Arc::ptr_eq(
            restored.snapshots["m:item/a"].files[&("m".into(), "models/a.obj".into())]
                .as_ref()
                .unwrap(),
            restored.snapshots["m:item/b"].files[&("m".into(), "models/a.obj".into())]
                .as_ref()
                .unwrap(),
        ));
        assert!(
            restored.snapshots["m:item/a"].files[&("m".into(), "models/missing.mtl".into())]
                .is_none()
        );
        assert_eq!(restored.unreadable_count, 1);
        assert_eq!(restored.unreadable, vec!["m:broken".to_owned()]);
    }

    #[test]
    fn source_cache_hit_keeps_item_obj_reads_lazy_and_frozen() {
        let root = std::env::temp_dir().join(format!(
            "horizon-model-cache-roundtrip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("horizon-mod-content.stamp"), b"test stamp").unwrap();
        let write_model = |name: &str, text: &str| {
            let (namespace, path) = name.split_once(':').unwrap();
            let path = root.join(format!("assets/{namespace}/models/{path}.json"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write_model(
            "m:item/a",
            r#"{"loader":"neoforge:obj","model":"m:models/shared.obj","textures":{"all":"m:block/a"}}"#,
        );
        write_model(
            "m:item/b",
            r#"{"loader":"neoforge:obj","model":"m:models/shared.obj","textures":{"all":"m:block/a"}}"#,
        );
        let obj = root.join("assets/m/models/shared.obj");
        std::fs::create_dir_all(obj.parent().unwrap()).unwrap();
        std::fs::write(&obj, "v 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n").unwrap();

        let cold = ParsedModels::load(&root);
        assert!(
            cold.snapshots["m:item/a"]
                .files
                .contains_key(&("m".into(), "models/shared.obj".into()))
        );
        drop(cold);
        let warm = ParsedModels::load(&root);
        assert!(Arc::ptr_eq(
            &warm.snapshots["m:item/a"].json,
            &warm.snapshots["m:item/b"].json,
        ));
        assert!(Arc::ptr_eq(
            warm.snapshots["m:item/a"].files[&("m".into(), "models/shared.obj".into())]
                .as_ref()
                .unwrap(),
            warm.snapshots["m:item/b"].files[&("m".into(), "models/shared.obj".into())]
                .as_ref()
                .unwrap(),
        ));
        let items = warm.into_item_library().unwrap();
        let before = format!("{:?}", items.resolve("m:item/a").unwrap());
        std::fs::write(&obj, "invalid changed OBJ").unwrap();
        assert_eq!(format!("{:?}", items.resolve("m:item/a").unwrap()), before);
        let _ = std::fs::remove_dir_all(&root);
    }
}
