//! Parsed resource models for one startup or resource-pack reload.
use crate::Result;
use horizon_assets::{
    model::CuboidModel,
    resolve::{ModelLibrary, ModelSource, cube_all_model},
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

/// Models made up for a blockstate whose own model is gone: a cube of one
/// sprite, named after it (`horizon:loader_cube/<namespace>/<sprite path>`).
const LOADER_CUBE: &str = "horizon:loader_cube/";

/// A model JSON, with the OBJ file of a `neoforge:obj` model read beside it.
/// An OBJ that cannot be read or parsed is logged once and the model keeps
/// its loader fallback.
fn decode_model(
    root: &Path,
    id: &str,
    text: &str,
) -> std::result::Result<CuboidModel, horizon_assets::DecodeError> {
    let mut model = CuboidModel::from_json(text)?;
    let read = |namespace: &str, path: &str| {
        fs::read_to_string(root.join(format!("assets/{namespace}/{path}"))).ok()
    };
    if let Err(e) = model.attach_obj(&read) {
        log::warn!("Horizon: OBJ model {id} not loaded: {e}");
    }
    Ok(model)
}

#[derive(Default)]
pub struct ParsedModels {
    snapshots: BTreeMap<String, Arc<ModelSnapshot>>,
}
/// JSON and every OBJ/MTL read captured at discovery, including failed reads.
/// A late item lookup must see the same pack even if its files have changed.
struct ModelSnapshot {
    json: Arc<crate::compact_bytes::CompactBytes>,
    files: BTreeMap<(String, String), Option<Arc<crate::compact_bytes::CompactBytes>>>,
}
impl ModelSnapshot {
    fn capture(root: &Path, id: &str, text: String) -> Option<Self> {
        let mut model = CuboidModel::from_json(&text).ok()?;
        let files = std::sync::Mutex::new(BTreeMap::new());
        let read = |namespace: &str, path: &str| {
            let text = fs::read_to_string(root.join(format!("assets/{namespace}/{path}"))).ok();
            files.lock().unwrap().insert(
                (namespace.to_owned(), path.to_owned()),
                text.as_ref().map(|s| {
                    Arc::new(crate::compact_bytes::CompactBytes::new(
                        s.as_bytes().to_vec(),
                    ))
                }),
            );
            text
        };
        if let Err(e) = model.attach_obj(&read) {
            log::warn!("Horizon: OBJ model {id} not loaded: {e}");
        }
        // Validation and OBJ diagnostics stay eager, but the expanded tree has
        // no lifetime beyond this worker. Blocks decode only their used roots.
        Some(Self {
            json: Arc::new(crate::compact_bytes::CompactBytes::new(text.into_bytes())),
            files: files.into_inner().unwrap(),
        })
    }
}
impl ModelSource for ModelSnapshot {
    fn decode(&self) -> Arc<CuboidModel> {
        let json = self.json.decode();
        let mut model = CuboidModel::from_json(std::str::from_utf8(&json).unwrap())
            .expect("validated model snapshot");
        let read = |namespace: &str, path: &str| {
            self.files
                .get(&(namespace.to_owned(), path.to_owned()))?
                .as_ref()
                .map(|bytes| String::from_utf8(bytes.decode().into_owned()).unwrap())
        };
        // Failure was already logged at discovery; preserve the loader fallback.
        let _ = model.attach_obj(&read);
        Arc::new(model)
    }
}
impl ParsedModels {
    /// Every `models/**.json` of a namespace directory, by identifier.
    pub fn load(root: &Path) -> Self {
        let Ok(namespaces) = fs::read_dir(root.join("assets")) else {
            return Self::default();
        };
        // Listed first, then read and parsed at once: a content pack has tens
        // of thousands of models.
        let mut files = Vec::new();
        for ns in namespaces.flatten() {
            let namespace = ns.file_name().to_string_lossy().into_owned();
            let base = ns.path().join("models");
            let mut stack = vec![base.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = fs::read_dir(&dir) else {
                    continue;
                };
                for e in entries.flatten() {
                    let path = e.path();
                    // The entry's own type saves a stat per file; a link
                    // (content roots link into the base) is followed.
                    let is_dir = match e.file_type() {
                        Ok(t) if !t.is_symlink() => t.is_dir(),
                        _ => path.is_dir(),
                    };
                    if is_dir {
                        stack.push(path);
                    } else if path.extension().is_some_and(|x| x == "json")
                        && let Ok(rel) = path.strip_prefix(&base)
                    {
                        let rel = rel.with_extension("").to_string_lossy().replace('\\', "/");
                        files.push((format!("{namespace}:{rel}"), path));
                    }
                }
            }
        }
        let mut snapshots = BTreeMap::new();
        let mut backing = HashSet::<Arc<crate::compact_bytes::CompactBytes>>::new();
        let mut unreadable = Vec::new();
        let mut unreadable_count = 0usize;
        crate::par::map_batched(
            "parse_models",
            crate::par::threads(),
            files,
            128,
            |(id, path)| {
                let model = fs::read_to_string(path)
                    .ok()
                    .and_then(|text| ModelSnapshot::capture(root, id, text));
                (id.clone(), model)
            },
            |(id, model)| {
                if let Some(mut snapshot) = model {
                    // Many model identifiers reference the same OBJ/MTL, or
                    // repeat whole JSON files. Keep one exact byte snapshot.
                    for bytes in std::iter::once(&mut snapshot.json)
                        .chain(snapshot.files.values_mut().flatten())
                    {
                        if let Some(shared) = backing.get(&*bytes) {
                            *bytes = shared.clone();
                        } else {
                            backing.insert(bytes.clone());
                        }
                    }
                    snapshots.insert(id, Arc::new(snapshot));
                } else {
                    unreadable_count += 1;
                    if unreadable.len() < 5 {
                        unreadable.push(id);
                    }
                }
            },
        );
        if !unreadable.is_empty() {
            log::warn!(
                "Horizon: {} model files could not be read and are skipped (first: {})",
                unreadable_count,
                unreadable.join(", ")
            );
        }
        let source_bytes: usize = snapshots
            .values()
            .map(|s| s.json.len() + s.files.values().flatten().map(|b| b.len()).sum::<usize>())
            .sum();
        let packed_bytes: usize = backing.iter().map(|b| b.stored_len()).sum();
        log::info!(
            "load assets: {} model snapshots, {} -> {} source bytes",
            snapshots.len(),
            source_bytes,
            packed_bytes
        );
        Self { snapshots }
    }

    /// The made-up cube model of a sprite, for [`Self::chain_loads`] misses.
    pub fn loader_cube_name(sprite: &str) -> String {
        format!("{LOADER_CUBE}{}", sprite.replace(':', "/"))
    }
    fn loader_cube(name: &str) -> Option<CuboidModel> {
        let (namespace, path) = name.strip_prefix(LOADER_CUBE)?.split_once('/')?;
        Some(cube_all_model(&format!("{namespace}:{path}")))
    }
    /// Whether `name` and every parent of it read.
    pub fn chain_loads(&self, root: &Path, name: &str) -> bool {
        let mut name = name.to_owned();
        for _ in 0..16 {
            let parent = match self.snapshots.get(&name) {
                Some(model) => model.decode().parent.clone(),
                None => {
                    let Some((namespace, path)) = name.split_once(':') else {
                        return false;
                    };
                    let Some(model) = fs::read_to_string(
                        root.join(format!("assets/{namespace}/models/{path}.json")),
                    )
                    .ok()
                    .and_then(|text| CuboidModel::from_json(&text).ok()) else {
                        return false;
                    };
                    model.parent
                }
            };
            match parent {
                Some(parent) => name = parent,
                None => return true,
            }
        }
        false
    }

    /// Blocks require every referenced resource, including parents. Items instead
    /// skip broken resources and use the resolver's missing-model fallback.
    pub fn block_library(&self, root: &Path, pending: Vec<String>) -> Result<ModelLibrary> {
        self.block_models(root, pending, false)
    }
    /// [`Self::block_library`], but a model that does not read takes the
    /// built-in missing model, logged, as vanilla's model discovery does
    /// ("Missing block model"). Mods' content gets this; vanilla's resources
    /// always read.
    pub fn block_library_or_missing(
        &self,
        root: &Path,
        pending: Vec<String>,
    ) -> Result<ModelLibrary> {
        self.block_models(root, pending, true)
    }
    fn block_models(
        &self,
        root: &Path,
        mut pending: Vec<String>,
        or_missing: bool,
    ) -> Result<ModelLibrary> {
        let mut models = BTreeMap::new();
        let mut skipped = std::collections::BTreeSet::new();
        while let Some(name) = pending.pop() {
            if models.contains_key(&name) || skipped.contains(&name) {
                continue;
            }
            let known = self
                .snapshots
                .get(&name)
                .map(|source| source.decode())
                .or_else(|| Self::loader_cube(&name).map(Arc::new));
            let model = match known {
                Some(model) => model,
                None => {
                    let read = || -> Result<CuboidModel> {
                        let (namespace, path) =
                            name.split_once(':').ok_or("invalid model identifier")?;
                        let text = fs::read_to_string(
                            root.join(format!("assets/{namespace}/models/{path}.json")),
                        )?;
                        Ok(decode_model(root, &name, &text)?)
                    };
                    match read() {
                        Ok(model) => Arc::new(model),
                        Err(e) if or_missing => {
                            log::warn!("Horizon: missing block model {name}: {e}");
                            skipped.insert(name);
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
            };
            if let Some(parent) = &model.parent {
                pending.push(parent.clone());
            }
            models.insert(name, model);
        }
        Ok(ModelLibrary::from_shared(models)?)
    }
    pub fn into_item_library(self) -> std::result::Result<Arc<ModelLibrary>, String> {
        ModelLibrary::with_deferred_item_generator(
            self.snapshots
                .into_iter()
                .map(|(name, source)| (name, source as Arc<dyn ModelSource>))
                .collect(),
        )
        .map(Arc::new)
        .map_err(|e| format!("item models: {e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_assets::resolve::{GENERATED_MODEL, MISSING_MODEL};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn write(&self, name: &str, json: &str) {
            let (namespace, path) = name.split_once(':').unwrap();
            let path = self
                .0
                .join(format!("assets/{namespace}/models/{path}.json"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, json).unwrap();
        }
        fn asset(&self, path: &str, text: &str) {
            let path = self.0.join("assets").join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn compact_item_sources_release_trees_and_snapshot_parents_composites_and_obj() -> Result<()> {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("horizon-model-snapshot-{}", std::process::id())),
        );
        fixture.write("m:block/parent", r##"{"textures":{"all":"m:block/a","alias":"#all"},"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"up":{"texture":"#alias"}}}]}"##);
        fixture.write("m:item/child", r#"{"parent":"m:block/parent","textures":{"all":"m:block/b"},"display":{"gui":{"rotation":[10,20,30]}}}"#);
        fixture.write("m:item/composite", r#"{"loader":"neoforge:composite","children":{"one":{"parent":"m:block/parent"},"two":{"parent":"minecraft:builtin/generated","textures":{"layer0":"m:item/a"}}}}"#);
        fixture.write(
            "m:item/obj",
            r#"{"loader":"neoforge:obj","model":"m:models/x.obj","textures":{"all":"m:block/a"}}"#,
        );
        fixture.write("m:item/no_obj", r#"{"loader":"neoforge:obj","model":"m:models/absent.obj","textures":{"all":"m:block/a"}}"#);
        fixture.write("m:cycle_a", r#"{"parent":"m:cycle_b"}"#);
        fixture.write("m:cycle_b", r#"{"parent":"m:cycle_a"}"#);
        fixture.asset(
            "m/models/x.obj",
            "mtllib x.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nusemtl face\nf 1 2 3 4\n",
        );
        fixture.asset("m/models/x.mtl", "newmtl face\nmap_Kd #all\n");
        let parsed = ParsedModels::load(&fixture.0);
        let decoded: BTreeMap<_, _> = parsed
            .snapshots
            .keys()
            .map(|name| {
                let (namespace, path) = name.split_once(':').unwrap();
                let text = fs::read_to_string(
                    fixture
                        .0
                        .join(format!("assets/{namespace}/models/{path}.json")),
                )
                .unwrap();
                (
                    name.clone(),
                    Arc::new(decode_model(&fixture.0, name, &text).unwrap()),
                )
            })
            .collect();
        let trees: Vec<_> = decoded.values().map(Arc::downgrade).collect();
        let eager = ModelLibrary::with_shared_item_generator(decoded)?;
        let names = [
            "m:item/child",
            "m:item/composite",
            "m:item/obj",
            "m:item/no_obj",
            "m:unknown",
            GENERATED_MODEL,
            MISSING_MODEL,
        ];
        let expected: Vec<_> = names
            .iter()
            .map(|name| format!("{:?}", eager.resolve(name).unwrap()))
            .collect();
        assert_eq!(eager.resolve("m:item/obj")?.obj_quads().len(), 1);
        drop(eager);
        let items = parsed.into_item_library()?;
        assert!(trees.iter().all(|tree| tree.upgrade().is_none()));
        // Both successful and failed reads remain frozen until explicit reload.
        fixture.write("m:block/parent", r#"{"elements":[]}"#);
        fixture.asset("m/models/x.obj", "invalid changed OBJ");
        fixture.asset("m/models/x.mtl", "invalid changed material");
        fixture.asset(
            "m/models/absent.obj",
            "v 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n",
        );
        for _ in 0..2 {
            for (name, expected) in names.iter().zip(&expected) {
                assert_eq!(&format!("{:?}", items.resolve(name)?), expected, "{name}");
            }
            assert!(items.resolve("m:cycle_a").is_err());
            assert!(items.resolve("m:INVALID").is_err());
        }
        let reloaded = ParsedModels::load(&fixture.0).into_item_library()?;
        assert!(reloaded.resolve("m:item/child")?.elements().is_empty());
        Ok(())
    }

    #[test]
    fn repeated_source_json_and_obj_files_share_exact_backing() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("horizon-model-backing-{}", std::process::id())),
        );
        let json = r#"{"loader":"neoforge:obj","model":"m:models/shared.obj","textures":{"all":"m:block/a"}}"#;
        fixture.write("m:item/a", json);
        fixture.write("m:item/b", json);
        fixture.write("m:item/c", &json.replace("m:block/a", "m:block/c"));
        fixture.asset(
            "m/models/shared.obj",
            "mtllib shared.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\n",
        );
        fixture.asset("m/models/shared.mtl", "newmtl face\nmap_Kd #all\n");
        let parsed = ParsedModels::load(&fixture.0);
        let a = &parsed.snapshots["m:item/a"];
        let b = &parsed.snapshots["m:item/b"];
        let c = &parsed.snapshots["m:item/c"];
        assert!(Arc::ptr_eq(&a.json, &b.json));
        assert!(!Arc::ptr_eq(&a.json, &c.json));
        for path in ["models/shared.obj", "models/shared.mtl"] {
            let key = ("m".to_owned(), path.to_owned());
            assert!(Arc::ptr_eq(
                a.files[&key].as_ref().unwrap(),
                c.files[&key].as_ref().unwrap()
            ));
        }
    }

    #[test]
    fn blockstate_with_a_missing_model_falls_back_to_a_cube_of_its_texture() -> Result<()> {
        use horizon_assets::blockstate::BlockStateDefinition;
        let fixture = Fixture(
            std::env::temp_dir().join(format!("horizon-loader-cube-{}", std::process::id())),
        );
        fixture.write("horizon:present", r#"{"elements":[]}"#);
        let parsed = ParsedModels::load(&fixture.0);
        assert!(parsed.chain_loads(&fixture.0, "horizon:present"));
        assert!(!parsed.chain_loads(&fixture.0, "horizon:gone"));
        let text = r#"{"variants":{"a":{"model":"horizon:gone"},"b":{"model":"horizon:present"}},
            "athena:loader":"athena:ctm","ctm_textures":{"center":"horizon:block/c","particle":"horizon:block/p"}}"#;
        let mut definition = BlockStateDefinition::from_json(text).unwrap();
        let sprite = horizon_assets::model::loader_fallback_texture(text).unwrap();
        let cube = ParsedModels::loader_cube_name(&sprite);
        definition
            .replace_models(|model| (!parsed.chain_loads(&fixture.0, model)).then(|| cube.clone()));
        let models: Vec<_> = definition
            .variants
            .unwrap()
            .into_iter()
            .map(|(_, set)| format!("{set:?}"))
            .collect();
        assert!(models[0].contains(&cube) && models[1].contains("horizon:present"));
        let library = parsed.block_library(&fixture.0, vec![cube.clone()])?;
        let model = library.resolve(&cube)?;
        assert_eq!(model.elements().len(), 1);
        assert_eq!(model.material("all")?.unwrap().sprite, "horizon:block/p");
        Ok(())
    }

    #[test]
    fn shared_views_preserve_resolution_errors_builtins_and_reload() -> Result<()> {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("horizon-shared-models-{}", std::process::id())),
        );
        fixture.write("horizon:parent", r##"{"textures":{"side":"#all","all":"block/stone"},"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"up":{"texture":"#side"}}}]}"##);
        fixture.write("horizon:nested/child", r#"{"parent":"horizon:parent","textures":{"all":"block/glass"},"display":{"gui":{"rotation":[30,45,0]}}}"#);
        fixture.write("horizon:broken", "{");
        fixture.write("horizon:dangling", r#"{"parent":"horizon:absent"}"#);
        fixture.write("horizon:a", r#"{"parent":"horizon:b"}"#);
        fixture.write("horizon:b", r#"{"parent":"horizon:a"}"#);
        fixture.write(
            GENERATED_MODEL,
            r#"{"elements":[{"from":[0,0,0],"to":[1,1,1],"faces":{"up":{"texture":"particle"}}}],"gui_light":"side"}"#,
        );
        fixture.write(MISSING_MODEL, r#"{"elements":[]}"#);
        let parsed = ParsedModels::load(&fixture.0);
        let direct = ParsedModels::default();
        let names = vec![
            "horizon:nested/child".into(),
            GENERATED_MODEL.into(),
            MISSING_MODEL.into(),
            "horizon:a".into(),
        ];
        let old = direct.block_library(&fixture.0, names.clone())?;
        let shared = parsed.block_library(&fixture.0, names)?;
        for name in ["horizon:nested/child", GENERATED_MODEL, MISSING_MODEL] {
            assert_eq!(
                format!("{:?}", old.resolve(name)?),
                format!("{:?}", shared.resolve(name)?)
            );
        }
        assert!(shared.resolve("horizon:a").is_err());
        for name in ["horizon:broken", "horizon:absent", "horizon:dangling"] {
            let load = |models: &ParsedModels| {
                models
                    .block_library(&fixture.0, vec![name.into()])
                    .err()
                    .unwrap()
                    .to_string()
            };
            assert_eq!(load(&direct), load(&parsed));
            // Mods' content: vanilla's missing model instead of an error.
            let lenient = parsed.block_library_or_missing(&fixture.0, vec![name.into()])?;
            assert_eq!(
                format!("{:?}", lenient.resolve(name)?.elements()),
                format!("{:?}", lenient.resolve(MISSING_MODEL)?.elements()),
                "{name}"
            );
        }
        let items = parsed.into_item_library()?;
        let block = shared.resolve("horizon:nested/child")?;
        let item = items.resolve("horizon:nested/child")?;
        assert_eq!(block.elements(), item.elements());
        assert_eq!(
            item.material("side")?.unwrap().sprite,
            "minecraft:block/glass"
        );
        assert_eq!(shared.resolve(GENERATED_MODEL)?.elements().len(), 1);
        assert!(items.resolve(GENERATED_MODEL)?.elements().is_empty());
        assert!(items.resolve(GENERATED_MODEL)?.uses_item_generator());
        for name in [
            "horizon:broken",
            "horizon:absent",
            "horizon:dangling",
            MISSING_MODEL,
        ] {
            assert_eq!(items.resolve(name)?.elements().len(), 1);
        }
        assert!(items.resolve("horizon:a").is_err());
        fixture.write("horizon:parent", r#"{"elements":[]}"#);
        let reloaded = ParsedModels::load(&fixture.0).into_item_library()?;
        assert!(
            reloaded
                .resolve("horizon:nested/child")?
                .elements()
                .is_empty()
        );
        assert_eq!(items.resolve("horizon:nested/child")?.elements().len(), 1);
        // An unrelated bad item identifier must not make block preparation fail
        // (which would trigger resource-pack rollback at a different stage).
        fixture.write("horizon:INVALID", "{}");
        let parsed = ParsedModels::load(&fixture.0);
        assert!(
            parsed
                .block_library(&fixture.0, vec!["horizon:nested/child".into()])
                .is_ok()
        );
        assert!(parsed.into_item_library().is_err());
        Ok(())
    }
}
