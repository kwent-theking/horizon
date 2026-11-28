use super::*;
use crate::models::cube::cube_styles;

const VARIANTS: &str = r#"{"variants":{
    "mode=a":{"model":"test:block/base"},
    "mode=b":{"model":"test:block/alias"},
    "mode=c":[{"model":"test:block/base","y":90,"uvlock":true,"weight":2},
              {"model":"test:block/detail","weight":3}]
}}"#;
const MULTIPART: &str = r#"{"multipart":[
    {"apply":{"model":"test:block/base"}},
    {"when":{"mode":"b"},"apply":{"model":"test:block/detail"}},
    {"when":{"mode":"c"},"apply":[{"model":"test:block/alias","weight":1},
                                 {"model":"test:block/detail","weight":2}]}
]}"#;
const BLOCKS: [(&str, &str); 2] = [("test:variants", VARIANTS), ("test:multipart", MULTIPART)];
const BASE_ELEMENT: &str = r##"{"from":[0,0,0],"to":[16,12,16],"light_emission":3,
    "faces":{"north":{"texture":"#all","cullface":"north","tintindex":2,"uv":[1,2,15,13]}}}"##;
const DETAIL_ELEMENT: &str = r##"{"from":[1,1,1],"to":[15,15,15],"shade":false,
    "faces":{"up":{"texture":"#all","uv":[0,1,9,14],"rotation":90}}}"##;

struct Fixture {
    root: std::path::PathBuf,
    registry: BlockRegistry,
    facts: BTreeMap<String, Value>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "horizon-model-pool-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let blockstates = root.join("assets/test/blockstates");
        let models = root.join("assets/test/models/block");
        fs::create_dir_all(&blockstates).unwrap();
        fs::create_dir_all(&models).unwrap();
        for (name, definition) in BLOCKS {
            let (_, path) = name.split_once(':').unwrap();
            fs::write(blockstates.join(format!("{path}.json")), definition).unwrap();
        }
        for (name, elements) in [
            ("base", BASE_ELEMENT.to_owned()),
            ("detail", format!("{BASE_ELEMENT},{DETAIL_ELEMENT}")),
        ] {
            fs::write(
                models.join(format!("{name}.json")),
                format!(
                    r##"{{"ambientocclusion":false,"textures":{{"all":"minecraft:missingno","particle":"#all"}},"elements":[{elements}]}}"##
                ),
            )
            .unwrap();
        }
        fs::write(models.join("alias.json"), r#"{"parent":"test:block/base"}"#).unwrap();
        let states = BLOCKS
            .into_iter()
            .flat_map(|(name, _)| {
                ["a", "b", "c"].map(|mode| BlockState {
                    name: name.into(),
                    properties: [("mode".into(), mode.into())].into(),
                })
            })
            .collect();
        let registry = BlockRegistry::from_ordered_states(
            states,
            [("test:variants".into(), 0), ("test:multipart".into(), 3)].into(),
        )
        .unwrap();
        let facts = BLOCKS
            .into_iter()
            .map(|(name, _)| {
                (
                    name.to_owned(),
                    serde_json::json!({
                        "schema":{"mode":{"named":["a","b","c"]}},
                        "states":[{"mode":"a"},{"mode":"b"},{"mode":"c"}],
                    }),
                )
            })
            .collect();
        Self {
            root,
            registry,
            facts,
        }
    }

    fn prepare(&self, sprites: &mut Sprites) -> StateModels {
        StateModels::from_facts(
            &self.root,
            &self.registry,
            &BLOCKS.map(|(name, _)| name),
            sprites,
            &self.facts,
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn prepared_registry_faces_match_direct_bakes_for_every_state() {
    let fixture = Fixture::new();
    let mut sprites = Sprites::default();
    let cold = fixture.prepare(&mut sprites);
    let encoded = postcard::to_allocvec(&cold).unwrap();
    let warm: StateModels = postcard::from_bytes(&encoded).unwrap();
    let library = crate::model_library::ParsedModels::default()
        .block_library(
            &fixture.root,
            ["base", "alias", "detail"]
                .map(|name| format!("test:block/{name}"))
                .into(),
        )
        .unwrap();
    for (block, (name, json)) in BLOCKS.into_iter().enumerate() {
        let definition = BlockStateDefinition::from_json(json).unwrap();
        let schema = [(
            "mode".into(),
            PropertyValues::Named(["a".into(), "b".into(), "c".into()].into()),
        )]
        .into();
        let states =
            serde_json::from_value::<Vec<Properties>>(fixture.facts[name]["states"].clone())
                .unwrap();
        let bound = definition.bind(&schema, &states).unwrap();
        let choices = CompiledChoices::new(&definition).unwrap();
        for (offset, selected) in bound.iter().enumerate() {
            let state = BlockStateId((block * states.len() + offset) as u32);
            let selected = selected.as_ref().unwrap();
            for seed in [0, 1, 42, -1, i64::MIN, i64::MAX] {
                let mut variants = Vec::new();
                choices
                    .collect(selected, &mut LegacyRandom::new(seed), &mut variants)
                    .unwrap();
                let selected =
                    [&cold, &warm].map(|catalog| catalog.collect_seeded(state, seed).unwrap());
                assert!(selected.iter().all(|models| models.len() == variants.len()));
                for (i, expected) in variants.into_iter().enumerate() {
                    let model = library.resolve(&expected.model).unwrap();
                    let quads = bake_block_model(
                        &model,
                        BlockModelTransform::new(expected.rotations, expected.uv_lock),
                    )
                    .unwrap();
                    let faces: Vec<_> = quads
                        .into_iter()
                        .map(|quad| {
                            let layer = sprites.layers[&quad.material.sprite];
                            BakedFace {
                                layer,
                                opaque: sprites.region_opaque(layer, &quad.uv),
                                translucent: quad.material.force_translucent
                                    || sprites.translucent[layer as usize],
                                quad: quad.into(),
                            }
                        })
                        .collect();
                    for models in &selected {
                        let (variant, actual) = models[i];
                        assert_eq!(variant, expected);
                        assert!(
                            actual
                                .faces
                                .iter()
                                .map(face_key)
                                .eq(faces.iter().map(face_key)),
                            "{name}, state {state:?}, seed {seed}, variant {variant:?}"
                        );
                        assert_eq!(actual.ambient_occlusion, model.ambient_occlusion);
                        assert_eq!(actual.particle, Some(sprites.layers["minecraft:missingno"]));
                        assert_eq!(actual.cube.as_deref(), cube_styles(&faces).as_ref());
                    }
                }
            }
        }
    }
}

#[test]
fn identical_variants_share_models_and_overlapping_lists_share_faces() {
    let fixture = Fixture::new();
    let cold = fixture.prepare(&mut Sprites::default());
    let encoded = postcard::to_allocvec(&cold).unwrap();
    let warm: StateModels = postcard::from_bytes(&encoded).unwrap();
    for catalog in [&cold, &warm] {
        let base = catalog.collect_seeded(BlockStateId(0), 0).unwrap()[0].1;
        let alias = catalog.collect_seeded(BlockStateId(1), 0).unwrap()[0].1;
        let multipart = catalog.collect_seeded(BlockStateId(4), 0).unwrap();
        assert!(std::ptr::eq(base, multipart[0].1));
        assert!(Arc::ptr_eq(&base.faces.indices, &alias.faces.indices));
        assert_eq!(base.faces.len(), 1);
        assert_eq!(multipart[1].1.faces.len(), 2);
        assert!(std::ptr::eq(&base.faces[0], &multipart[1].1.faces[0]));
    }
}
