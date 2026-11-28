//! Before/after panels for the Create logistics and kinetic renderer batches.

use super::super::{
    angle_radians, arm, contraptions, levers, logistics, machine_state_from_nbt, specs,
    speed_from_nbt, spin_sign,
};
use super::{
    FixtureModels, TEMPLATE_CACHE_TESTS, machines, release_assets, render_release_comparison,
    wire_tag,
};
use glam::{Mat4, Quat, Vec3};
use horizon_assets::{
    item_bake::item_transform_matrix,
    item_model::DisplayContext,
    model::{DISPLAY_CONTEXTS, DisplayTransform},
};
use horizon_client::item_render::glam_matrix;
use horizon_client::world::{
    KineticAuxState, KineticContraptionState, KineticLogisticsState, LerpedValue,
};
use horizon_protocol::registry::Ids;
use horizon_world::nbt::Tag;
use std::{fs::File, path::Path};
use zip::ZipArchive;

const ITEM: &str = "create:shaft";
const ITEM_MODEL: &str = "create:item/shaft";
const STACK: &str = r#"{id:"create:shaft",count:1}"#;
const TIME: f64 = 11.0;
const TYPES: &[&str] = &[
    "create:belt",
    "create:andesite_tunnel",
    "create:brass_tunnel",
    "create:depot",
    "create:chute",
    "create:smart_chute",
    "create:mechanical_arm",
    "create:rope_pulley",
    "create:hose_pulley",
    "create:elevator_pulley",
    "create:analog_lever",
    "create:fluid_valve",
    "create:valve_handle",
    "create:toolbox",
    "create:packager",
    "create:schematicannon",
    "create:mechanical_roller",
    "create:chain_conveyor",
    "create:peculiar_bell",
    "create:haunted_bell",
    "create:desk_bell",
    "create:item_drain",
    "create:track_station",
    "create:funnel",
    "create:sticker",
    "create:bogey",
];

type Position = [i32; 3];

fn at(pos: Position) -> Vec3 {
    Vec3::from_array(pos.map(|coord| coord as f32))
}

fn slot(column: i32, row: i32) -> Position {
    [column * 3, 0, row * 7]
}

#[derive(Default)]
struct Frame {
    bodies: Vec<release_assets::Placement>,
    parts: Vec<(Position, machines::MachinePart)>,
    chains: Vec<(Position, contraptions::ChainSpan)>,
}

struct Panel<'a> {
    frame: Frame,
    active: bool,
    registry: &'a Ids,
    jar: &'a mut ZipArchive<File>,
}

impl Panel<'_> {
    fn body(&mut self, pos: Position, block: &str, props: &[(&str, &str)]) {
        self.body_moved(pos, block, props, Mat4::IDENTITY);
    }

    fn body_moved(&mut self, pos: Position, block: &str, props: &[(&str, &str)], local: Mat4) {
        self.frame.bodies.push(release_assets::block_body(
            self.jar,
            block,
            props,
            Mat4::from_translation(at(pos)) * local,
        ));
    }

    fn tag(&self, kind: &str, pos: Position, snbt: &str) -> Tag {
        wire_tag(self.registry, kind, pos, snbt)
    }

    fn parts(&mut self, pos: Position, parts: Vec<machines::MachinePart>) {
        self.frame
            .parts
            .extend(parts.into_iter().map(|part| (pos, part)));
    }

    fn item(&mut self, pos: Position, id: &str, local: Mat4) {
        assert_eq!(id, ITEM);
        self.parts(pos, vec![machines::part(ITEM_MODEL, local)]);
    }

    fn draws(
        &mut self,
        pos: Position,
        (parts, items): (Vec<machines::MachinePart>, Vec<logistics::WorldItemDraw>),
    ) {
        self.parts(pos, parts);
        for draw in items {
            assert_eq!(draw.display, DisplayContext::Fixed);
            for local in logistics::item_draw_matrices(&draw, true) {
                self.item(pos, &draw.stack.item, local);
            }
        }
    }

    fn time(&self) -> f64 {
        if self.active { TIME } else { 0.0 }
    }
}

fn logistics_rows(panel: &mut Panel<'_>) {
    let tick = logistics::LogisticsTickCtx {
        player_xz: None,
        facing: "south".to_owned(),
        blaze: "none".to_owned(),
        valid_block_above: false,
        mixer_running_above: false,
        belt_speed: 48.0,
    };
    let time = panel.time();
    for (column, slope, dyed) in [
        (0, "horizontal", false),
        (1, "upward", false),
        (2, "horizontal", true),
    ] {
        let pos = slot(column, 0);
        let speed = if panel.active { tick.belt_speed } else { 0.0 };
        let dye = if dyed { r#",Dye:"BLUE""# } else { "" };
        let tag = panel.tag("create:belt", pos, &format!(
            "{{Speed:{speed}f,IsController:1b,Length:2,Index:0,Inventory:{{Items:[{{Item:{STACK},Pos:0.3f,Offset:0f,InDirection:3}}]}}{dye}}}"
        ));
        let mut state = logistics::logistics_state_from_nbt(None, &tag).unwrap();
        if panel.active {
            for _ in 0..8 {
                logistics::tick_logistics_state(&mut state, pos, &tick);
            }
        }
        for (index, part) in ["start", "end"].into_iter().enumerate() {
            let [x, y, z] = pos;
            let index = index as i32;
            let segment = [x, y + if slope == "upward" { index } else { 0 }, z + index];
            panel.draws(
                segment,
                logistics::logistics_parts_and_items(&logistics::LogisticsBlock {
                    block: "create:belt",
                    facing: "south",
                    slope,
                    part,
                    shape: "",
                    blaze: "",
                    pos: segment,
                    speed: speed_from_nbt(&tag).unwrap(),
                    time,
                    alpha: 1.0,
                    logistics: (index == 0).then_some(&state),
                }),
            );
        }
    }

    for (column, block) in [(3, "create:andesite_tunnel"), (4, "create:brass_tunnel")] {
        let pos = slot(column, 0);
        panel.body(pos, block, &[("axis", "z"), ("shape", "straight")]);
        let mut state =
            logistics::logistics_state_from_nbt(None, &panel.tag(block, pos, "{Flaps:[2,3]}"))
                .unwrap();
        let KineticLogisticsState::BeltTunnel { flaps } = &mut state else {
            panic!("expected tunnel flaps");
        };
        for (_, flap) in flaps {
            if panel.active {
                logistics::kick_flap(flap, false);
            } else {
                flap.set_value(0.0);
            }
        }
        if panel.active {
            for _ in 0..8 {
                logistics::tick_logistics_state(&mut state, pos, &tick);
            }
        }
        panel.draws(
            pos,
            logistics::logistics_parts_and_items(&logistics::LogisticsBlock {
                block,
                facing: "south",
                slope: "",
                part: "",
                shape: "straight",
                blaze: "",
                pos,
                speed: 0.0,
                time,
                alpha: 1.0,
                logistics: Some(&state),
            }),
        );
    }

    for (column, block) in [
        (0, "create:depot"),
        (1, "create:chute"),
        (2, "create:smart_chute"),
    ] {
        let pos = slot(column, 1);
        let is_chute = block == "create:chute";
        let is_depot = block == "create:depot";
        let progress = match (block, panel.active) {
            ("create:smart_chute", true) => -0.25,
            (_, true) => 0.64,
            (_, false) => 0.25,
        };
        let snbt = if is_depot {
            panel.body(pos, block, &[]);
            format!(
                "{{OutputBuffer:{{Items:[]}},HeldItem:{{Item:{STACK},Pos:{progress}f,InDirection:3}}}}"
            )
        } else {
            if is_chute {
                panel.body(
                    pos,
                    block,
                    &[
                        ("facing", "down"),
                        ("shape", "window"),
                        ("waterlogged", "false"),
                    ],
                );
            } else {
                panel.body(
                    pos,
                    block,
                    &[("powered", if panel.active { "true" } else { "false" })],
                );
            }
            let pull = if is_chute { 0 } else { 64 };
            format!(
                "{{Item:{STACK},ItemPosition:{progress}f,Pull:{pull}f,BottomAirFlowDistance:1f}}"
            )
        };
        let state =
            logistics::logistics_state_from_nbt(None, &panel.tag(block, pos, &snbt)).unwrap();
        panel.draws(
            pos,
            logistics::logistics_parts_and_items(&logistics::LogisticsBlock {
                block,
                facing: if is_chute { "down" } else { "north" },
                slope: "",
                part: "",
                shape: if is_chute { "window" } else { "" },
                blaze: "",
                pos,
                speed: 0.0,
                time,
                alpha: 1.0,
                logistics: Some(&state),
            }),
        );
    }
}

fn machine_rows(panel: &mut Panel<'_>) {
    let time = panel.time();
    let block = "create:mechanical_arm";
    let pos = slot(3, 1);
    panel.body(pos, block, &[("ceiling", "false")]);
    let phase = if panel.active {
        "MOVE_TO_OUTPUT"
    } else {
        "SEARCH_INPUTS"
    };
    let progress = if panel.active { 0.72 } else { 0.0 };
    let held = if panel.active {
        format!(",HeldItem:{STACK}")
    } else {
        String::new()
    };
    let tag = panel.tag(block, pos, &format!(
        r#"{{Speed:32f,Phase:"{phase}",TargetPointIndex:{},MovementProgress:{progress}f,InteractionPoints:[{{Pos:[I;1,0,1],Type:"create:depot",Mode:"DEPOSIT"}}]{held}}}"#,
        if panel.active { 0 } else { -1 },
    ));
    let state = arm::apply_arm_nbt(None, pos, false, &tag, |_| None).unwrap();
    let machine = machines::Machine {
        block,
        facing: "north",
        axis: "z",
        along_first: false,
        ceiling: false,
        vertical: false,
        pos,
        speed: speed_from_nbt(&tag).unwrap(),
        time,
        alpha: 1.0,
        state: Default::default(),
        has_controller_bracket: false,
        arm: Some(&state),
        arm_has_item: state.held_item.is_some(),
        arm_is_block_item: state.held_item.is_some(),
    };
    let (parts, head) = machines::arm_parts_with_head(&machine);
    panel.parts(pos, parts);
    if let Some(stack) = state.held_item {
        panel.item(pos, &stack.item, arm::held_item_local_matrix(head, true));
    }

    for (column, block) in [
        (4, "create:rope_pulley"),
        (5, "create:hose_pulley"),
        (6, "create:elevator_pulley"),
    ] {
        let pos = slot(column, 1);
        let rope = block == "create:rope_pulley";
        let props = if rope {
            [("axis", "x")]
        } else {
            [("facing", "south")]
        };
        panel.body(pos, block, &props);
        let snbt = match (block, panel.active) {
            ("create:hose_pulley", true) => "{Speed:32f,Offset:{Value:2.15f,Force:1b}}",
            ("create:hose_pulley", false) => "{Speed:0f,Offset:{Value:0f,Force:1b}}",
            (_, true) => "{Speed:32f,Offset:2.15f,Running:1b}",
            (_, false) => "{Speed:0f,Offset:0f,Running:0b}",
        };
        let tag = panel.tag(block, pos, snbt);
        let machine = machines::Machine {
            block,
            facing: if rope { "north" } else { "south" },
            axis: if rope { "x" } else { "z" },
            along_first: false,
            ceiling: false,
            vertical: false,
            pos,
            speed: speed_from_nbt(&tag).unwrap(),
            time,
            alpha: 1.0,
            state: machine_state_from_nbt(&tag).unwrap(),
            has_controller_bracket: false,
            arm: None,
            arm_has_item: false,
            arm_is_block_item: false,
        };
        panel.parts(pos, machines::machine_parts(&machine));
    }
}

fn aux_rows(panel: &mut Panel<'_>) {
    for (pos, block, kind) in [
        (slot(5, 0), "create:andesite_funnel", "create:funnel"),
        (slot(6, 0), "create:brass_funnel", "create:funnel"),
        (slot(0, 2), "create:analog_lever", "create:analog_lever"),
        (slot(1, 2), "create:fluid_valve", "create:fluid_valve"),
        (slot(4, 2), "create:red_toolbox", "create:toolbox"),
        (slot(5, 2), "create:packager", "create:packager"),
        (slot(6, 2), "create:sticker", "create:sticker"),
    ] {
        let active = panel.active;
        let mut facing = "south";
        let mut extended = false;
        let mut enabled = false;
        let state = match kind {
            "create:funnel" => {
                panel.body(
                    pos,
                    block,
                    &[
                        ("facing", facing),
                        ("extracting", "true"),
                        ("powered", "false"),
                    ],
                );
                // The client receives flap kicks in a separate packet.
                let mut flap = logistics::new_flap();
                flap.set_value(0.0);
                if active {
                    logistics::kick_flap(&mut flap, false);
                }
                Some(KineticAuxState::Funnel { flap })
            }
            "create:analog_lever" => {
                panel.body(pos, block, &[("facing", facing), ("face", "floor")]);
                let initial = levers::aux_state_from_nbt(
                    None,
                    &panel.tag(kind, pos, "{State:0,ChangeTimer:0}"),
                );
                if active {
                    levers::aux_state_from_nbt(
                        initial.as_ref(),
                        &panel.tag(kind, pos, "{State:15,ChangeTimer:0}"),
                    )
                } else {
                    initial
                }
            }
            "create:fluid_valve" => {
                enabled = active;
                panel.body(
                    pos,
                    block,
                    &[
                        ("facing", facing),
                        ("axis_along_first", "false"),
                        ("enabled", bool_name(enabled)),
                    ],
                );
                levers::aux_state_from_nbt(
                    None,
                    &panel.tag(
                        kind,
                        pos,
                        if active {
                            "{Pointer:{Value:0f,Target:1f,Speed:0.125f}}"
                        } else {
                            "{Pointer:{Value:0f,Target:0f,Speed:0.125f}}"
                        },
                    ),
                )
            }
            "create:toolbox" => {
                panel.body(pos, block, &[("facing", facing)]);
                let initial =
                    levers::aux_state_from_nbt(None, &panel.tag(kind, pos, "{OpenCount:0}"));
                if active {
                    levers::aux_state_from_nbt(
                        initial.as_ref(),
                        &panel.tag(kind, pos, "{OpenCount:1}"),
                    )
                } else {
                    initial
                }
            }
            "create:packager" => {
                facing = "north";
                panel.body(
                    pos,
                    block,
                    &[
                        ("facing", facing),
                        ("linked", "false"),
                        ("powered", "false"),
                    ],
                );
                levers::aux_state_from_nbt(
                    None,
                    &panel.tag(
                        kind,
                        pos,
                        if active {
                            "{AnimationTicks:19,AnimationInward:1b}"
                        } else {
                            "{AnimationTicks:0,AnimationInward:1b}"
                        },
                    ),
                )
            }
            "create:sticker" => {
                extended = active;
                panel.body(
                    pos,
                    block,
                    &[
                        ("facing", facing),
                        ("extended", bool_name(extended)),
                        ("powered", bool_name(extended)),
                    ],
                );
                // Sticker extension is driven by the blockstate, like in the game.
                Some(KineticAuxState::Sticker {
                    piston: LerpedValue::start_with(0.0),
                })
            }
            _ => unreachable!(),
        };
        let mut state = state.unwrap_or_else(|| panic!("no animation state for {block}"));
        if active {
            for _ in 0..8 {
                levers::tick_aux_state(&mut state, extended);
            }
        }
        panel.draws(
            pos,
            levers::aux_parts_and_item(&levers::AuxBlock {
                block,
                facing,
                face: "floor",
                shape: "",
                along_first: false,
                enabled,
                extended,
                pos,
                speed: 0.0,
                time: panel.time(),
                alpha: 1.0,
                aux: Some(&state),
            }),
        );
    }

    for (column, block) in [
        (2, "create:copper_valve_handle"),
        (3, "create:blue_valve_handle"),
    ] {
        let pos = slot(column, 2);
        let update = panel.tag(
            "create:valve_handle",
            pos,
            if panel.active {
                "{Speed:32f}"
            } else {
                "{Speed:0f}"
            },
        );
        let [part] = specs::core_spec(block).unwrap().parts else {
            panic!("expected a single valve handle part");
        };
        let specs::Motion::Spin {
            axis: specs::AxisSource::BlockFacingAxis,
            speed_mult,
            tooth_phase: specs::ToothPhase::None,
        } = part.motion
        else {
            panic!("expected valve handle spin around its facing axis");
        };
        assert!(matches!(part.model, specs::PartModel::BlockState));
        let axis = Vec3::Z;
        let angle = angle_radians(
            panel.time(),
            speed_from_nbt(&update).unwrap() * speed_mult,
            0.0,
        );
        let local = Mat4::from_translation(part.pivot)
            * Mat4::from_quat(Quat::from_axis_angle(
                axis,
                angle * spin_sign(Quat::IDENTITY, axis),
            ))
            * Mat4::from_translation(-part.pivot);
        panel.body_moved(pos, block, &[("facing", "south")], local);
    }
}

fn bool_name(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn door_fixture_animation(block: &str, open: bool) -> (KineticContraptionState, f64) {
    let mut state = KineticContraptionState::SlidingDoor {
        animation: LerpedValue::start_with(0.0),
        bridge_ticks: 0,
    };
    let (ticks, alpha) = if matches!(block, "create:andesite_door" | "create:copper_door") {
        (4, 1.0 / 3.0) // Keep the fold visible halfway through the transition.
    } else {
        (7, 1.0) // Seven 0.15 steps settle the slide at its fully open position.
    };
    if open {
        let tick = contraptions::ContraptionTickCtx {
            open,
            visible: false,
            powered: false,
            assembling: false,
        };
        for _ in 0..ticks {
            contraptions::tick_contraption_state(&mut state, &tick);
        }
    }
    (state, alpha)
}

#[test]
fn sliding_doors_in_the_after_panel_reach_the_open_offset() {
    for block in [
        "create:train_door",
        "create:framed_glass_door",
        "create:brass_door",
    ] {
        let (state, alpha) = door_fixture_animation(block, true);
        let ctx = contraptions::ContraptionBlock {
            block,
            facing: "south",
            axis: "z",
            half: "lower",
            hinge: "left",
            attachment: "",
            open: true,
            visible: false,
            powered: false,
            assembling: false,
            pos: slot(0, 3),
            speed: 0.0,
            time: TIME,
            alpha,
            contraption: Some(&state),
        };
        let [(lower, bottom), (upper, top)] = contraptions::sliding_door_halves(&ctx).unwrap();
        assert_eq!((lower, upper), ("lower", "upper"));
        assert_eq!(bottom, Vec3::new(13.0 / 16.0, 0.0, 1.0 / 32.0));
        assert_eq!(top, bottom + Vec3::Y * (1.0 - 1.0 / 512.0));
    }
}

fn contraption_rows(panel: &mut Panel<'_>) {
    let active = panel.active;
    let time = panel.time();
    let chain_wheels = [slot(6, 3), slot(7, 3)];
    for (pos, block) in [
        (slot(7, 2), "create:schematicannon"),
        (slot(5, 3), "create:mechanical_roller"),
        (chain_wheels[0], "create:chain_conveyor"),
        (chain_wheels[1], "create:chain_conveyor"),
        (slot(0, 4), "create:peculiar_bell"),
        (slot(1, 4), "create:haunted_bell"),
        (slot(2, 4), "create:desk_bell"),
        (slot(3, 4), "create:item_drain"),
        (slot(4, 4), "create:track_station"),
        (slot(5, 4), "create:small_bogey"),
        (slot(6, 4), "create:large_bogey"),
    ] {
        let mut state = None;
        let mut speed = 0.0;
        let mut facing = "south";
        let mut powered = false;
        let mut assembling = false;
        let mut ticks = 0;
        let snbt = match block {
            "create:schematicannon" => {
                panel.body(pos, block, &[]);
                if active {
                    let [x, y, z] = pos;
                    format!(
                        "{{DefaultYaw:0f,Printer:{{CurrentPos:[I;{},{},{}]}},FlyingBlocks:[{{TotalTicks:20,TicksLeft:19}}]}}",
                        x + 3,
                        y + 2,
                        z + 2
                    )
                } else {
                    "{DefaultYaw:0f}".to_owned()
                }
            }
            "create:mechanical_roller" => {
                panel.body(pos, block, &[("facing", facing)]);
                if active { "{Speed:96f}" } else { "{Speed:0f}" }.to_owned()
            }
            "create:chain_conveyor" => {
                panel.body(pos, block, &[]);
                let dx = chain_wheels.iter().find(|&&wheel| wheel != pos).unwrap()[0] - pos[0];
                let rpm = if active { 32 } else { 0 };
                format!("{{Speed:{rpm}f,Connections:[[I;{dx},0,0]]}}")
            }
            "create:peculiar_bell" | "create:haunted_bell" => {
                panel.body(pos, block, &[("facing", facing), ("attachment", "floor")]);
                ticks = 5;
                if active { r#"{Ringing:"NORTH"}"# } else { "" }.to_owned()
            }
            "create:desk_bell" => {
                facing = "up";
                powered = active;
                panel.body(
                    pos,
                    block,
                    &[("facing", facing), ("powered", bool_name(powered))],
                );
                ticks = 7;
                if active { "{Ding:1b}" } else { "" }.to_owned()
            }
            "create:item_drain" => {
                panel.body(pos, block, &[]);
                ticks = if active { 8 } else { 0 };
                format!("{{ProcessingTicks:0,HeldItem:{{Item:{STACK},Pos:0.25f,InDirection:3}}}}")
            }
            "create:track_station" => {
                facing = "north"; // The station blockstate has no facing property.
                assembling = active;
                panel.body(pos, block, &[]);
                ticks = if active { 3 } else { 0 };
                "{TargetTrack:[I;0,0,1],OutputBuffer:{Items:[]}}".to_owned()
            }
            "create:small_bogey" | "create:large_bogey" => {
                panel.body(pos, block, &[("axis", "x")]);
                // The renderer uses the kinetic speed to turn both bogey sizes.
                let tag = panel.tag(
                    "create:bogey",
                    pos,
                    if active { "{Speed:32f}" } else { "{Speed:0f}" },
                );
                speed = speed_from_nbt(&tag).unwrap();
                String::new()
            }
            _ => unreachable!(),
        };
        if !snbt.is_empty() {
            let update = panel.tag(block, pos, &snbt);
            speed = speed_from_nbt(&update).unwrap();
            state = contraptions::contraption_state_from_nbt(None, &update);
            if block != "create:mechanical_roller" {
                assert!(state.is_some(), "no animation state for {block}");
            }
        }
        let tick = contraptions::ContraptionTickCtx {
            open: false,
            visible: true,
            powered,
            assembling,
        };
        if let Some(state) = &mut state {
            for _ in 0..ticks {
                contraptions::tick_contraption_state(state, &tick);
            }
        }
        let ctx = contraptions::ContraptionBlock {
            block,
            facing,
            axis: "x",
            half: "lower",
            hinge: "left",
            attachment: "floor",
            open: false,
            visible: true,
            powered,
            assembling,
            pos,
            speed,
            time,
            alpha: 1.0,
            contraption: state.as_ref(),
        };
        panel
            .frame
            .chains
            .extend(contraptions::chain_spans(&ctx).map(|span| (pos, span)));
        panel.draws(pos, contraptions::contraption_parts_and_items(&ctx));
    }

    for (column, block) in [
        "create:train_door",
        "create:framed_glass_door",
        "create:brass_door",
        "create:andesite_door",
        "create:copper_door",
    ]
    .into_iter()
    .enumerate()
    {
        let pos = slot(column as i32, 3);
        let facing = "south";
        let hinge = "left";
        if !active {
            for (dy, half) in ["lower", "upper"].into_iter().enumerate() {
                panel.body(
                    [pos[0], pos[1] + dy as i32, pos[2]],
                    block,
                    &[
                        ("facing", facing),
                        ("hinge", hinge),
                        ("half", half),
                        ("open", "false"),
                        ("visible", "true"),
                    ],
                );
            }
        }
        let (state, alpha) = door_fixture_animation(block, active);
        let ctx = contraptions::ContraptionBlock {
            block,
            facing,
            axis: "z",
            half: "lower",
            hinge,
            attachment: "",
            open: active,
            visible: !active,
            powered: false,
            assembling: false,
            pos,
            speed: 0.0,
            time,
            alpha,
            contraption: Some(&state),
        };
        if let Some(halves) = contraptions::sliding_door_halves(&ctx) {
            for (half, offset) in halves {
                panel.body_moved(
                    pos,
                    block,
                    &[
                        ("facing", facing),
                        ("hinge", hinge),
                        ("half", half),
                        ("open", "false"),
                        ("visible", "true"),
                    ],
                    Mat4::from_translation(offset),
                );
            }
        }
        panel.draws(pos, contraptions::contraption_parts_and_items(&ctx));
    }
}

pub(super) fn render() {
    let _cache = TEMPLATE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let jar =
        std::env::var_os("HORIZON_CREATE_FIXTURE_JAR").expect("path to the official Create JAR");
    let output = std::env::var_os("HORIZON_CREATE_BATCHES_PNG").expect("output PNG path");
    let mut jar = release_assets::open(Path::new(&jar));
    let vanilla = std::env::var_os("HORIZON_CREATE_FIXTURE_VANILLA_JAR")
        .expect("path to the Minecraft 1.21.1 client JAR");
    let vanilla =
        ZipArchive::new(File::open(vanilla).unwrap()).expect("Minecraft 1.21.1 client JAR");
    let registry = Ids::from_names(TYPES.iter().map(|name| (*name).to_owned()).collect());
    let frames = [false, true].map(|active| {
        let mut panel = Panel {
            active,
            registry: &registry,
            jar: &mut jar,
            frame: Frame::default(),
        };
        logistics_rows(&mut panel);
        machine_rows(&mut panel);
        aux_rows(&mut panel);
        contraption_rows(&mut panel);
        panel.frame
    });
    let [before, after] = frames;
    let bodies = [before.bodies, after.bodies];
    let parts = [before.parts, after.parts];
    let mut chains = [before.chains, after.chains].into_iter();
    let models: FixtureModels = release_assets::load_scene(
        &mut [("create", jar), ("minecraft", vanilla)].into(),
        bodies,
        parts.iter().flatten().map(|(_, part)| part.model),
        parts
            .iter()
            .flatten()
            .filter_map(|(_, part)| part.sprite_shift.map(|shift| shift.target))
            .chain([contraptions::CHAIN_SPRITE]),
    );
    let release = models.release.as_ref().unwrap();
    let item = models.library.resolve(ITEM_MODEL).unwrap();
    let fixed = DISPLAY_CONTEXTS
        .iter()
        .position(|name| *name == "fixed")
        .unwrap();
    let display = match &item.display.contexts[fixed] {
        DisplayTransform::Inherit => None,
        DisplayTransform::Explicit(transform) => transform.as_ref(),
    };
    let item_transform = glam_matrix(&item_transform_matrix(display, false));
    let mut frames = parts.into_iter();
    let vertices = [false, true].map(|active| {
        let mut out = Vec::new();
        release.draw_bodies(&models, active, &mut out);
        for (pos, mut part) in frames.next().unwrap() {
            if part.model == ITEM_MODEL {
                part.local *= item_transform;
            }
            models.parts(at(pos), [part], 0xffffff, &mut out);
        }
        for (pos, span) in chains.next().unwrap() {
            span.draw(
                at(pos),
                &models.sprites[contraptions::CHAIN_SPRITE],
                [super::LIGHT; 2],
                &mut out,
            );
        }
        assert!(out.iter().all(|v| Vec3::from(v.pos).is_finite()));
        assert_eq!(out.len() % 4, 0);
        out
    });
    let center = Vec3::new(11.0, 0.15, 14.5);
    let view = Mat4::look_at_rh(center + Vec3::new(2.0, 26.0, 30.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-12.75, 12.75, -12.0, 12.0, 0.1, 90.0) * view;
    render_release_comparison(
        release,
        vertices,
        view,
        view_proj,
        [1088, 1024],
        Path::new(&output),
    );
}
