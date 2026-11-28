//! Animation of placed Create 6.0.10 deployers from their block entity updates.

use super::facing_z_rotation;
use super::levers::{dir_normal_vec3, vertical_angle};
use super::logistics::{ItemPoseStyle, WorldItemDraw};
use super::machines::{MachinePart, centred, horizontal_angle, part};
use glam::{Mat4, Quat, Vec3};
use horizon_assets::item_model::DisplayContext;
use horizon_client::world::{DeployerMode, DeployerPhase, DeployerState};
use horizon_client_core::nbt_value::{float, number, text};
use horizon_items::item_nbt::item_from_nbt;
use horizon_world::nbt::Tag;
use std::f32::consts::FRAC_PI_2;

const POLE: &str = "create:block/deployer/pole";
const HAND_POINTING: &str = "create:block/deployer/hand_pointing";
const HAND_HOLDING: &str = "create:block/deployer/hand_holding";
const HAND_PUNCHING: &str = "create:block/deployer/hand_punching";
const STROKE_TIME: f32 = 1000.0;
const MAX_EXTENSION: f32 = 21.0 / 16.0;

pub(super) struct DeployerBlock<'a> {
    pub facing: &'a str,
    pub along_first: bool,
    pub speed: f32,
    pub time: f64,
    pub alpha: f64,
    pub deployer: Option<&'a DeployerState>,
    pub held_item_is_block: bool,
}

pub(super) fn is_deployer_be_tag(name: &str, tag: &Tag) -> bool {
    name == "create:deployer" && matches!(tag, Tag::Compound(fields) if !fields.is_empty())
}

pub(super) fn apply_deployer_tag(state: &mut DeployerState, tag: &Tag) {
    state.mode = match text(tag.get("Mode")).as_deref() {
        Some("USE") => DeployerMode::Use,
        _ => DeployerMode::Punch,
    };
    state.phase = match text(tag.get("State")).as_deref() {
        Some("EXPANDING") => DeployerPhase::Expanding,
        Some("RETRACTING") => DeployerPhase::Retracting,
        Some("DUMPING") => DeployerPhase::Dumping,
        _ => DeployerPhase::Waiting,
    };
    state.timer = number(tag.get("Timer"))
        .and_then(|timer| i32::try_from(timer).ok())
        .unwrap_or(0);
    state.reach = float(tag.get("Reach"))
        .map(|reach| reach as f32)
        .filter(|reach| reach.is_finite() && *reach >= 0.0)
        .unwrap_or(0.0);
    state.fist_bump = number(tag.get("Fistbump")).is_some_and(|value| value != 0);
    if let Some(item) = tag.get("HeldItem") {
        state.held_item = item_from_nbt(item);
    }
}

fn timer_rate(speed: f32) -> i32 {
    if speed == 0.0 {
        return 0;
    }
    (2.0 * speed.abs()).clamp(8.0, 512.0) as i32
}

pub(super) fn tick_deployer_state(state: &mut DeployerState, speed: f32) {
    // The block entity update changes phases. The client only runs its countdown.
    if state.timer > 0 {
        state.timer -= timer_rate(speed);
    }
}

fn hand_pose(state: Option<&DeployerState>) -> (&'static str, f32) {
    match state {
        Some(state) if state.mode == DeployerMode::Punch => (HAND_PUNCHING, 3.0 / 16.0),
        Some(state) if state.held_item.is_some() => (HAND_HOLDING, 4.0 / 16.0),
        _ => (HAND_POINTING, 0.0),
    }
}

fn hand_offset(state: Option<&DeployerState>, speed: f32, alpha: f32) -> f32 {
    let Some(deployer) = state else {
        return 0.0;
    };
    let remaining = (deployer.timer as f32 - alpha * timer_rate(speed) as f32) / STROKE_TIME;
    let progress = match deployer.phase {
        DeployerPhase::Expanding if deployer.fist_bump => (1.0 - remaining).powi(2),
        DeployerPhase::Expanding => 1.0 - remaining,
        DeployerPhase::Retracting => remaining,
        _ => return 0.0,
    };
    let (_, hand_length) = hand_pose(state);
    (progress.clamp(0.0, 1.0) * (deployer.reach + hand_length)).min(MAX_EXTENSION)
}

fn item_pose(ctx: &DeployerBlock<'_>, position: Vec3, punching: bool) -> (Mat4, DisplayContext) {
    let yaw = (horizontal_angle(ctx.facing) + 180.0).to_radians();
    let grip = Mat4::from_translation(position) * Mat4::from_rotation_y(yaw);
    if ctx.facing == "up" && ctx.speed == 0.0 && !punching {
        let (height, scale) = if ctx.held_item_is_block {
            (9.0 / 16.0, 1.25)
        } else {
            (11.0 / 16.0, 1.0)
        };
        let spin = Mat4::from_rotation_y((ctx.time % 360.0).to_radians() as f32);
        let stand = Mat4::from_translation(Vec3::Y * height) * Mat4::from_scale(Vec3::splat(scale));
        return (grip * stand * spin, DisplayContext::Ground);
    }
    let (height, depth, scale, display) = if punching {
        (
            1.0 / 8.0,
            12.0 / 16.0,
            0.75,
            DisplayContext::ThirdPersonRightHand,
        )
    } else {
        let scale = if ctx.held_item_is_block {
            47.0 / 64.0
        } else {
            0.5
        };
        (0.0, 11.0 / 16.0, scale, DisplayContext::Fixed)
    };
    let pitch = -vertical_angle(ctx.facing).to_radians();
    let local = grip
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_translation(Vec3::new(0.0, height, -depth))
        * Mat4::from_scale(Vec3::splat(scale));
    (local, display)
}

pub(super) fn deployer_parts_and_items(
    ctx: &DeployerBlock<'_>,
) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let distance = hand_offset(ctx.deployer, ctx.speed, ctx.alpha as f32);
    let travel = dir_normal_vec3(ctx.facing) * distance;
    let hand_rotation = facing_z_rotation(ctx.facing);
    let pole_rotation = if ctx.along_first ^ matches!(ctx.facing, "north" | "south") {
        hand_rotation * Quat::from_rotation_z(FRAC_PI_2)
    } else {
        hand_rotation
    };
    let translate = Mat4::from_translation(travel);
    let parts = vec![
        part(POLE, translate * centred(pole_rotation)),
        part(
            hand_pose(ctx.deployer).0,
            translate * centred(hand_rotation),
        ),
    ];
    let mut draws = Vec::new();
    if let Some(deployer) = ctx.deployer
        && let Some(stack) = &deployer.held_item
    {
        let (local, display) = item_pose(
            ctx,
            Vec3::splat(0.5) + travel,
            deployer.mode == DeployerMode::Punch,
        );
        draws.push(WorldItemDraw {
            stack: stack.clone(),
            local,
            light_offset: [0; 3],
            display,
            style: ItemPoseStyle::Raw,
        });
    }
    (parts, draws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client_core::nbt_value::string;

    fn item() -> Tag {
        Tag::compound([
            ("id", string("minecraft:iron_pickaxe")),
            ("count", Tag::Int(1)),
            (
                "components",
                Tag::compound([("minecraft:damage", Tag::Int(14))]),
            ),
        ])
    }

    fn ctx(state: Option<&DeployerState>) -> DeployerBlock<'_> {
        DeployerBlock {
            facing: "south",
            along_first: true,
            speed: 100.0,
            time: 0.0,
            alpha: 0.0,
            deployer: state,
            held_item_is_block: false,
        }
    }

    fn near(actual: Vec3, expected: Vec3) {
        assert!(
            actual.abs_diff_eq(expected, 1e-5),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn deployer_update_tag_controls_stroke_and_held_stack() {
        let mut state = DeployerState::default();
        let sync = Tag::compound([
            ("Mode", string("PUNCH")),
            ("State", string("EXPANDING")),
            ("Timer", Tag::Int(500)),
            ("Reach", Tag::Float(1.0_f32.to_bits())),
            ("Fistbump", Tag::Byte(1)),
            ("HeldItem", item()),
        ]);
        assert!(is_deployer_be_tag("create:deployer", &sync));
        assert!(!is_deployer_be_tag("create:mechanical_arm", &sync));
        apply_deployer_tag(&mut state, &sync);
        assert_eq!(state.held_item.as_ref().unwrap().damage, 14);
        let (parts, draws) = deployer_parts_and_items(&ctx(Some(&state)));
        assert_eq!(parts[1].model, HAND_PUNCHING);
        near(
            parts[1].local.transform_point3(Vec3::splat(0.5)),
            Vec3::new(0.5, 0.5, 0.796875),
        );
        let [draw] = draws.as_slice() else {
            panic!("expected one held item")
        };
        assert_eq!(draw.display, DisplayContext::ThirdPersonRightHand);
        near(
            draw.local.transform_point3(Vec3::ZERO),
            Vec3::new(0.5, 0.625, 1.546875),
        );

        let mode = Tag::compound([("Mode", string("USE"))]);
        apply_deployer_tag(&mut state, &mode);
        let (parts, draws) = deployer_parts_and_items(&ctx(Some(&state)));
        assert_eq!(parts[1].model, HAND_HOLDING);
        assert_eq!(draws[0].display, DisplayContext::Fixed);

        let empty = Tag::compound([
            ("Mode", string("USE")),
            ("HeldItem", Tag::Compound(Default::default())),
        ]);
        apply_deployer_tag(&mut state, &empty);
        let (parts, draws) = deployer_parts_and_items(&ctx(Some(&state)));
        assert_eq!(parts[1].model, HAND_POINTING);
        assert!(draws.is_empty());
    }

    #[test]
    fn deployer_interpolates_countdown_then_awaits_server_phase() {
        let (idle, _) = deployer_parts_and_items(&ctx(None));
        assert_eq!(idle[1].model, HAND_POINTING);
        near(
            idle[1].local.transform_point3(Vec3::splat(0.5)),
            Vec3::splat(0.5),
        );

        let mut state = DeployerState {
            phase: DeployerPhase::Expanding,
            timer: 1000,
            reach: 1.0,
            ..Default::default()
        };
        tick_deployer_state(&mut state, -50.0);
        tick_deployer_state(&mut state, -50.0);
        assert!((hand_offset(Some(&state), -50.0, 0.5) - 0.25).abs() < 1e-6);
        tick_deployer_state(&mut state, 0.0);
        assert!((hand_offset(Some(&state), 0.0, 0.5) - 0.20).abs() < 1e-6);
        for _ in 0..20 {
            tick_deployer_state(&mut state, 1024.0);
        }
        assert_eq!(state.phase, DeployerPhase::Expanding);
        assert_eq!(state.timer, -224);
        assert_eq!(hand_offset(Some(&state), 1024.0, 0.5), 1.0);

        apply_deployer_tag(
            &mut state,
            &Tag::compound([
                ("Mode", string("USE")),
                ("State", string("RETRACTING")),
                ("Timer", Tag::Int(8)),
                ("Reach", Tag::Float(1.0_f32.to_bits())),
            ]),
        );
        assert!((hand_offset(Some(&state), 0.5, 0.5) - 0.004).abs() < 1e-7);
        tick_deployer_state(&mut state, 0.5);
        assert_eq!(hand_offset(Some(&state), 0.5, 0.5), 0.0);
    }

    #[test]
    fn deployer_moves_all_placed_parts_along_the_facing_and_caps_reach() {
        let mut state = DeployerState::default();
        apply_deployer_tag(
            &mut state,
            &Tag::compound([
                ("Mode", string("USE")),
                ("State", string("RETRACTING")),
                ("Timer", Tag::Int(1000)),
                ("Reach", Tag::Float(1.25_f32.to_bits())),
                (
                    "HeldItem",
                    Tag::compound([("id", string("minecraft:stone"))]),
                ),
            ]),
        );
        let mut block = ctx(Some(&state));
        block.held_item_is_block = true;
        for (facing, dir) in [
            ("east", Vec3::X),
            ("west", Vec3::NEG_X),
            ("up", Vec3::Y),
            ("down", Vec3::NEG_Y),
            ("north", Vec3::NEG_Z),
            ("south", Vec3::Z),
        ] {
            block.facing = facing;
            let (parts, draws) = deployer_parts_and_items(&block);
            let center = Vec3::splat(0.5) + dir * (21.0 / 16.0);
            for part in &parts {
                near(part.local.transform_point3(Vec3::splat(0.5)), center);
                near(part.local.transform_vector3(Vec3::Z), dir);
            }
            assert_eq!(parts[0].model, POLE);
            assert_eq!(parts[1].model, HAND_HOLDING);
            near(
                draws[0].local.transform_point3(Vec3::ZERO),
                center + dir * (11.0 / 16.0),
            );
            near(
                draws[0].local.transform_vector3(Vec3::Z),
                -dir * (47.0 / 64.0),
            );
        }
        block.along_first = false;
        let (parts, _) = deployer_parts_and_items(&block);
        near(parts[0].local.transform_vector3(Vec3::X), Vec3::Y);
        near(parts[1].local.transform_vector3(Vec3::X), Vec3::X);
    }

    #[test]
    fn deployer_stopped_upright_use_displays_spinning_item() {
        let mut state = DeployerState::default();
        apply_deployer_tag(
            &mut state,
            &Tag::compound([
                ("Mode", string("USE")),
                ("State", string("WAITING")),
                (
                    "HeldItem",
                    Tag::compound([("id", string("minecraft:stone"))]),
                ),
            ]),
        );
        let block = DeployerBlock {
            facing: "up",
            speed: 0.0,
            time: 90.0,
            held_item_is_block: true,
            ..ctx(Some(&state))
        };
        let (_, draws) = deployer_parts_and_items(&block);
        assert_eq!(draws[0].display, DisplayContext::Ground);
        near(
            draws[0].local.transform_point3(Vec3::ZERO),
            Vec3::new(0.5, 1.0625, 0.5),
        );
        near(draws[0].local.transform_vector3(Vec3::X), Vec3::Z * 1.25);
    }
}
