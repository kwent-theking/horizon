//! Create mechanical arm inverse kinematics, NBT parsing and per-tick pose
//! interpolation.
//!
//! Ported from `Creators-of-Create/Create` (`ArmAngleTarget`, `ArmBlockEntity`,
//! `ArmRenderer`, `ArmInteractionPoint`, `AllArmInteractionPointTypes`, MIT) and
//! `Creators-of-Create/Ponder` (`AngleHelper`, `VecHelper`, `Color`, MIT).

use super::opposite_facing;
use glam::{DVec3, Mat4, Vec3};
use horizon_client::world::{ArmAngleSet, ArmAnimState, ArmPhase, ArmPoint, ArmPointKind};
use horizon_world::nbt::{JavaString, Tag};
use std::f32::consts::FRAC_PI_2;

const LOWER_ARM_LENGTH: f64 = 14.0 / 16.0;
const UPPER_ARM_LENGTH: f64 = 15.0 / 16.0;

/// Render angles for one frame of a Create mechanical arm (`ArmRenderer`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ArmRenderPose {
    pub base: f32,
    pub lower: f32,
    pub upper: f32,
    pub head: f32,
    pub rave_tint: Option<u32>,
}

/// `AngleHelper.getShortestAngleDiff(current, target)` in degrees.
pub(super) fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let current = current % 360.0;
    let target = target % 360.0;
    (((((target - current) % 360.0) + 540.0) % 360.0) - 180.0) as f32
}

/// `AngleHelper.angleLerp(pct, current, target)` in degrees.
pub(super) fn angle_lerp(pct: f64, current: f64, target: f64) -> f32 {
    (current + f64::from(shortest_angle_diff(current, target)) * pct) as f32
}

pub(super) fn lerp_f32(pct: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * pct
}

/// `VecHelper.rotate(vec, deg, axis)`.
pub(super) fn rotate_vec_deg(v: DVec3, deg: f64, axis: char) -> DVec3 {
    if deg == 0.0 || v == DVec3::ZERO {
        return v;
    }
    let angle = (deg as f32 / 180.0 * std::f32::consts::PI) as f64;
    let sin = (angle as f32).sin() as f64;
    let cos = (angle as f32).cos() as f64;
    match axis {
        'x' => DVec3::new(v.x, v.y * cos - v.z * sin, v.z * cos + v.y * sin),
        'y' => DVec3::new(v.x * cos + v.z * sin, v.y, v.z * cos - v.x * sin),
        'z' => DVec3::new(v.x * cos - v.y * sin, v.y * cos + v.x * sin, v.z),
        _ => v,
    }
}

fn dir_normal(facing: &str) -> DVec3 {
    match facing {
        "down" => DVec3::NEG_Y,
        "up" => DVec3::Y,
        "north" => DVec3::NEG_Z,
        "south" => DVec3::Z,
        "west" => DVec3::NEG_X,
        "east" => DVec3::X,
        _ => DVec3::ZERO,
    }
}

/// Maps `AllArmInteractionPointTypes` registry IDs to target geometry rules.
pub(super) fn point_kind_from_type(id: &str) -> ArmPointKind {
    match id {
        "create:chute" | "create:jukebox" | "create:respawn_anchor" | "create:crushing_wheels" => {
            ArmPointKind::TopFace
        }
        "create:depot" | "create:belt" | "create:saw" => ArmPointKind::Depot,
        "create:composter" => ArmPointKind::Composter,
        "create:crafter" => ArmPointKind::Crafter,
        "create:deployer" => ArmPointKind::Deployer,
        "create:funnel" => ArmPointKind::Funnel,
        _ => ArmPointKind::Default,
    }
}

/// `ArmInteractionPoint.getInteractionPositionVector` and `getInteractionDirection`.
pub(super) fn point_target_and_facing(
    point: ArmPoint,
    arm_pos: [i32; 3],
    block_facing: Option<&str>,
) -> (DVec3, &'static str) {
    let world_pos = [
        arm_pos[0] + point.rel_pos[0],
        arm_pos[1] + point.rel_pos[1],
        arm_pos[2] + point.rel_pos[2],
    ];
    let center = DVec3::new(
        world_pos[0] as f64 + 0.5,
        world_pos[1] as f64 + 0.5,
        world_pos[2] as f64 + 0.5,
    );
    match point.kind {
        ArmPointKind::Default => (center, "down"),
        ArmPointKind::TopFace => (
            DVec3::new(center.x, world_pos[1] as f64 + 1.0, center.z),
            "down",
        ),
        ArmPointKind::Depot => (
            DVec3::new(center.x, world_pos[1] as f64 + 14.0 / 16.0, center.z),
            "down",
        ),
        ArmPointKind::Composter => (
            DVec3::new(center.x, world_pos[1] as f64 + 13.0 / 16.0, center.z),
            "down",
        ),
        ArmPointKind::Crafter => {
            let dir = canonical_opposite(block_facing.unwrap_or("south"));
            (center + dir_normal(dir) * 0.5, dir)
        }
        ArmPointKind::Deployer => {
            let dir = canonical_opposite(block_facing.unwrap_or("up"));
            (center + dir_normal(dir) * 0.65, dir)
        }
        ArmPointKind::Funnel => match block_facing {
            Some(facing) => (
                center + dir_normal(facing) * -0.15,
                canonical_opposite(facing),
            ),
            None => (center, "up"),
        },
    }
}

fn canonical_opposite(facing: &str) -> &'static str {
    match opposite_facing(facing) {
        "down" => "down",
        "up" => "up",
        "north" => "north",
        "south" => "south",
        "west" => "west",
        "east" => "east",
        _ => "down",
    }
}

/// `ArmAngleTarget(BlockPos armPos, Vec3 pointTarget, Direction clawFacing, boolean ceiling)`.
pub(super) fn compute_arm_angles(
    arm_pos: [i32; 3],
    point_target: DVec3,
    claw_facing: &str,
    ceiling: bool,
) -> ArmAngleSet {
    let origin = DVec3::new(
        arm_pos[0] as f64 + 0.5,
        arm_pos[1] as f64 + 0.5 + if ceiling { -6.0 / 16.0 } else { 6.0 / 16.0 },
        arm_pos[2] as f64 + 0.5,
    );
    let claw_target = point_target;
    let target = point_target + dir_normal(canonical_opposite(claw_facing)) * 0.5;

    let mut diff = target - origin;
    let horizontal_distance = (diff.x * diff.x + diff.z * diff.z).sqrt();

    let mut base_angle = diff.x.atan2(diff.z).to_degrees() + 180.0;
    if ceiling {
        diff.y = -diff.y;
        base_angle = 180.0 - base_angle;
    }

    let alpha_offset = diff.y.atan2(horizontal_distance).to_degrees();

    let a = LOWER_ARM_LENGTH;
    let a2 = a * a;
    let b = UPPER_ARM_LENGTH;
    let b2 = b * b;
    let diff_length = (diff.y * diff.y + horizontal_distance * horizontal_distance)
        .sqrt()
        .clamp(1.0 / 8.0, a + b);
    let diff_length2 = diff_length * diff_length;

    let alpha_ratio = (-b2 + a2 + diff_length2) / (2.0 * a * diff_length);
    let mut alpha = alpha_ratio.acos().to_degrees() + alpha_offset;
    let beta_ratio = (-diff_length2 + a2 + b2) / (2.0 * b * a);
    let mut beta = beta_ratio.acos().to_degrees();

    if alpha.is_nan() {
        alpha = 0.0;
    }
    if beta.is_nan() {
        beta = 0.0;
    }

    let mut head_pos = DVec3::ZERO;
    head_pos = rotate_vec_deg(head_pos + DVec3::new(0.0, b, 0.0), beta + 180.0, 'x');
    head_pos = rotate_vec_deg(head_pos + DVec3::new(0.0, a, 0.0), alpha - 90.0, 'x');
    head_pos = rotate_vec_deg(head_pos, base_angle, 'y');
    if ceiling {
        head_pos = rotate_vec_deg(head_pos, 180.0, 'x');
    }
    head_pos += origin;

    let mut head_diff = claw_target - head_pos;
    if ceiling {
        head_diff.y = -head_diff.y;
    }
    let horizontal_head_distance = (head_diff.x * head_diff.x + head_diff.z * head_diff.z).sqrt();
    let head_angle =
        alpha + beta + 135.0 - head_diff.y.atan2(horizontal_head_distance).to_degrees();

    ArmAngleSet {
        base: base_angle as f32,
        lower: alpha as f32,
        upper: beta as f32,
        head: (-head_angle) as f32,
    }
}

/// Reads an NBT block position (`NbtUtils.writeBlockPos` IntArray or legacy `{X, Y, Z}` compound).
pub(super) fn read_block_pos(tag: &Tag) -> Option<[i32; 3]> {
    match tag {
        Tag::IntArray(coords) if coords.len() >= 3 => Some([coords[0], coords[1], coords[2]]),
        Tag::Compound(fields) => {
            let int = |upper: &str, lower: &str| match fields
                .get(&JavaString::from(upper))
                .or_else(|| fields.get(&JavaString::from(lower)))
            {
                Some(Tag::Int(v)) => Some(*v),
                _ => None,
            };
            Some([int("X", "x")?, int("Y", "y")?, int("Z", "z")?])
        }
        _ => None,
    }
}

/// Whether `tag` carries mechanical arm fields (`ArmBlockEntity.write`).
pub(super) fn is_arm_tag(tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    fields.contains_key(&JavaString::from("Phase"))
        || fields.contains_key(&JavaString::from("InteractionPoints"))
}

fn parse_phase(value: Option<&Tag>) -> ArmPhase {
    let Some(Tag::String(raw)) = value else {
        return ArmPhase::SearchInputs;
    };
    let name = raw.to_string_lossy();
    if name.eq_ignore_ascii_case("MOVE_TO_INPUT") {
        ArmPhase::MoveToInput
    } else if name.eq_ignore_ascii_case("SEARCH_OUTPUTS") {
        ArmPhase::SearchOutputs
    } else if name.eq_ignore_ascii_case("MOVE_TO_OUTPUT") {
        ArmPhase::MoveToOutput
    } else if name.eq_ignore_ascii_case("DANCING") {
        ArmPhase::Dancing
    } else {
        ArmPhase::SearchInputs
    }
}

/// Parsed fields from an `ArmBlockEntity` update tag.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ParsedArmNbt {
    pub phase: ArmPhase,
    pub target_index: i32,
    pub progress: f32,
    pub goggles: bool,
    pub inputs: Vec<ArmPoint>,
    pub outputs: Vec<ArmPoint>,
    pub held_item: Option<horizon_items::items::ItemStack>,
}

pub(super) fn parse_arm_nbt(tag: &Tag) -> Option<ParsedArmNbt> {
    if !is_arm_tag(tag) {
        return None;
    }
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let phase = parse_phase(fields.get(&JavaString::from("Phase")));
    let target_index = match fields.get(&JavaString::from("TargetPointIndex")) {
        Some(Tag::Int(v)) => *v,
        _ => -1,
    };
    let progress = match fields.get(&JavaString::from("MovementProgress")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    let goggles = matches!(fields.get(&JavaString::from("Goggles")), Some(Tag::Byte(v)) if *v != 0);
    let held_item = fields
        .get(&JavaString::from("HeldItem"))
        .and_then(horizon_items::item_nbt::item_from_nbt);

    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    if let Some(Tag::List(points)) = fields.get(&JavaString::from("InteractionPoints")) {
        for entry in points {
            let Tag::Compound(point) = entry else {
                continue;
            };
            let Some(rel_pos) = point.get(&JavaString::from("Pos")).and_then(read_block_pos) else {
                continue;
            };
            let kind = match point.get(&JavaString::from("Type")) {
                Some(Tag::String(t)) => point_kind_from_type(&t.to_string_lossy()),
                _ => ArmPointKind::Default,
            };
            let is_deposit = match point.get(&JavaString::from("Mode")) {
                Some(Tag::String(m)) => m.to_string_lossy().eq_ignore_ascii_case("DEPOSIT"),
                _ => false,
            };
            let arm_point = ArmPoint { rel_pos, kind };
            if is_deposit {
                outputs.push(arm_point);
            } else {
                inputs.push(arm_point);
            }
        }
    }

    Some(ParsedArmNbt {
        phase,
        target_index,
        progress,
        goggles,
        inputs,
        outputs,
        held_item,
    })
}

pub(super) fn targeted_point(
    phase: ArmPhase,
    target_index: i32,
    inputs: &[ArmPoint],
    outputs: &[ArmPoint],
) -> Option<ArmPoint> {
    let idx = usize::try_from(target_index).ok()?;
    match phase {
        ArmPhase::MoveToInput => inputs.get(idx).copied(),
        ArmPhase::MoveToOutput => outputs.get(idx).copied(),
        _ => None,
    }
}

fn target_angles_for_point(
    point: ArmPoint,
    arm_pos: [i32; 3],
    ceiling: bool,
    lookup_facing: &impl Fn([i32; 3]) -> Option<String>,
) -> ArmAngleSet {
    let world_pos = [
        arm_pos[0] + point.rel_pos[0],
        arm_pos[1] + point.rel_pos[1],
        arm_pos[2] + point.rel_pos[2],
    ];
    let facing = match point.kind {
        ArmPointKind::Crafter | ArmPointKind::Deployer | ArmPointKind::Funnel => {
            lookup_facing(world_pos)
        }
        _ => None,
    };
    let (target_vec, claw_facing) = point_target_and_facing(point, arm_pos, facing.as_deref());
    compute_arm_angles(arm_pos, target_vec, claw_facing, ceiling)
}

fn step_angles(
    chased_progress: f32,
    previous_target: ArmAngleSet,
    previous_base_angle: f32,
    target: Option<ArmAngleSet>,
) -> ArmAngleSet {
    let base = angle_lerp(
        f64::from(chased_progress),
        f64::from(previous_base_angle),
        f64::from(target.map_or(previous_base_angle, |t| t.base)),
    );

    let (prev_t, next_t) = if chased_progress < 0.5 {
        (previous_target, ArmAngleSet::NO_TARGET)
    } else {
        (
            ArmAngleSet::NO_TARGET,
            target.unwrap_or(ArmAngleSet::NO_TARGET),
        )
    };
    let sub = if chased_progress >= 1.0 {
        1.0
    } else {
        (chased_progress % 0.5) * 2.0
    };

    let lower = lerp_f32(sub, prev_t.lower, next_t.lower);
    let upper = lerp_f32(sub, prev_t.upper, next_t.upper);
    let head = angle_lerp(
        f64::from(sub),
        f64::from(prev_t.head % 360.0),
        f64::from(next_t.head % 360.0),
    );
    ArmAngleSet {
        base,
        lower,
        upper,
        head,
    }
}

/// Applies an `ArmBlockEntity` update tag, preserving transition targets like `ArmBlockEntity.read`.
pub(super) fn apply_arm_nbt(
    existing: Option<&ArmAnimState>,
    arm_pos: [i32; 3],
    ceiling: bool,
    tag: &Tag,
    lookup_facing: impl Fn([i32; 3]) -> Option<String>,
) -> Option<ArmAnimState> {
    let parsed = parse_arm_nbt(tag)?;
    let mut state = match existing {
        Some(old) => {
            let mut next = old.clone();
            if old.target_index != parsed.target_index || old.phase != parsed.phase {
                let prev_point =
                    targeted_point(old.phase, old.target_index, &old.inputs, &old.outputs);
                match prev_point {
                    Some(point) => {
                        let prev_angles =
                            target_angles_for_point(point, arm_pos, ceiling, &lookup_facing);
                        next.previous_target = prev_angles;
                        next.previous_base_angle = prev_angles.base;
                    }
                    None => {
                        next.previous_target = ArmAngleSet::NO_TARGET;
                    }
                }
            }
            next
        }
        None => ArmAnimState::default(),
    };

    state.phase = parsed.phase;
    state.target_index = parsed.target_index;
    state.progress = parsed.progress.clamp(0.0, 1.0);
    state.goggles = parsed.goggles;
    state.inputs = parsed.inputs;
    state.outputs = parsed.outputs;
    state.held_item = parsed.held_item;

    if existing.is_none() {
        let target = targeted_point(
            state.phase,
            state.target_index,
            &state.inputs,
            &state.outputs,
        )
        .map(|point| target_angles_for_point(point, arm_pos, ceiling, &lookup_facing));
        if let Some(t) = target {
            state.previous_base_angle = t.base;
        }
        let pose = step_angles(
            state.progress,
            state.previous_target,
            state.previous_base_angle,
            target,
        );
        state.prev_angles = pose;
        state.curr_angles = pose;
        state.initialized = true;
    }

    Some(state)
}

/// `ArmBlockEntity.tickMovementProgress` on the client.
pub(super) fn tick_arm_state(
    arm: &mut ArmAnimState,
    arm_pos: [i32; 3],
    speed: f32,
    ceiling: bool,
    lookup_facing: impl Fn([i32; 3]) -> Option<String>,
) {
    arm.progress = (arm.progress + speed.abs().min(256.0) / 1024.0).min(1.0);

    let target = targeted_point(arm.phase, arm.target_index, &arm.inputs, &arm.outputs)
        .map(|point| target_angles_for_point(point, arm_pos, ceiling, &lookup_facing));

    let next = step_angles(
        arm.progress,
        arm.previous_target,
        arm.previous_base_angle,
        target,
    );
    if !arm.initialized {
        arm.prev_angles = next;
        arm.curr_angles = next;
        arm.initialized = true;
    } else {
        arm.prev_angles = arm.curr_angles;
        arm.curr_angles = next;
    }
}

fn color_in_phase(phase: i32, progress: i32) -> u32 {
    match phase.rem_euclid(6) {
        0 | 1 => 0,
        2 => progress as u32,
        3 | 4 => 255,
        _ => (255 - progress) as u32,
    }
}

/// `Color.rainbowColor(timeStep)` returning 0xRRGGBB.
pub(super) fn rainbow_rgb(time_step: i64) -> u32 {
    let local = (time_step.unsigned_abs() % 1536) as i32;
    let progress = local % 256;
    let phase_blue = local / 256;
    let red = color_in_phase(phase_blue + 4, progress);
    let green = color_in_phase(phase_blue + 2, progress);
    let blue = color_in_phase(phase_blue, progress);
    (red << 16) | (green << 8) | blue
}

fn pos_hash_mod_64(pos: [i32; 3]) -> i32 {
    let mut h = pos[0].wrapping_mul(73856093)
        ^ pos[1].wrapping_mul(19349663)
        ^ pos[2].wrapping_mul(83492791);
    if h == i32::MIN {
        h = 0;
    }
    h.abs() % 64
}

/// Computes renderer joint angles (`ArmRenderer.renderSafe`) for `arm` at `alpha` partial tick.
pub(super) fn arm_render_pose(
    arm: Option<&ArmAnimState>,
    arm_pos: [i32; 3],
    speed: f32,
    time: f64,
    alpha: f64,
) -> ArmRenderPose {
    let Some(arm) = arm else {
        return ArmRenderPose {
            base: ArmAngleSet::NO_TARGET.base,
            lower: ArmAngleSet::NO_TARGET.lower - 135.0,
            upper: ArmAngleSet::NO_TARGET.upper - 90.0,
            head: ArmAngleSet::NO_TARGET.head,
            rave_tint: None,
        };
    };

    if arm.phase == ArmPhase::Dancing && speed != 0.0 {
        let render_tick = time as f32 + pos_hash_mod_64(arm_pos) as f32;
        let base = (render_tick * 10.0) % 360.0;
        let lower = lerp_f32(((render_tick / 4.0).sin() + 1.0) / 2.0, -45.0, 15.0);
        let upper = lerp_f32(((render_tick / 8.0).sin() + 1.0) / 4.0, -45.0, 95.0);
        let head = -lower;
        let rave_tint = Some(rainbow_rgb(time.floor() as i64 * 100));
        return ArmRenderPose {
            base,
            lower,
            upper,
            head,
            rave_tint,
        };
    }

    let prev = arm.prev_angles;
    let curr = arm.curr_angles;
    ArmRenderPose {
        base: angle_lerp(alpha, f64::from(prev.base), f64::from(curr.base)),
        lower: angle_lerp(alpha, f64::from(prev.lower), f64::from(curr.lower)) - 135.0,
        upper: angle_lerp(alpha, f64::from(prev.upper), f64::from(curr.upper)) - 90.0,
        head: angle_lerp(alpha, f64::from(prev.head), f64::from(curr.head)),
        rave_tint: None,
    }
}

/// `ArmRenderer.transformClawHalf`: distance of each claw half from center along Y.
pub(super) fn claw_grip_offset(has_item: bool, is_block_item: bool) -> f32 {
    if !has_item {
        1.0 / 16.0
    } else if is_block_item {
        3.0 / 16.0
    } else {
        5.0 / 64.0
    }
}

/// `ArmRenderer.renderSafe`: item transform relative to `head_local` in block coordinates.
pub(super) fn held_item_local_matrix(head_local: Mat4, is_block_item: bool) -> Mat4 {
    let scale = if is_block_item { 0.5 } else { 0.625 };
    let y_offset = if is_block_item {
        -9.0 / 16.0
    } else {
        -10.0 / 16.0
    };
    head_local
        * Mat4::from_rotation_x(FRAC_PI_2)
        * Mat4::from_translation(Vec3::new(0.0, y_offset, 0.0))
        * Mat4::from_scale(Vec3::splat(scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::Compound;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn forward_head_pivot(arm_pos: [i32; 3], angles: ArmAngleSet, ceiling: bool) -> (DVec3, DVec3) {
        let origin = DVec3::new(
            arm_pos[0] as f64 + 0.5,
            arm_pos[1] as f64 + 0.5 + if ceiling { -6.0 / 16.0 } else { 6.0 / 16.0 },
            arm_pos[2] as f64 + 0.5,
        );
        let mut head_pos = DVec3::ZERO;
        head_pos = rotate_vec_deg(
            head_pos + DVec3::new(0.0, UPPER_ARM_LENGTH, 0.0),
            f64::from(angles.upper) + 180.0,
            'x',
        );
        head_pos = rotate_vec_deg(
            head_pos + DVec3::new(0.0, LOWER_ARM_LENGTH, 0.0),
            f64::from(angles.lower) - 90.0,
            'x',
        );
        head_pos = rotate_vec_deg(head_pos, f64::from(angles.base), 'y');
        if ceiling {
            head_pos = rotate_vec_deg(head_pos, 180.0, 'x');
        }
        head_pos += origin;

        // Head's -Z in local frame: rotated by (lower + upper - 180 + head - 45) about X, then base about Y, then ceiling.
        let net_x = f64::from(angles.lower - 135.0 + 135.0)
            + f64::from(angles.upper - 90.0 - 90.0)
            + f64::from(angles.head - 45.0);
        let mut head_fwd = rotate_vec_deg(DVec3::NEG_Z, net_x, 'x');
        head_fwd = rotate_vec_deg(head_fwd, f64::from(angles.base), 'y');
        if ceiling {
            head_fwd = rotate_vec_deg(head_fwd, 180.0, 'x');
        }
        (head_pos, head_fwd)
    }

    #[test]
    fn angle_lerp_takes_shortest_arc_across_zero() {
        assert!((shortest_angle_diff(350.0, 10.0) - 20.0).abs() < 1e-4);
        assert!((shortest_angle_diff(10.0, 350.0) + 20.0).abs() < 1e-4);
        assert!((angle_lerp(0.5, 350.0, 10.0) - 360.0).abs() < 1e-4);
        assert!((angle_lerp(0.5, 10.0, 350.0) - 0.0).abs() < 1e-4);
        assert!((angle_lerp(0.5, -10.0, 10.0) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn arm_ik_reaches_depot_target_on_floor_and_ceiling() {
        let depot = ArmPoint {
            rel_pos: [1, 0, 0],
            kind: ArmPointKind::Depot,
        };
        let arm_pos = [10, 64, -4];
        let (target_vec, claw_facing) = point_target_and_facing(depot, arm_pos, None);
        for ceiling in [false, true] {
            let angles = compute_arm_angles(arm_pos, target_vec, claw_facing, ceiling);
            assert!(
                angles.lower.is_finite() && angles.upper.is_finite() && angles.head.is_finite()
            );
            let (head_pos, head_fwd) = forward_head_pivot(arm_pos, angles, ceiling);
            let claw_tip = head_pos + head_fwd * 0.5;
            assert!(
                close(claw_tip, target_vec),
                "ceiling={ceiling} expected {target_vec:?}, got {claw_tip:?}"
            );
        }
    }

    #[test]
    fn arm_point_types_use_create_offsets_and_directions() {
        let arm = [0, 0, 0];
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [1, 0, 0],
                    kind: ArmPointKind::Depot
                },
                arm,
                None
            ),
            (DVec3::new(1.5, 14.0 / 16.0, 0.5), "down")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [0, 0, 1],
                    kind: ArmPointKind::Composter
                },
                arm,
                None
            ),
            (DVec3::new(0.5, 13.0 / 16.0, 1.5), "down")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [0, 1, 0],
                    kind: ArmPointKind::TopFace
                },
                arm,
                None
            ),
            (DVec3::new(0.5, 2.0, 0.5), "down")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [2, 0, 0],
                    kind: ArmPointKind::Crafter
                },
                arm,
                Some("east")
            ),
            (DVec3::new(2.0, 0.5, 0.5), "west")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [2, 0, 0],
                    kind: ArmPointKind::Deployer
                },
                arm,
                Some("north")
            ),
            (DVec3::new(2.5, 0.5, 0.5 + 0.65), "south")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [0, 0, 2],
                    kind: ArmPointKind::Funnel
                },
                arm,
                Some("south")
            ),
            (DVec3::new(0.5, 0.5, 2.5 - 0.15), "north")
        );
        assert_eq!(
            point_target_and_facing(
                ArmPoint {
                    rel_pos: [-1, 0, 0],
                    kind: ArmPointKind::Default
                },
                arm,
                None
            ),
            (DVec3::new(-0.5, 0.5, 0.5), "down")
        );
    }

    fn make_point(kind: &str, mode: &str, pos: Tag) -> Tag {
        let mut map = Compound::new();
        map.insert(
            JavaString::from("Type"),
            Tag::String(JavaString::from(kind)),
        );
        map.insert(
            JavaString::from("Mode"),
            Tag::String(JavaString::from(mode)),
        );
        map.insert(JavaString::from("Pos"), pos);
        Tag::Compound(map)
    }

    fn make_arm_tag(phase: &str, idx: i32, progress: f32, held: Option<&str>) -> Tag {
        let mut legacy = Compound::new();
        legacy.insert(JavaString::from("X"), Tag::Int(0));
        legacy.insert(JavaString::from("Y"), Tag::Int(0));
        legacy.insert(JavaString::from("Z"), Tag::Int(2));
        let points = vec![
            make_point("create:depot", "TAKE", Tag::IntArray(vec![2, 0, 0])),
            make_point("create:funnel", "DEPOSIT", Tag::Compound(legacy)),
        ];
        let mut arm = Compound::new();
        arm.insert(
            JavaString::from("Phase"),
            Tag::String(JavaString::from(phase)),
        );
        arm.insert(JavaString::from("TargetPointIndex"), Tag::Int(idx));
        arm.insert(
            JavaString::from("MovementProgress"),
            Tag::Float(progress.to_bits()),
        );
        arm.insert(JavaString::from("Goggles"), Tag::Byte(1));
        arm.insert(JavaString::from("InteractionPoints"), Tag::List(points));
        if let Some(item_id) = held {
            let mut item = Compound::new();
            item.insert(
                JavaString::from("id"),
                Tag::String(JavaString::from(item_id)),
            );
            item.insert(JavaString::from("count"), Tag::Int(1));
            arm.insert(JavaString::from("HeldItem"), Tag::Compound(item));
        }
        Tag::Compound(arm)
    }

    #[test]
    fn arm_nbt_parses_points_phase_item_and_preserves_transition() {
        let lookup = |pos: [i32; 3]| (pos == [0, 0, 2]).then(|| "north".to_owned());
        let tag1 = make_arm_tag("MOVE_TO_INPUT", 0, 1.0, None);
        let s1 = apply_arm_nbt(None, [0, 0, 0], false, &tag1, lookup).unwrap();
        assert_eq!(s1.phase, ArmPhase::MoveToInput);
        assert_eq!(s1.inputs.len(), 1);
        assert_eq!(s1.outputs.len(), 1);
        assert_eq!(s1.inputs[0].kind, ArmPointKind::Depot);
        assert_eq!(s1.outputs[0].kind, ArmPointKind::Funnel);
        assert!(s1.held_item.is_none());
        assert!(s1.goggles);

        let tag2 = make_arm_tag("MOVE_TO_OUTPUT", 0, 0.0, Some("minecraft:iron_ingot"));
        let s2 = apply_arm_nbt(Some(&s1), [0, 0, 0], false, &tag2, lookup).unwrap();
        assert_eq!(s2.phase, ArmPhase::MoveToOutput);
        assert_eq!(
            s2.held_item.as_ref().unwrap().item.as_ref(),
            "minecraft:iron_ingot"
        );
        // Previous target captured input depot angles when transitioning.
        let depot_angles = target_angles_for_point(s1.inputs[0], [0, 0, 0], false, &lookup);
        assert!((s2.previous_target.lower - depot_angles.lower).abs() < 1e-4);
        assert!((s2.previous_base_angle - depot_angles.base).abs() < 1e-4);
    }

    #[test]
    fn arm_smoke_full_pick_and_place_cycle() {
        let lookup = |pos: [i32; 3]| (pos == [0, 0, 2]).then(|| "north".to_owned());
        let tag_in = make_arm_tag("MOVE_TO_INPUT", 0, 0.0, None);
        let mut arm = apply_arm_nbt(None, [0, 0, 0], false, &tag_in, lookup).unwrap();
        // Tick with 64 rpm -> 64/1024 = 0.0625 per tick -> 8 ticks reaches 0.5 rest pullback, 16 reaches 1.0.
        for _ in 0..8 {
            tick_arm_state(&mut arm, [0, 0, 0], 64.0, false, lookup);
        }
        assert!((arm.progress - 0.5).abs() < 1e-5);
        for _ in 0..8 {
            tick_arm_state(&mut arm, [0, 0, 0], 64.0, false, lookup);
        }
        assert!((arm.progress - 1.0).abs() < 1e-5);

        let pose_start = arm_render_pose(Some(&arm), [0, 0, 0], 64.0, 16.0, 0.0);
        let pose_mid = arm_render_pose(Some(&arm), [0, 0, 0], 64.0, 16.5, 0.5);
        let pose_end = arm_render_pose(Some(&arm), [0, 0, 0], 64.0, 17.0, 1.0);
        assert!(pose_start.rave_tint.is_none());
        assert!(pose_mid.lower.is_finite() && pose_end.lower.is_finite());

        // Transition to MOVE_TO_OUTPUT holding a stone block.
        let tag_out = make_arm_tag("MOVE_TO_OUTPUT", 0, 0.0, Some("minecraft:stone"));
        arm = apply_arm_nbt(Some(&arm), [0, 0, 0], false, &tag_out, lookup).unwrap();
        tick_arm_state(&mut arm, [0, 0, 0], 64.0, false, lookup);
        assert!(arm.progress > 0.0 && arm.progress < 0.5);
        for _ in 0..20 {
            tick_arm_state(&mut arm, [0, 0, 0], 64.0, false, lookup);
        }
        assert!((arm.progress - 1.0).abs() < 1e-5);

        // Transition to DANCING with nonzero speed -> rave angles & rainbow tint.
        let tag_dance = make_arm_tag("DANCING", -1, 1.0, None);
        arm = apply_arm_nbt(Some(&arm), [0, 0, 0], false, &tag_dance, lookup).unwrap();
        let rave_a = arm_render_pose(Some(&arm), [0, 0, 0], 32.0, 10.0, 0.0);
        let rave_b = arm_render_pose(Some(&arm), [0, 0, 0], 32.0, 14.0, 0.0);
        assert!(rave_a.rave_tint.is_some());
        assert_ne!(rave_a.base, rave_b.base);
        assert_ne!(rave_a.rave_tint, rave_b.rave_tint);
    }
}
