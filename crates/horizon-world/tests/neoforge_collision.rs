use horizon_core::geometry::VoxelShape;
use horizon_world::{
    DATA_VERSION,
    block_collision::{BlockCollisionRegistry, DynamicCollisionWorld, ShulkerCollisionState},
    chunk::BlockState,
    collision::{CachedCollisionRegistry, OracleShapes},
    dynamic_support::DynamicSupportWorld,
    nbt::Tag,
    registry::{BlockRegistry, BlockStateId},
};
use serde_json::Value;
use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

const FACTS: &[u8] = include_bytes!("../src/collision/fixtures/neoforge-shapes.json");

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|&(key, value)| (key.into(), value.into()))
            .collect(),
    }
}

fn registry() -> BlockRegistry {
    let mut states = vec![
        state("minecraft:air", &[]),
        state("minecraft:stone", &[]),
        state("fixture:fence", &[("north", "false"), ("east", "false")]),
        state("fixture:fence", &[("east", "true"), ("north", "false")]),
        state("fixture:fence", &[("east", "false"), ("north", "true")]),
        state("fixture:fence", &[("east", "true"), ("north", "true")]),
        state("fixture:slab", &[("type", "top")]),
        state("fixture:slab", &[("type", "bottom")]),
        state("fixture:grate", &[]),
    ];
    states.extend(
        ["south", "north", "west", "east"]
            .map(|facing| state("fixture:sconce", &[("facing", facing)])),
    );
    let mut defaults = BTreeMap::new();
    for (i, state) in states.iter().enumerate() {
        defaults.entry(state.name.clone()).or_insert(i as i64);
    }
    BlockRegistry::from_ordered_states(states, defaults).unwrap()
}

fn donor_table(registry: &BlockRegistry) -> CachedCollisionRegistry<'_> {
    let mut state_shapes = vec![0; registry.len()];
    state_shapes[0] = -1;
    let coords = Tag::LongArray(vec![0, 1.0f64.to_bits() as i64]);
    let cube = Tag::compound([
        ("x", coords.clone()),
        ("y", coords.clone()),
        ("z", coords),
        ("occupied", Tag::ByteArray(vec![1])),
        ("cube", Tag::Byte(1)),
        ("cube_axes", Tag::Byte(7)),
        ("canonical", Tag::Byte(2)),
        ("discrete_min", Tag::IntArray(vec![0; 3])),
        ("discrete_max", Tag::IntArray(vec![1; 3])),
    ]);
    CachedCollisionRegistry::from_nbt(
        &Tag::compound([
            ("DataVersion", Tag::Int(DATA_VERSION)),
            (
                "states",
                Tag::List(
                    (0..registry.len())
                        .map(|id| registry.state(BlockStateId(id as u32)).unwrap().to_nbt())
                        .collect(),
                ),
            ),
            ("shapes", Tag::List(vec![cube])),
            ("cached_shapes", Tag::IntArray(state_shapes)),
        ]),
        registry,
    )
    .unwrap()
}

#[derive(Default)]
struct Neighbours(BTreeMap<[i32; 3], BlockStateId>);

impl DynamicSupportWorld for Neighbours {
    type Error = Infallible;

    fn shulker_is_closed(&mut self, _: [i32; 3]) -> Result<Option<bool>, Self::Error> {
        Ok(None)
    }
    fn moving_piston_shape(&mut self, _: [i32; 3]) -> Result<Option<VoxelShape>, Self::Error> {
        Ok(None)
    }
}

impl DynamicCollisionWorld for Neighbours {
    fn shulker_collision_state(
        &mut self,
        _: [i32; 3],
    ) -> Result<Option<ShulkerCollisionState>, Self::Error> {
        Ok(None)
    }

    fn collision_neighbour(&mut self, position: [i32; 3]) -> Option<BlockStateId> {
        self.0.get(&position).copied()
    }
}

fn assert_boxes(shape: &VoxelShape, expected: &Value) {
    let mut expected: Vec<[f64; 6]> = serde_json::from_value(expected.clone()).unwrap();
    let mut actual: Vec<_> = shape
        .boxes()
        .iter()
        .map(|aabb| {
            let lo = aabb.min_position();
            let hi = aabb.max_position();
            [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
        })
        .collect();
    let sort = |a: &[f64; 6], b: &[f64; 6]| {
        a.iter()
            .zip(b)
            .map(|(a, b)| a.total_cmp(b))
            .find(|order| !order.is_eq())
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    actual.sort_by(sort);
    expected.sort_by(sort);
    assert_eq!(actual, expected);
}

#[test]
fn neoforge_collision_uses_oracle_states_and_world_neighbours() {
    let registry = registry();
    let oracle = OracleShapes::from_json(FACTS, &registry).unwrap();
    assert_eq!(oracle.states().count(), 10);
    let json: Value = serde_json::from_slice(FACTS).unwrap();
    let blocks = &json["blocks"];
    let fence = registry.default_state("fixture:fence").unwrap();
    let fence_rows = &blocks["fixture:fence"]["states"];
    let fence_row = &fence_rows["north=false,east=false"];
    let sconce = registry.default_state("fixture:sconce").unwrap();
    let sconce_row = &blocks["fixture:sconce"]["states"]["facing=south"];
    let stone = registry.default_state("minecraft:stone").unwrap();
    assert!(oracle.collision(stone, 0).is_none());
    assert!(oracle.outline(sconce, 0).unwrap().is_empty());
    assert_boxes(
        oracle.outline(sconce, 4).unwrap(),
        &sconce_row["neighbours"]["palette"][0]["outline"],
    );
    let (boxes, edges) = oracle.outline_geometry(sconce, 4).unwrap();
    assert_eq!(boxes.len(), 1);
    assert_eq!(edges.len(), 12);
    let slab = registry.default_state("fixture:slab").unwrap();
    assert_boxes(
        oracle.outline(slab, 0).unwrap(),
        &blocks["fixture:slab"]["states"]["type=top"]["outline"],
    );
    assert_boxes(
        oracle.outline(fence, 36).unwrap(),
        &fence_row["neighbours"]["palette"][2]["outline"],
    );

    let cached = donor_table(&registry).with_oracle_shapes(oracle);
    let collisions = BlockCollisionRegistry::new(Arc::new(cached)).unwrap();
    let mut world = Neighbours::default();
    let position = [20, 8, -3];
    let north = [20, 8, -4];
    let east = [21, 8, -3];
    assert_boxes(
        &collisions
            .collision_shape(&mut world, fence, position)
            .unwrap(),
        &fence_row["collision"],
    );
    world.0.insert(north, stone);
    assert_boxes(
        &collisions
            .collision_shape(&mut world, fence, position)
            .unwrap(),
        &fence_rows["east=false,north=true"]["collision"],
    );
    world.0.insert(east, stone);
    assert_boxes(
        &collisions
            .collision_shape(&mut world, fence, position)
            .unwrap(),
        &fence_row["neighbours"]["palette"][2]["collision"],
    );
    world.0.insert(north, slab);
    assert_boxes(
        &collisions
            .collision_shape(&mut world, fence, position)
            .unwrap(),
        &fence_rows["east=true,north=false"]["collision"],
    );
    assert!(collisions.reaches_out(fence));
    assert!(collisions.collision_block(sconce).unwrap().empty_shape);

    let north_sconce = registry
        .id(&state("fixture:sconce", &[("facing", "north")]))
        .unwrap();
    assert!(
        !collisions
            .collision_block(north_sconce)
            .unwrap()
            .empty_shape
    );
    assert!(
        collisions
            .collision_shape(&mut world, north_sconce, position)
            .unwrap()
            .is_empty()
    );
    world.0.insert([20, 8, -2], stone);
    assert_boxes(
        &collisions
            .collision_shape(&mut world, north_sconce, position)
            .unwrap(),
        &blocks["fixture:sconce"]["states"]["facing=north"]["outline"],
    );

    let grate = registry.default_state("fixture:grate").unwrap();
    assert_boxes(
        &collisions
            .collision_shape(&mut world, grate, position)
            .unwrap(),
        &blocks["fixture:grate"]["states"][""]["collision"],
    );
    let missing = registry
        .id(&state(
            "fixture:fence",
            &[("east", "true"), ("north", "true")],
        ))
        .unwrap();
    assert!(
        collisions
            .collision_shape(&mut world, missing, position)
            .unwrap()
            .is_block()
    );
}

#[test]
fn neoforge_outline_uses_oracle_states_and_world_neighbours() {
    let registry = registry();
    let oracle = OracleShapes::from_json(FACTS, &registry).unwrap();
    let json: Value = serde_json::from_slice(FACTS).unwrap();
    let blocks = &json["blocks"];
    let sconce = registry.default_state("fixture:sconce").unwrap();
    let north_sconce = registry
        .id(&state("fixture:sconce", &[("facing", "north")]))
        .unwrap();
    let west_sconce = registry
        .id(&state("fixture:sconce", &[("facing", "west")]))
        .unwrap();
    let east_sconce = registry
        .id(&state("fixture:sconce", &[("facing", "east")]))
        .unwrap();
    let slab = registry.default_state("fixture:slab").unwrap();
    let stone = registry.default_state("minecraft:stone").unwrap();
    let [sconce_base, north_sconce_base, slab_base] =
        [sconce, north_sconce, slab].map(|state| oracle.outline_geometry(state, 0).unwrap());
    let (boxes, edges) = oracle.outline_geometry(west_sconce, 0).unwrap();
    assert!(!boxes.is_empty());
    assert!(!edges.is_empty());
    let east_sconce_default = oracle.outline_geometry(east_sconce, 0).unwrap();
    assert!(east_sconce_default.0.is_empty());
    let cached = donor_table(&registry).with_oracle_shapes(oracle);
    let collisions = BlockCollisionRegistry::new(Arc::new(cached)).unwrap();

    let position = [20, 8, -3];
    let north = [20, 8, -4];
    let south = [20, 8, -2];
    let sconce_row = &blocks["fixture:sconce"]["states"]["facing=south"];
    let profile = &sconce_row["neighbours"];
    let palette_row = profile["masks"]["4"].as_u64().unwrap() as usize;
    let outline = |world: &Neighbours, state| {
        collisions
            .neighbour_outline(state, position, |pos| world.0.get(&pos).copied())
            .unwrap()
    };

    let mut world = Neighbours::default();
    world.0.insert(north, stone);
    let (boxes, edges) = outline(&world, sconce).unwrap();
    let expected: Vec<[f64; 6]> =
        serde_json::from_value(profile["palette"][palette_row]["outline"].clone()).unwrap();
    assert_eq!(boxes.as_ref(), expected.as_slice());
    assert_eq!(edges.len(), 12);
    let [x0, y0, z0, x1, _, _] = expected[0];
    assert!(edges.contains(&[x0, y0, z0, x1, y0, z0]));

    assert!(outline(&world, west_sconce).is_none());
    world.0.insert([21, 8, -3], stone);
    let (boxes, edges) = outline(&world, west_sconce).unwrap();
    assert!(boxes.is_empty());
    assert!(edges.is_empty());
    let (boxes, edges) = outline(&world, east_sconce).unwrap_or(&east_sconce_default);
    let expected: Vec<[f64; 6]> = serde_json::from_value(
        blocks["fixture:sconce"]["states"]["facing=east"]["outline"].clone(),
    )
    .unwrap();
    assert_eq!(boxes.as_ref(), expected.as_slice());
    assert_eq!(edges.len(), 12);

    world.0.insert(north, slab);
    world.0.insert(south, stone);
    for (state, original, expected) in [
        (sconce, &sconce_base, &sconce_row["outline"]),
        (
            north_sconce,
            &north_sconce_base,
            &blocks["fixture:sconce"]["states"]["facing=north"]["outline"],
        ),
        (
            slab,
            &slab_base,
            &blocks["fixture:slab"]["states"]["type=top"]["outline"],
        ),
    ] {
        let geometry = outline(&world, state);
        assert!(geometry.is_none());
        let (boxes, _) = geometry.unwrap_or(original);
        let expected: Vec<[f64; 6]> = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(boxes.as_ref(), expected.as_slice());
    }
}
