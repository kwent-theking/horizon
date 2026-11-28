use super::{FluidDraw, apply_fluid_tag};
use glam::Vec3;
use horizon_client::{
    hud::Rect,
    mesher::Vertex,
    world::{CreateFluidState, World},
};
use horizon_world::{nbt::Tag, registry::BlockStateId};
use std::collections::HashMap;

const SPOUT_POS: [i32; 3] = [4, 1, 0];
const CONTAINERS: [(&str, [i32; 3], Option<&str>); 5] = [
    ("create:fluid_tank", [0, 0, 0], None),
    ("create:basin", [2, 0, 0], None),
    ("create:spout", SPOUT_POS, None),
    ("create:item_drain", [6, 0, 0], None),
    ("create:glass_fluid_pipe", [8, 0, 0], Some("x")),
];
const EMPTY_TANK: &str = "{Window:1b,Size:1,Height:1,TankContent:{}}";
const EMPTY_VESSEL: &str = "{Tanks:[{TankContent:{}}],ProcessingTicks:-1}";
const SYNTHETIC_FILLED: [&str; 5] = [
    r#"{Window:1b,Size:1,Height:1,TankContent:{Fluid:{id:"minecraft:water",amount:4000}}}"#,
    r#"{InputTanks:[{TankContent:{Fluid:{id:"minecraft:water",amount:250}}}],OutputTanks:[{TankContent:{Fluid:{id:"create:honey",amount:750}}}]}"#,
    r#"{Tanks:[{TankContent:{Fluid:{id:"create:honey",amount:500}}}],ProcessingTicks:10}"#,
    r#"{Tanks:[{TankContent:{Fluid:{id:"minecraft:lava",amount:750}}}],ProcessingTicks:10}"#,
    r#"{west:{Flow:{Fluid:{id:"minecraft:water",amount:1},In:1b}},east:{Flow:{Fluid:{id:"minecraft:water",amount:1},In:0b,Progress:{Value:0.5f}}}}"#,
];
const CREATE_FILLED: [&str; 5] = [
    r#"{Window:1b,Size:1,Height:1,TankContent:{Fluid:{id:"create:honey",amount:4000}}}"#,
    r#"{InputTanks:[{TankContent:{Fluid:{id:"create:honey",amount:250}}}],OutputTanks:[{TankContent:{Fluid:{id:"create:chocolate",amount:750}}}]}"#,
    r#"{Tanks:[{TankContent:{Fluid:{id:"create:honey",amount:500}}}],ProcessingTicks:10}"#,
    r#"{Tanks:[{TankContent:{Fluid:{id:"create:chocolate",amount:750}}}],ProcessingTicks:10}"#,
    r#"{west:{Flow:{Fluid:{id:"create:potion",amount:1,components:{"minecraft:potion_contents":{custom_color:4159204}}},In:1b}},east:{Flow:{Fluid:{id:"create:potion",amount:1,components:{"minecraft:potion_contents":{custom_color:4159204}}},In:0b,Progress:{Value:0.5f}}}}"#,
];

fn atlas() -> HashMap<String, Rect> {
    [
        ("block/water_still", 0.0),
        ("block/water_flow", 0.25),
        ("create:fluid/honey_still", 0.5),
        ("create:fluid/honey_flow", 0.5),
        ("block/lava_still", 0.75),
        ("block/lava_flow", 0.75),
    ]
    .map(|(name, u0)| {
        (
            name.to_owned(),
            Rect {
                u0,
                v0: 0.0,
                u1: u0 + 0.25,
                v1: 1.0,
                w: 16,
                h: 16,
            },
        )
    })
    .into_iter()
    .collect()
}

fn vertices(world: &World, atlas: &HashMap<String, Rect>) -> Vec<Vertex> {
    let mut output = Vec::new();
    let mut draw = FluidDraw::new(atlas, 1.0, &mut output);
    for (block, [x, y, z], axis) in CONTAINERS {
        if let Some(state) = world.create_fluids.get(&(x, y, z)) {
            draw.emit(
                block,
                axis,
                state,
                Vec3::new(x as f32, y as f32, z as f32),
                0xf0,
            );
        }
    }
    output
}

pub(in crate::game::kinetics) fn draw(
    tag: impl Fn(&str, [i32; 3], &str) -> Tag,
) -> (Vec<Vertex>, Vec<Vertex>) {
    let (before, after, _) = draw_scene(tag, &atlas(), SYNTHETIC_FILLED, "block/water_flow");
    (before, after)
}

pub(in crate::game::kinetics) fn draw_with_atlas(
    tag: impl Fn(&str, [i32; 3], &str) -> Tag,
    atlas: &HashMap<String, Rect>,
) -> (Vec<Vertex>, Vec<Vertex>, CreateFluidState) {
    draw_scene(tag, atlas, CREATE_FILLED, "create:fluid/potion_flow")
}

fn draw_scene(
    tag: impl Fn(&str, [i32; 3], &str) -> Tag,
    atlas: &HashMap<String, Rect>,
    filled_tags: [&str; 5],
    pipe_flow_sprite: &str,
) -> (Vec<Vertex>, Vec<Vertex>, CreateFluidState) {
    let mut world = World::new(BlockStateId(0), 0);
    let empty_tags = [
        EMPTY_TANK,
        "{InputTanks:[{TankContent:{}}],OutputTanks:[{TankContent:{}}]}",
        EMPTY_VESSEL,
        EMPTY_VESSEL,
        "{west:{},east:{}}",
    ];
    for ((block, pos @ [x, y, z], _), snbt) in CONTAINERS.into_iter().zip(empty_tags) {
        assert!(apply_fluid_tag(
            &mut world,
            (x, y, z),
            block,
            block,
            &tag(block, pos, snbt)
        ));
    }
    assert_eq!(world.create_fluids.len(), CONTAINERS.len());
    let before = vertices(&world, atlas);
    assert!(before.is_empty());

    for ((block, pos @ [x, y, z], _), snbt) in CONTAINERS.into_iter().zip(filled_tags) {
        assert!(apply_fluid_tag(
            &mut world,
            (x, y, z),
            block,
            block,
            &tag(block, pos, snbt)
        ));
    }
    let after = vertices(&world, atlas);
    for ((block, [x, _, _], _), expected) in CONTAINERS.into_iter().zip([24, 40, 64, 48, 40]) {
        let x = x as f32;
        let count = after
            .iter()
            .filter(|v| (x..=x + 1.0).contains(&v.pos[0]))
            .count();
        assert_eq!(count, expected, "vertices for {block}");
    }
    let tank_top = after
        .iter()
        .filter(|v| v.pos[0] < 1.0)
        .map(|v| v.pos[1])
        .fold(0.0, f32::max);
    assert_eq!(tank_top, 17.0 / 32.0);
    assert!(
        after
            .iter()
            .any(|v| (4.0..5.0).contains(&v.pos[0]) && v.pos[1] < 0.0)
    );
    let flow = atlas[pipe_flow_sprite];
    assert!(after.iter().any(|v| {
        v.pos[0] == 8.75
            && (flow.u0..flow.u1).contains(&v.uv[0])
            && (flow.v0..=flow.v1).contains(&v.uv[1])
    }));

    let tank_pos = (0, 0, 0);
    let placed_tank = world.create_fluids[&tank_pos].clone();
    let wrong_type = tag(
        "create:basin",
        [0, 0, 0],
        r#"{Window:1b,Size:1,Height:1,TankContent:{Fluid:{id:"minecraft:lava",amount:8000}}}"#,
    );
    assert!(!apply_fluid_tag(
        &mut world,
        tank_pos,
        "create:fluid_tank",
        "create:basin",
        &wrong_type
    ));
    assert_eq!(world.create_fluids[&tank_pos], placed_tank);

    let drain = tag("create:fluid_tank", [0, 0, 0], EMPTY_TANK);
    assert!(apply_fluid_tag(
        &mut world,
        tank_pos,
        "create:fluid_tank",
        "create:fluid_tank",
        &drain
    ));
    let drained = vertices(&world, atlas);
    assert_eq!(drained.len() + 24, after.len());
    assert!(drained.iter().all(|v| v.pos[0] >= 2.0));
    let [x, y, z] = SPOUT_POS;
    let spout = world
        .create_fluids
        .remove(&(x, y, z))
        .expect("fixture spout state");
    (before, after, spout)
}

#[test]
fn create_fluid_scene_uses_the_supplied_uvs() {
    let atlas: HashMap<_, _> = [
        "create:fluid/honey_still",
        "create:fluid/honey_flow",
        "create:fluid/chocolate_still",
        "create:fluid/chocolate_flow",
        "create:fluid/potion_still",
        "create:fluid/potion_flow",
    ]
    .into_iter()
    .enumerate()
    .map(|(column, name)| {
        let u0 = (column + 1) as f32 / 8.0;
        (
            name.to_owned(),
            Rect {
                u0,
                v0: 0.25,
                u1: u0 + 0.125,
                v1: 0.5,
                w: 16,
                h: 16,
            },
        )
    })
    .collect();
    let (before, after, spout) = draw_with_atlas(
        |_, _, snbt| horizon_world::nbt::parse_snbt(snbt).unwrap().0,
        &atlas,
    );
    assert!(before.is_empty());
    assert_eq!(after.len(), 216);
    assert!(after.iter().all(|v| (0.25..=0.5).contains(&v.uv[1])));
    assert!(matches!(
        spout,
        CreateFluidState::Vessel { processing: 10, .. }
    ));
}
