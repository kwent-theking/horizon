//! Bind prepared registry states to model choices; bake each transformed model
//! once and share its geometry across positions. Resource failure recovery and
//! resource-pack reloads remain separate work.
use crate::{Result, sprites::Sprites};
use horizon_assets::{
    bake::{BlockModelTransform, ModelQuad, bake_block_model},
    blockstate::{
        BlockStateDefinition, Variant, VariantSet,
        bind::{Properties, PropertySchema, PropertyValues, SelectedModels},
        choice::CompiledChoices,
    },
};
use horizon_core::{
    position::BlockPos,
    random::{LegacyRandom, position_seed},
};
use horizon_world::{
    chunk::BlockState,
    registry::{BlockRegistry, BlockStateId},
};
use serde_json::{Value, value::RawValue};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct BakedFace {
    pub quad: BakedQuad,
    pub layer: u32,
    /// The texels under the quad's UVs are all opaque (`SpriteContents.computeTransparency`):
    /// drawn in the solid layer without an alpha test.
    pub opaque: bool,
    /// Fractional alpha: drawn in the sorted translucent pass.
    pub translucent: bool,
}
/// Block geometry after its material has been resolved to an atlas layer.
/// Keeping the source model's owned material string on every face would retain
/// a separate allocation for each baked quad, even though renderers use layer.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BakedQuad {
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub facing: horizon_assets::model::Direction,
    pub cull: Option<horizon_assets::model::Direction>,
    pub tint_index: i32,
    pub shade_direction_override: Option<horizon_assets::model::Direction>,
    pub light_emission: u8,
}
impl From<ModelQuad> for BakedQuad {
    fn from(quad: ModelQuad) -> Self {
        Self {
            positions: quad.positions,
            uv: quad.uv,
            facing: quad.facing,
            cull: quad.cull,
            tint_index: quad.tint_index,
            shade_direction_override: quad.shade_direction_override,
            light_emission: quad.light_emission,
        }
    }
}
#[cfg(test)]
mod baked_quad_tests {
    use super::*;

    #[test]
    fn compact_quad_keeps_render_attributes_without_the_material_string() {
        let source = ModelQuad {
            positions: [[1.0, 2.0, 3.0]; 4],
            uv: [[0.25, 0.75]; 4],
            facing: horizon_assets::model::Direction::North,
            cull: Some(horizon_assets::model::Direction::West),
            material: horizon_assets::model::Material {
                sprite: "example:block/texture".into(),
                force_translucent: true,
            },
            tint_index: 2,
            shade_direction_override: Some(horizon_assets::model::Direction::Up),
            light_emission: 7,
        };
        let compact = BakedQuad::from(source.clone());
        assert_eq!(compact.positions, source.positions);
        assert_eq!(compact.uv, source.uv);
        assert_eq!(compact.facing, source.facing);
        assert_eq!(compact.cull, source.cull);
        assert_eq!(compact.tint_index, source.tint_index);
        assert_eq!(
            compact.shade_direction_override,
            source.shade_direction_override
        );
        assert_eq!(compact.light_emission, source.light_emission);
        assert!(std::mem::size_of::<BakedQuad>() < std::mem::size_of::<ModelQuad>());
    }

    #[test]
    fn prepared_model_order_and_baked_geometry_match_across_batch_threads() {
        let model = horizon_assets::model::CuboidModel::from_json(
            r##"{"textures":{"all":"test:block/stone","particle":"#all"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"north":{"texture":"#all"}}}]}"##,
        )
        .unwrap();
        let models = (0..130)
            .map(|i| (format!("test:block/model_{i:03}"), model.clone()))
            .collect();
        let library = horizon_assets::resolve::ModelLibrary::new(models).unwrap();
        let variants: Vec<_> = (0..130)
            .map(|i| (format!("test:block/model_{i:03}"), [0, 0, 0], false))
            .collect();
        let mut expected = None;
        for threads in [1, 2, 6] {
            let mut actual = Vec::new();
            for_each_prepared_variant_batch(&library, &variants, threads, 128, |batch| {
                actual.extend(batch.into_iter().map(
                    |(name, turns, lock, _model, particle, quads)| {
                        (
                            (name, turns, lock),
                            particle,
                            quads.into_iter().map(BakedQuad::from).collect::<Vec<_>>(),
                        )
                    },
                ));
                Ok(())
            })
            .unwrap();
            assert_eq!(actual.len(), 130);
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BakedModel {
    pub ambient_occlusion: bool,
    /// The model's particle slot also exists for models without block faces.
    pub particle: Option<u32>,
    pub faces: std::sync::Arc<[BakedFace]>,
    pub cube: Option<Box<[super::cube::CubeStyle; 6]>>,
}
type VariantKey = (String, [u8; 3], bool);

type PreparedBlockVariant = (
    String,
    [u8; 3],
    bool,
    horizon_assets::resolve::ResolvedCuboidModel,
    Option<String>,
    Vec<ModelQuad>,
);

fn for_each_prepared_variant_batch(
    library: &horizon_assets::resolve::ModelLibrary,
    variants: &[(String, [u8; 3], bool)],
    threads: usize,
    batch_size: usize,
    mut consume: impl FnMut(Vec<PreparedBlockVariant>) -> Result<()>,
) -> Result<()> {
    for batch in variants.chunks(batch_size.max(1)) {
        let prepared = crate::par::par_map_with(threads, batch, |(name, turns, lock)| {
            let model = library.resolve(name).map_err(|e| e.to_string())?;
            let particle = model
                .material("particle")
                .map_err(|e| e.to_string())?
                .map(|material| material.sprite.clone());
            let quads = bake_block_model(&model, BlockModelTransform::new(*turns, *lock))
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((name.clone(), *turns, *lock, model, particle, quads))
        });
        let prepared: Vec<_> = prepared
            .into_iter()
            .collect::<std::result::Result<_, _>>()
            .map_err(|e: String| -> Box<dyn std::error::Error> { e.into() })?;
        consume(prepared)?;
    }
    Ok(())
}

/// Supplies one block's model-state facts at a time. The production path keeps
/// the document as raw JSON until a block is actually bound; callers with an
/// already materialized document can continue borrowing their Values.
pub(crate) trait FactSource {
    fn block(&self, name: &str) -> Result<Option<Cow<'_, Value>>>;
}
impl FactSource for BTreeMap<String, Value> {
    fn block(&self, name: &str) -> Result<Option<Cow<'_, Value>>> {
        Ok(self.get(name).map(Cow::Borrowed))
    }
}
/// A raw block entry validated as a `Value` while deserializing the outer map,
/// then retained as text so the full fact tree is never resident at once.
pub(crate) struct RawFact(Box<RawValue>);
impl<'de> serde::Deserialize<'de> for RawFact {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let raw = <Box<RawValue> as serde::Deserialize>::deserialize(deserializer)?;
        // The old whole-document Value parse sees each block fact one level
        // below the root object. Wrap this borrowed raw slice in one array so
        // the temporary validation Value gets the same recursion-depth budget
        // without copying the block's JSON text.
        let wrapped = io::Cursor::new(&b"["[..])
            .chain(io::Cursor::new(raw.get().as_bytes()))
            .chain(io::Cursor::new(&b"]"[..]));
        let _: Vec<Value> = serde_json::from_reader(wrapped).map_err(serde::de::Error::custom)?;
        Ok(Self(raw))
    }
}
impl FactSource for BTreeMap<String, RawFact> {
    fn block(&self, name: &str) -> Result<Option<Cow<'_, Value>>> {
        let Some(raw) = self.get(name) else {
            return Ok(None);
        };
        Ok(Some(Cow::Owned(serde_json::from_str(raw.0.get())?)))
    }
}

#[cfg(test)]
mod fact_source_tests {
    use super::*;
    use horizon_world::{nbt::Tag, registry::BlockRegistry};

    #[test]
    fn raw_facts_match_values_and_duplicate_keys_keep_the_last_entry() {
        let json = r#"{
            "minecraft:test":{"schema":{"age":{"integer":[0,1]}},"states":[{"age":"0"}]},
            "minecraft:test":{"schema":{"age":{"integer":[2,3]}},"states":[{"age":"2"}]}
        }"#;
        let raw: BTreeMap<String, RawFact> = serde_json::from_str(json).unwrap();
        let values: BTreeMap<String, Value> = serde_json::from_str(json).unwrap();
        let raw_fact = raw.block("minecraft:test").unwrap().unwrap();
        assert_eq!(raw_fact.as_ref(), &values["minecraft:test"]);
        assert_eq!(raw_fact["schema"]["age"]["integer"][0], 2);
        assert!(raw.block("minecraft:absent").unwrap().is_none());
        assert!(
            serde_json::from_str::<BTreeMap<String, RawFact>>(
                r#"{"minecraft:unused":{"n":1e999}}"#
            )
            .is_err()
        );
        for depth in 1..=140 {
            let nested = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            let document = format!(r#"{{"minecraft:depth":{nested}}}"#);
            let eager = serde_json::from_str::<BTreeMap<String, Value>>(&document).is_ok();
            let raw = serde_json::from_str::<BTreeMap<String, RawFact>>(&document).is_ok();
            assert_eq!(raw, eager, "nesting depth {depth}");
        }
    }

    #[test]
    fn raw_and_value_facts_keep_registry_identity_validation() {
        let directory =
            std::env::temp_dir().join(format!("horizon-model-fact-source-{}", std::process::id()));
        let blockstates = directory.join("assets/minecraft/blockstates");
        fs::create_dir_all(&blockstates).unwrap();
        fs::write(
            blockstates.join("test_block.json"),
            r#"{"variants":{"":{"model":"minecraft:block/stone"}}}"#,
        )
        .unwrap();
        let registry_state = BlockState {
            name: "minecraft:test_block".into(),
            properties: BTreeMap::from([("axis".into(), "x".into())]),
        };
        let registry = BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
            ("states", Tag::List(vec![registry_state.to_nbt()])),
            (
                "defaults",
                Tag::compound([(
                    horizon_world::nbt::JavaString::from("minecraft:test_block"),
                    Tag::Int(0),
                )]),
            ),
        ]))
        .unwrap();
        let json = r#"{"minecraft:test_block":{"schema":{"axis":{"named":["x"]}},"states":[{"axis":"y"}]}}"#;
        let values: BTreeMap<String, Value> = serde_json::from_str(json).unwrap();
        let raw: BTreeMap<String, RawFact> = serde_json::from_str(json).unwrap();
        fn check<S: FactSource>(directory: &Path, registry: &BlockRegistry, facts: &S) -> String {
            let mut sprites = Sprites::default();
            StateModels::from_facts_with_models(
                directory,
                registry,
                &["test_block"],
                &mut sprites,
                facts,
                &crate::model_library::ParsedModels::default(),
            )
            .err()
            .unwrap()
            .to_string()
        }
        assert!(
            check(&directory, &registry, &values)
                .contains("disagree with the imported block registry")
        );
        assert!(
            check(&directory, &registry, &raw)
                .contains("disagree with the imported block registry")
        );
        let _ = fs::remove_dir_all(directory);
    }
}

pub struct StateModels {
    /// Each definition's choices, a variant standing as its index in `baked`.
    choices: Vec<CompiledChoices<u32>>,
    /// By state id: its definition in `choices` and the models it selects.
    states: Vec<Option<(usize, StateSelectionWire)>>,
    /// Deduplicated multipart index lists shared across states.
    multipart_lists: Vec<Box<[usize]>>,
    /// Every transformed model once, with the variant it was baked from.
    baked: Vec<(Variant, BakedModel)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FaceKey {
    positions: [[u32; 3]; 4],
    uv: [[u32; 2]; 4],
    facing: u8,
    cull: Option<u8>,
    tint_index: i32,
    shade_direction_override: Option<u8>,
    light_emission: u8,
    layer: u32,
    opaque: bool,
    translucent: bool,
}

fn face_key(face: &BakedFace) -> FaceKey {
    let q = &face.quad;
    FaceKey {
        positions: q.positions.map(|p| p.map(f32::to_bits)),
        uv: q.uv.map(|u| u.map(f32::to_bits)),
        facing: q.facing as u8,
        cull: q.cull.map(|d| d as u8),
        tint_index: q.tint_index,
        shade_direction_override: q.shade_direction_override.map(|d| d as u8),
        light_emission: q.light_emission,
        layer: face.layer,
        opaque: face.opaque,
        translucent: face.translucent,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BakedModelWire {
    ambient_occlusion: bool,
    particle: Option<u32>,
    faces: u32,
    cube: Option<Box<[super::cube::CubeStyle; 6]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
enum StateSelectionWire {
    Variant(usize),
    Multipart(u32),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StateModelsWire {
    choices: Vec<CompiledChoices<u32>>,
    multipart_lists: Vec<Vec<usize>>,
    states: Vec<Option<(usize, StateSelectionWire)>>,
    faces: Vec<BakedFace>,
    face_lists: Vec<Vec<u32>>,
    baked: Vec<(Variant, BakedModelWire)>,
}

impl serde::Serialize for StateModels {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut face_pool = Vec::new();
        let mut face_index = std::collections::HashMap::<FaceKey, u32>::new();
        let mut face_lists = Vec::new();
        let mut list_by_ptr = std::collections::HashMap::<usize, u32>::new();
        let mut list_by_keys = std::collections::HashMap::<Vec<u32>, u32>::new();
        let mut baked = Vec::with_capacity(self.baked.len());
        for (variant, model) in &self.baked {
            let ptr = model.faces.as_ptr() as usize;
            let list_id = if !model.faces.is_empty()
                && let Some(&id) = list_by_ptr.get(&ptr)
            {
                id
            } else {
                let ids: Vec<u32> = model
                    .faces
                    .iter()
                    .map(|f| {
                        let fk = face_key(f);
                        match face_index.entry(fk) {
                            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let id = face_pool.len() as u32;
                                face_pool.push(*f);
                                e.insert(id);
                                id
                            }
                        }
                    })
                    .collect();
                let id = match list_by_keys.entry(ids) {
                    std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let id = face_lists.len() as u32;
                        face_lists.push(e.key().clone());
                        e.insert(id);
                        id
                    }
                };
                if !model.faces.is_empty() {
                    list_by_ptr.insert(ptr, id);
                }
                id
            };
            baked.push((
                variant.clone(),
                BakedModelWire {
                    ambient_occlusion: model.ambient_occlusion,
                    particle: model.particle,
                    faces: list_id,
                    cube: model.cube.clone(),
                },
            ));
        }
        let multipart_lists = self.multipart_lists.iter().map(|l| l.to_vec()).collect();
        let states = self.states.clone();
        serde::Serialize::serialize(
            &StateModelsWire {
                choices: self.choices.clone(),
                multipart_lists,
                states,
                faces: face_pool,
                face_lists,
                baked,
            },
            serializer,
        )
    }
}

impl<'de> serde::Deserialize<'de> for StateModels {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire: StateModelsWire = serde::Deserialize::deserialize(deserializer)?;
        let empty_faces: std::sync::Arc<[BakedFace]> = std::sync::Arc::from([]);
        let mut slices = Vec::with_capacity(wire.face_lists.len());
        for list in wire.face_lists {
            if list.is_empty() {
                slices.push(empty_faces.clone());
                continue;
            }
            let mut faces = Vec::with_capacity(list.len());
            for id in list {
                let face = wire
                    .faces
                    .get(id as usize)
                    .copied()
                    .ok_or_else(|| serde::de::Error::custom("face pool index out of range"))?;
                faces.push(face);
            }
            slices.push(std::sync::Arc::from(faces));
        }
        let mut baked = Vec::with_capacity(wire.baked.len());
        for (variant, model) in wire.baked {
            let faces = slices
                .get(model.faces as usize)
                .cloned()
                .ok_or_else(|| serde::de::Error::custom("face list index out of range"))?;
            baked.push((
                variant,
                BakedModel {
                    ambient_occlusion: model.ambient_occlusion,
                    particle: model.particle,
                    faces,
                    cube: model.cube,
                },
            ));
        }
        let multipart_lists = wire
            .multipart_lists
            .into_iter()
            .map(Vec::into_boxed_slice)
            .collect();
        let states = wire.states;
        Ok(Self {
            choices: wire.choices,
            states,
            multipart_lists,
            baked,
        })
    }
}
fn key(variant: &Variant) -> VariantKey {
    (variant.model.clone(), variant.rotations, variant.uv_lock)
}
impl StateModels {
    pub(crate) fn valid(&self, states: usize, layers: u32) -> bool {
        self.states.len() == states
            && self
                .choices
                .iter()
                .all(|c| c.valid(|id| (*id as usize) < self.baked.len()))
            && self.states.iter().flatten().all(|(choice, selected)| {
                let Some(choices) = self.choices.get(*choice) else {
                    return false;
                };
                match selected {
                    StateSelectionWire::Variant(v) => {
                        choices.valid_selection(&SelectedModels::Variant(*v))
                    }
                    StateSelectionWire::Multipart(id) => self
                        .multipart_lists
                        .get(*id as usize)
                        .is_some_and(|list| choices.valid_multipart(list)),
                }
            })
            && self.baked.iter().all(|(variant, model)| {
                variant.rotations.iter().all(|r| *r < 4)
                    && model.particle.is_none_or(|layer| layer < layers)
                    && model.faces.iter().all(|face| {
                        face.layer < layers
                            && face
                                .quad
                                .positions
                                .iter()
                                .flatten()
                                .chain(face.quad.uv.iter().flatten())
                                .all(|v| v.is_finite())
                    })
                    && model
                        .cube
                        .as_ref()
                        .is_none_or(|styles| styles.iter().all(|s| s.layer < layers))
            })
    }

    pub fn load(
        root: &Path,
        registry: &BlockRegistry,
        blocks: &[&str],
        sprites: &mut Sprites,
    ) -> Result<Self> {
        let facts: BTreeMap<String, RawFact> = serde_json::from_reader(std::io::BufReader::new(
            fs::File::open(root.join("model-states.json"))?,
        ))?;
        Self::from_facts_with_models(
            root,
            registry,
            blocks,
            sprites,
            &facts,
            &crate::model_library::ParsedModels::default(),
        )
    }
    /// Bind and bake models using an already parsed model-state document.
    pub fn from_facts(
        root: &Path,
        registry: &BlockRegistry,
        blocks: &[&str],
        sprites: &mut Sprites,
        facts: &BTreeMap<String, Value>,
    ) -> Result<Self> {
        Self::from_facts_with_models(
            root,
            registry,
            blocks,
            sprites,
            facts,
            &crate::model_library::ParsedModels::default(),
        )
    }
    pub(crate) fn from_facts_with_models(
        root: &Path,
        registry: &BlockRegistry,
        blocks: &[&str],
        sprites: &mut Sprites,
        facts: &impl FactSource,
        models: &crate::model_library::ParsedModels,
    ) -> Result<Self> {
        let mut choices = Vec::new();
        let mut states = vec![None; registry.len()];
        let mut variants = BTreeSet::new();
        let mut by_name: std::collections::HashMap<&str, Vec<BlockStateId>> =
            std::collections::HashMap::new();
        for i in 0..registry.len() {
            let id = BlockStateId(i as u32);
            if let Some(state) = registry.state(id) {
                by_name.entry(state.name.as_str()).or_default().push(id);
            }
        }
        let unique: Vec<&str> = blocks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for chunk in unique.chunks(256) {
            let texts = crate::par::par_map(chunk, |block| {
                let (namespace, path) = block.split_once(':').unwrap_or(("minecraft", block));
                fs::read_to_string(root.join(format!("assets/{namespace}/blockstates/{path}.json")))
                    .map_err(|e| e.to_string())
            });
            for (&block, text) in chunk.iter().zip(texts) {
                // Vanilla blocks come bare; content mods' blocks with their namespace.
                let (namespace, path) = block.split_once(':').unwrap_or(("minecraft", block));
                let name = format!("{namespace}:{path}");
                let fact = facts
                    .block(&name)?
                    .ok_or("missing block model state schema")?;
                let schema: PropertySchema = fact["schema"]
                    .as_object()
                    .ok_or("invalid model schema")?
                    .iter()
                    .map(|(key, value)| {
                        let values = if let Some(range) = value.get("integer") {
                            PropertyValues::Integer {
                                minimum: i32::try_from(
                                    range[0]
                                        .as_i64()
                                        .ok_or("invalid integer property minimum")?,
                                )?,
                                maximum: i32::try_from(
                                    range[1]
                                        .as_i64()
                                        .ok_or("invalid integer property maximum")?,
                                )?,
                            }
                        } else {
                            PropertyValues::Named(
                                value["named"]
                                    .as_array()
                                    .ok_or("missing named property values")?
                                    .iter()
                                    .map(|v| {
                                        v.as_str()
                                            .map(str::to_owned)
                                            .ok_or("invalid named property value")
                                    })
                                    .collect::<std::result::Result<_, _>>()?,
                            )
                        };
                        Ok((key.clone(), values))
                    })
                    .collect::<Result<_>>()?;
                let possible: Vec<Properties> = serde::Deserialize::deserialize(&fact["states"])?;
                // Vanilla's BlockStateModelLoader: a definition that does not
                // read or bind is logged and its block gets no model; the other
                // blocks still load.
                let loaded = text
                    .and_then(|text| {
                        let mut definition =
                            BlockStateDefinition::from_json(&text).map_err(|e| e.to_string())?;
                        // A connected-textures loader (athena) whose model is gone:
                        // a cube of its best texture instead of the missing model.
                        if text.contains("\"ctm_textures\"")
                            && let Some(sprite) =
                                horizon_assets::model::loader_fallback_texture(&text)
                        {
                            let cube =
                                crate::model_library::ParsedModels::loader_cube_name(&sprite);
                            definition.replace_models(|model| {
                                (!models.chain_loads(root, model)).then(|| cube.clone())
                            });
                        }
                        Ok(definition)
                    })
                    .and_then(|mut definition| {
                        // Imported mod resources can describe values absent from
                        // their registry. Keep the remaining multipart cases.
                        if namespace != "minecraft" {
                            definition.skip_unknown_multipart_values(&schema);
                        }
                        let bound = definition
                            .bind(&schema, &possible)
                            .map_err(|e| e.to_string())?;
                        Ok((definition, bound))
                    });
                let (definition, bound) = match loaded {
                    Ok(loaded) => loaded,
                    Err(e) => {
                        log::warn!(
                            "Horizon: blockstate definition of {namespace}:{path} not loaded: {e}"
                        );
                        continue;
                    }
                };
                let native_order: Vec<_> = by_name
                    .get(name.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|&id| (id, &registry.state(id).expect("indexed state").properties))
                    .collect();
                if native_order.len() != possible.len()
                    || native_order
                        .iter()
                        .zip(&possible)
                        .any(|((_, a), b)| *a != b)
                {
                    return Err(
                        "model state definitions disagree with the imported block registry".into(),
                    );
                }
                for ((id, _), selected) in native_order.into_iter().zip(bound) {
                    if let Some(selected) = selected {
                        states[id.0 as usize] = Some((choices.len(), selected));
                    }
                }
                for choice in definition
                    .variants
                    .iter()
                    .flatten()
                    .map(|(_, choice)| choice)
                    .chain(
                        definition
                            .multipart
                            .iter()
                            .flatten()
                            .map(|selector| &selector.variant),
                    )
                {
                    match choice {
                        VariantSet::Single(v) => {
                            variants.insert(key(v));
                        }
                        VariantSet::Weighted(v) => variants.extend(v.iter().map(|(v, _)| key(v))),
                    }
                }
                choices.push(CompiledChoices::new(&definition)?);
            }
        }
        let names: Vec<String> = variants.iter().map(|(name, _, _)| name.clone()).collect();
        let library = match models.block_library(root, names.clone()) {
            Ok(library) => library,
            Err(e) => {
                log::warn!("Horizon: block models incomplete ({e}); using the missing model");
                models.block_library_or_missing(root, names)?
            }
        };
        let variants: Vec<_> = variants.into_iter().collect();
        let mut baked = Vec::with_capacity(variants.len());
        let mut index = BTreeMap::new();
        let mut face_slices: std::collections::HashMap<Vec<FaceKey>, std::sync::Arc<[BakedFace]>> =
            std::collections::HashMap::new();
        let empty_faces: std::sync::Arc<[BakedFace]> = std::sync::Arc::from([]);
        let threads = crate::par::threads();
        crate::par::time("prepare_block_models", threads, || {
            for_each_prepared_variant_batch(&library, &variants, threads, 128, |staged| {
                // Prefetch one bounded batch, then assign atlas layers in the
                // same global variant order as the serial implementation.
                sprites.prefetch(
                    root,
                    staged.iter().flat_map(|(_, _, _, _, particle, quads)| {
                        particle
                            .iter()
                            .map(|sprite| sprite.as_str())
                            .chain(quads.iter().map(|quad| quad.material.sprite.as_str()))
                    }),
                );
                for (name, turns, lock, model, particle, quads) in staged {
                    let particle = particle
                        .map(|sprite| sprites.insert(root, &sprite))
                        .transpose()?;
                    let faces_vec: Vec<BakedFace> = quads
                        .into_iter()
                        .map(|quad| {
                            let layer = sprites.insert(root, &quad.material.sprite)?;
                            let translucent = quad.material.force_translucent
                                || sprites.translucent[layer as usize];
                            let opaque = sprites.region_opaque(layer, &quad.uv);
                            Ok(BakedFace {
                                translucent,
                                opaque,
                                quad: quad.into(),
                                layer,
                            })
                        })
                        .collect::<Result<_>>()?;
                    let cube = super::cube::cube_styles(&faces_vec).map(Box::new);
                    let faces: std::sync::Arc<[BakedFace]> = if faces_vec.is_empty() {
                        empty_faces.clone()
                    } else {
                        let keys: Vec<FaceKey> = faces_vec.iter().map(face_key).collect();
                        match face_slices.entry(keys) {
                            std::collections::hash_map::Entry::Occupied(e) => e.get().clone(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let arc: std::sync::Arc<[BakedFace]> =
                                    std::sync::Arc::from(faces_vec);
                                e.insert(arc.clone());
                                arc
                            }
                        }
                    };
                    index.insert((name.clone(), turns, lock), baked.len() as u32);
                    let variant = Variant {
                        model: name,
                        rotations: turns,
                        uv_lock: lock,
                    };
                    let model = BakedModel {
                        ambient_occlusion: model.ambient_occlusion,
                        particle,
                        faces,
                        cube,
                    };
                    baked.push((variant, model));
                }
                Ok(())
            })
        })?;
        // Every variant of every definition was baked above.
        let choices = choices.iter().map(|c| c.map(|v| index[&key(v)])).collect();
        let face_count = baked
            .iter()
            .map(|(_, model)| model.faces.len())
            .sum::<usize>();
        log::info!(
            "loaded block models: {} unique variants, {face_count} baked faces",
            baked.len()
        );
        let mut multipart_lists = Vec::new();
        let mut mp_index = std::collections::HashMap::<Vec<usize>, u32>::new();
        let packed_states: Vec<Option<(usize, StateSelectionWire)>> = states
            .into_iter()
            .map(|entry| {
                entry.map(|(choice, sel)| {
                    let sel_wire = match sel {
                        SelectedModels::Variant(v) => StateSelectionWire::Variant(v),
                        SelectedModels::Multipart(indices) => {
                            let id = match mp_index.entry(indices.clone()) {
                                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    let id = multipart_lists.len() as u32;
                                    multipart_lists.push(indices.into_boxed_slice());
                                    e.insert(id);
                                    id
                                }
                            };
                            StateSelectionWire::Multipart(id)
                        }
                    };
                    (choice, sel_wire)
                })
            })
            .collect();
        Ok(Self {
            choices,
            states: packed_states,
            multipart_lists,
            baked,
        })
    }
    pub fn collect(
        &self,
        state: BlockStateId,
        position: BlockPos,
    ) -> Result<Vec<(&Variant, &BakedModel)>> {
        // Blocks use their position seed; contained/displayed blocks can supply one.
        self.collect_seeded(state, position_seed(position))
    }
    /// `BlockStateModel.particleMaterial`: the particle sprite layer of the
    /// state's first variant (see `CompiledChoices::particle_variant`).
    pub fn particle(&self, state: BlockStateId) -> Option<u32> {
        let (choice, selected) = self.state(state)?;
        let model = match selected {
            StateSelectionWire::Variant(v) => {
                let sel = SelectedModels::Variant(*v);
                self.choices[*choice].particle_variant(&sel)?
            }
            StateSelectionWire::Multipart(_) => {
                let sel = SelectedModels::Multipart(Vec::new());
                self.choices[*choice].particle_variant(&sel)?
            }
        };
        self.baked[*model as usize].1.particle
    }
    pub fn collect_seeded(
        &self,
        state: BlockStateId,
        seed: i64,
    ) -> Result<Vec<(&Variant, &BakedModel)>> {
        let mut models = Vec::new();
        self.each_seeded(state, seed, |(variant, model)| {
            models.push((variant, model))
        })?;
        Ok(models)
    }
    /// The models a block draws at `position`, into `out` (cleared first):
    /// `collect` for the mesher, which reuses one list for every block.
    pub fn models_at<'a>(
        &'a self,
        state: BlockStateId,
        position: BlockPos,
        out: &mut Vec<&'a BakedModel>,
    ) -> Result<()> {
        out.clear();
        self.each_seeded(state, position_seed(position), |(_, model)| out.push(model))
    }
    fn each_seeded<'a>(
        &'a self,
        state: BlockStateId,
        seed: i64,
        mut f: impl FnMut(&'a (Variant, BakedModel)),
    ) -> Result<()> {
        let (choice, selected) = self.state(state).ok_or("unbound gallery block state")?;
        let mut random = LegacyRandom::new(seed);
        match selected {
            StateSelectionWire::Variant(v) => {
                let sel = SelectedModels::Variant(*v);
                self.choices[*choice].for_each(&sel, &mut random, |&model| {
                    f(&self.baked[model as usize]);
                })?;
            }
            StateSelectionWire::Multipart(id) => {
                let list = self
                    .multipart_lists
                    .get(*id as usize)
                    .ok_or("multipart list index out of range")?;
                self.choices[*choice].for_each_multipart(list, &mut random, |&model| {
                    f(&self.baked[model as usize]);
                })?;
            }
        }
        Ok(())
    }
    fn state(&self, state: BlockStateId) -> Option<&(usize, StateSelectionWire)> {
        self.states.get(state.0 as usize)?.as_ref()
    }
}

pub fn descriptor(name: &str, overrides: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: overrides
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use horizon_world::resource_cache::Cache;

    fn fixture() -> StateModels {
        let definition = BlockStateDefinition::from_json(
            r#"{"variants":{"":[{"model":"test:cube","weight":2},{"model":"test:other","weight":3}]}}"#,
        ).unwrap();
        StateModels {
            choices: vec![CompiledChoices::new(&definition).unwrap().map(|_| 0)],
            states: vec![Some((0, StateSelectionWire::Variant(0)))],
            multipart_lists: Vec::new(),
            baked: vec![(
                Variant {
                    model: "test:cube".into(),
                    rotations: [0, 1, 0],
                    uv_lock: true,
                },
                BakedModel {
                    ambient_occlusion: true,
                    particle: Some(0),
                    cube: None,
                    faces: std::sync::Arc::from(vec![BakedFace {
                        layer: 0,
                        opaque: true,
                        translucent: false,
                        quad: BakedQuad {
                            positions: [[0.0, 1.0, 0.0]; 4],
                            uv: [[0.25, 0.75]; 4],
                            facing: horizon_assets::model::Direction::Up,
                            cull: Some(horizon_assets::model::Direction::Up),
                            tint_index: 2,
                            shade_direction_override: None,
                            light_emission: 3,
                        },
                    }]),
                },
            )],
        }
    }

    #[test]
    fn binary_catalog_round_trip_keeps_choices_quads_and_rejects_invalid_indexes() {
        let root =
            std::env::temp_dir().join(format!("horizon-catalog-snapshot-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("horizon-mod-content.stamp"), "catalog-a").unwrap();
        let cache = Cache::new(&root, "catalog-test-v1", &[], &[]).unwrap();
        let original = fixture();
        assert!(original.valid(1, 1));
        cache.write_value(&original).unwrap();
        let mut warm: StateModels = cache.read_value().unwrap();
        assert!(warm.valid(1, 1));
        for seed in [0, 1, -1, i64::MAX] {
            let a = original.collect_seeded(BlockStateId(0), seed).unwrap();
            let b = warm.collect_seeded(BlockStateId(0), seed).unwrap();
            assert_eq!(a.len(), b.len());
            for ((va, ma), (vb, mb)) in a.iter().zip(&b) {
                assert_eq!(va, vb);
                assert_eq!(ma.faces[0].quad, mb.faces[0].quad);
                assert_eq!(ma.particle, mb.particle);
                assert_eq!(ma.ambient_occlusion, mb.ambient_occlusion);
            }
        }
        warm.states[0].as_mut().unwrap().0 = usize::MAX;
        assert!(!warm.valid(1, 1));
        warm = cache.read_value().unwrap();
        warm.baked[0].1.faces = std::sync::Arc::from(vec![BakedFace {
            layer: 1,
            ..warm.baked[0].1.faces[0]
        }]);
        assert!(!warm.valid(1, 1));
        assert!(!original.valid(2, 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pooled_state_models_shares_identical_face_slices_and_rejects_bad_pool_indices() {
        let mut source = fixture();
        let copy = source.baked[0].1.faces.clone();
        source.baked.push((
            Variant {
                model: "test:cube_dup".into(),
                rotations: [0, 2, 0],
                uv_lock: false,
            },
            BakedModel {
                ambient_occlusion: true,
                particle: Some(0),
                cube: None,
                faces: copy,
            },
        ));
        let root = std::env::temp_dir().join(format!(
            "horizon-catalog-pool-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("horizon-mod-content.stamp"), "catalog-pool").unwrap();
        let cache = Cache::new(&root, "catalog-pool-v1", &[], &[]).unwrap();
        cache.write_value(&source).unwrap();
        let restored: StateModels = cache.read_value().unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &restored.baked[0].1.faces,
            &restored.baked[1].1.faces
        ));

        let bad = StateModelsWire {
            choices: Vec::new(),
            multipart_lists: Vec::new(),
            states: Vec::new(),
            faces: Vec::new(),
            face_lists: vec![vec![99]],
            baked: Vec::new(),
        };
        let bytes = postcard::to_allocvec(&bad).unwrap();
        assert!(postcard::from_bytes::<StateModels>(&bytes).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn multipart_compact_selection_shares_lists_and_evaluates() {
        let definition = BlockStateDefinition::from_json(
            r#"{"multipart":[{"apply":{"model":"test:cube"}},{"apply":{"model":"test:other"}}]}"#,
        )
        .unwrap();
        let choices = vec![CompiledChoices::new(&definition).unwrap().map(|_| 0)];
        let model = fixture().baked.remove(0);
        let models = StateModels {
            choices,
            states: vec![
                Some((0, StateSelectionWire::Multipart(0))),
                Some((0, StateSelectionWire::Multipart(0))),
            ],
            multipart_lists: vec![vec![0, 1].into_boxed_slice()],
            baked: vec![model],
        };
        assert!(models.valid(2, 1));
        let collected = models.collect_seeded(BlockStateId(0), 1234).unwrap();
        assert_eq!(collected.len(), 2);

        let dangling = StateModels {
            states: vec![Some((0, StateSelectionWire::Multipart(1)))],
            ..models
        };
        assert!(!dangling.valid(1, 1));
        assert!(dangling.collect_seeded(BlockStateId(0), 1234).is_err());
    }
}
