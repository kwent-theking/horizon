//! Create machines whose block entity renderers draw moving parts besides the
//! turning shaft: the mechanical saw (`SawRenderer`), the harvester
//! (`HarvesterRenderer`), the rope, hose and elevator pulleys
//! (`AbstractPulleyRenderer`, `PulleyRenderer`, `HosePulleyRenderer`,
//! `ElevatorPulleyRenderer`), the Group A spinners (`GearboxRenderer`,
//! `SplitShaftRenderer`, `SpeedControllerRenderer`, `BacktankRenderer`,
//! `MechanicalCrafterRenderer`, `FlapDisplayRenderer`, `GantryCarriageRenderer`)
//! and the mechanical arm (`ArmRenderer`).
//!
//! Every matrix is block-local, in blocks. A `SuperByteBuffer` call sequence
//! multiplies like a `PoseStack`: the first call is the outermost transform
//! and `CachedBuffers.partialFacing` bakes its rotation into the vertices
//! first, so it is the innermost one.
use super::{angle_radians, arm, facing_z_rotation, opposite_facing, rotation_offset};
use glam::{Mat4, Quat, Vec3};
use horizon_client::world::{ArmAnimState, MachineState};
use std::f32::consts::{FRAC_PI_2, PI};

const SAW_BLADE_HORIZONTAL_ACTIVE: &str = "create:block/mechanical_saw/blade_horizontal_active";
const SAW_BLADE_HORIZONTAL_REVERSED: &str = "create:block/mechanical_saw/blade_horizontal_reversed";
const SAW_BLADE_HORIZONTAL_INACTIVE: &str = "create:block/mechanical_saw/blade_horizontal_inactive";
const SAW_BLADE_VERTICAL_ACTIVE: &str = "create:block/mechanical_saw/blade_vertical_active";
const SAW_BLADE_VERTICAL_REVERSED: &str = "create:block/mechanical_saw/blade_vertical_reversed";
const SAW_BLADE_VERTICAL_INACTIVE: &str = "create:block/mechanical_saw/blade_vertical_inactive";
const SHAFT: &str = "create:block/shaft";
const SHAFT_HALF: &str = "create:block/shaft_half";

const HARVESTER_BLADE: &str = "create:block/mechanical_harvester/blade";

const ROPE_COIL: &str = "create:block/rope_pulley/rope_coil";
const ROPE_HALF: &str = "create:block/rope_pulley/rope_half";
const ROPE_HALF_MAGNET: &str = "create:block/rope_pulley/rope_half_magnet";
/// `AllBlocks.ROPE`'s blockstate is the `rope_pulley/rope` model.
const ROPE: &str = "create:block/rope_pulley/rope";
/// `AllBlocks.PULLEY_MAGNET`'s blockstate is the `rope_pulley/pulley_magnet` model.
const PULLEY_MAGNET: &str = "create:block/rope_pulley/pulley_magnet";

const HOSE_COIL: &str = "create:block/hose_pulley/hose_coil";
const HOSE: &str = "create:block/hose_pulley/rope";
const HOSE_MAGNET: &str = "create:block/hose_pulley/pulley_magnet";
const HOSE_HALF: &str = "create:block/hose_pulley/rope_half";
const HOSE_HALF_MAGNET: &str = "create:block/hose_pulley/rope_half_magnet";

const ELEVATOR_COIL: &str = "create:block/elevator_pulley/rope_coil";
const ELEVATOR_MAGNET: &str = "create:block/elevator_pulley/pulley_magnet";
const ELEVATOR_BELT: &str = "create:block/elevator_pulley/rope";
const ELEVATOR_BELT_HALF: &str = "create:block/elevator_pulley/rope_half";

const ARM_BASE: &str = "create:block/mechanical_arm/base";
const ARM_LOWER_BODY: &str = "create:block/mechanical_arm/lower_body";
const ARM_UPPER_BODY: &str = "create:block/mechanical_arm/upper_body";
const ARM_CLAW_BASE: &str = "create:block/mechanical_arm/claw_base";
const ARM_CLAW_BASE_GOGGLES: &str = "create:block/mechanical_arm/claw_base_goggles";
const ARM_CLAW_GRIP_UPPER: &str = "create:block/mechanical_arm/upper_claw_grip";
const ARM_CLAW_GRIP_LOWER: &str = "create:block/mechanical_arm/lower_claw_grip";

const SPEED_CONTROLLER_BRACKET: &str = "create:block/rotation_speed_controller/bracket";
const COPPER_BACKTANK_SHAFT: &str = "create:block/copper_backtank/block_shaft_input";
const COPPER_BACKTANK_COGS: &str = "create:block/copper_backtank/block_cogs";
const NETHERITE_BACKTANK_SHAFT: &str = "create:block/netherite_backtank/block_shaft_input";
const NETHERITE_BACKTANK_COGS: &str = "create:block/netherite_backtank/block_cogs";
const SHAFTLESS_COGWHEEL: &str = "create:block/cogwheel_shaftless";
const GANTRY_COGS: &str = "create:block/gantry_carriage/wheels";

/// `HarvesterRenderer.PIVOT` in pixels (x is zero).
const HARVESTER_PIVOT: Vec3 = Vec3::new(0.0, 6.0 / 16.0, 9.0 / 16.0);
/// `HarvesterBlockEntity.manuallyAnimatedSpeed`: only Ponder sets it, it is
/// never written to a tag, so a placed harvester's blade stands still.
const HARVESTER_ANIMATED_SPEED: f32 = 0.0;

/// `HosePulleyBlockEntity.getInterpolatedOffset`: the hose is out at least this far.
const HOSE_MIN_OFFSET: f32 = 3.0 / 16.0;
/// A rope longer than this is not drawn past it (Create's `maxRopeLength` is 384).
const MAX_ROPE_SEGMENTS: usize = 512;

/// UV sprite shift and scroll delta applied at quad emit time without mutating
/// cached templates (`Catnip DefaultSuperByteBuffer.shiftUVScrolling`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SpriteShift {
    pub original: &'static str,
    pub target: &'static str,
    pub scroll_u: f32,
    pub scroll_v: f32,
}

/// One model of a machine, drawn with `local` inside its block. `below` is
/// how many blocks under the machine its light is sampled
/// (`AbstractPulleyRenderer.renderAt`: `pulleyPos.below((int) offset)`).
pub(super) struct MachinePart {
    pub model: &'static str,
    pub local: Mat4,
    pub below: i32,
    pub tint: Option<u32>,
    pub sprite_shift: Option<SpriteShift>,
    pub full_bright: bool,
}

/// The block's properties and the block entity's state a machine draws from.
pub(super) struct Machine<'a> {
    pub block: &'a str,
    pub facing: &'a str,
    pub axis: &'a str,
    pub along_first: bool,
    pub ceiling: bool,
    pub vertical: bool,
    pub pos: [i32; 3],
    pub speed: f32,
    pub time: f64,
    pub alpha: f64,
    pub state: MachineState,
    pub has_controller_bracket: bool,
    pub arm: Option<&'a ArmAnimState>,
    pub arm_has_item: bool,
    pub arm_is_block_item: bool,
}

/// Whether `block` has parts drawn by `machine_parts`.
pub(super) fn is_machine(block: &str) -> bool {
    matches!(
        block,
        "create:mechanical_saw"
            | "create:mechanical_harvester"
            | "create:rope_pulley"
            | "create:hose_pulley"
            | "create:elevator_pulley"
            | "create:mechanical_arm"
            | "create:gearbox"
            | "create:sequenced_gearshift"
            | "create:rotation_speed_controller"
            | "create:copper_backtank"
            | "create:netherite_backtank"
            | "create:mechanical_crafter"
            | "create:display_board"
            | "create:gantry_carriage"
    )
}

pub(super) fn machine_parts(machine: &Machine) -> Vec<MachinePart> {
    match machine.block {
        "create:mechanical_saw" => saw_parts(machine),
        "create:mechanical_harvester" => harvester_parts(machine),
        "create:rope_pulley" => rope_pulley_parts(machine),
        "create:hose_pulley" => hose_pulley_parts(machine),
        "create:elevator_pulley" => elevator_pulley_parts(machine),
        "create:mechanical_arm" => arm_parts(machine),
        "create:gearbox" => gearbox_parts(machine),
        "create:sequenced_gearshift" => sequenced_gearshift_parts(machine),
        "create:rotation_speed_controller" => speed_controller_parts(machine),
        "create:copper_backtank" | "create:netherite_backtank" => backtank_parts(machine),
        "create:mechanical_crafter" => crafter_parts(machine),
        "create:display_board" => display_board_parts(machine),
        "create:gantry_carriage" => gantry_carriage_parts(machine),
        _ => Vec::new(),
    }
}

/// A turn of `rotation` about the block's centre.
pub(super) fn centred(rotation: Quat) -> Mat4 {
    Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_quat(rotation)
        * Mat4::from_translation(Vec3::splat(-0.5))
}

/// `AngleHelper.horizontalAngle`, in degrees: `Direction.toYRot`, negated on
/// the X axis; zero for up and down.
pub(super) fn horizontal_angle(facing: &str) -> f32 {
    match facing {
        "south" => 0.0,
        "west" => -90.0,
        "north" => 180.0,
        "east" => -270.0,
        _ => 0.0,
    }
}

fn axis_name_index(axis: &str) -> usize {
    match axis {
        "x" => 0,
        "z" => 2,
        _ => 1,
    }
}

fn is_horizontal(facing: &str) -> bool {
    matches!(facing, "north" | "south" | "east" | "west")
}

/// `Direction.get(POSITIVE, axis)` as a facing.
fn positive_direction(axis: &str) -> &'static str {
    match axis {
        "x" => "east",
        "z" => "south",
        _ => "up",
    }
}

/// `getClockWise().getAxis()` of a horizontal facing: the shaft axis of
/// hose and elevator pulleys.
pub(super) fn clockwise_axis(facing: &str) -> &'static str {
    match facing {
        "north" | "south" => "x",
        _ => "z",
    }
}

/// `KineticBlockEntityRenderer.kineticRotationTransform`: a turn about the
/// positive direction of `axis` through the block's centre.
pub(super) fn spin(axis: &str, angle: f32) -> Mat4 {
    let mut direction = Vec3::ZERO;
    direction[axis_name_index(axis)] = 1.0;
    centred(Quat::from_axis_angle(direction, angle))
}

/// `standardKineticRotationTransform`'s angle, with the position's tooth offset.
pub(super) fn shaft_angle(machine: &Machine, axis: &str) -> f32 {
    angle_radians(
        machine.time,
        machine.speed,
        rotation_offset(axis_name_index(axis), machine.pos),
    )
}

pub(super) fn part(model: &'static str, local: Mat4) -> MachinePart {
    MachinePart {
        model,
        local,
        below: 0,
        tint: None,
        sprite_shift: None,
        full_bright: false,
    }
}

/// A full shaft along `axis`, turning (`KineticBlockEntityRenderer.shaft`).
pub(super) fn shaft_along(machine: &Machine, axis: &str) -> MachinePart {
    let rotation = centred(super::axis_y_rotation(axis));
    part(SHAFT, spin(axis, shaft_angle(machine, axis)) * rotation)
}

/// `SawRenderer`: the blade by speed sign and orientation, and the shaft.
fn saw_parts(machine: &Machine) -> Vec<MachinePart> {
    let horizontal = is_horizontal(machine.facing);
    let blade = match (horizontal, machine.speed.partial_cmp(&0.0)) {
        (true, Some(std::cmp::Ordering::Greater)) => SAW_BLADE_HORIZONTAL_ACTIVE,
        (true, Some(std::cmp::Ordering::Less)) => SAW_BLADE_HORIZONTAL_REVERSED,
        (true, _) => SAW_BLADE_HORIZONTAL_INACTIVE,
        (false, Some(std::cmp::Ordering::Greater)) => SAW_BLADE_VERTICAL_ACTIVE,
        (false, Some(std::cmp::Ordering::Less)) => SAW_BLADE_VERTICAL_REVERSED,
        (false, _) => SAW_BLADE_VERTICAL_INACTIVE,
    };
    // `partialFacing` first, then `rotateCentered(rad(90), UP)` over it.
    let mut blade_local = centred(facing_z_rotation(machine.facing));
    if !horizontal && machine.along_first {
        blade_local = centred(Quat::from_rotation_y(FRAC_PI_2)) * blade_local;
    }

    // `getRotatedModel`: a half shaft out of the back of a horizontal saw
    // (`partialFacing(SHAFT_HALF, state.rotate(CLOCKWISE_180))`), else a full
    // shaft along the rotation axis (`SawBlock.getRotationAxis`).
    let shaft = if horizontal {
        let axis = match machine.facing {
            "east" | "west" => "x",
            _ => "z",
        };
        let facing_back = centred(facing_z_rotation(opposite_facing(machine.facing)));
        part(
            SHAFT_HALF,
            spin(axis, shaft_angle(machine, axis)) * facing_back,
        )
    } else {
        let axis = super::instruments::directional_axis(machine.facing, machine.along_first)
            .unwrap_or("y");
        shaft_along(machine, axis)
    };
    vec![part(blade, blade_local), shaft]
}

/// `HarvesterRenderer.transform`: the blade turned with the facing, and
/// about the pivot's west axis by `angle` degrees.
fn harvester_blade(facing: &str, angle: f32) -> Mat4 {
    centred(Quat::from_rotation_y(horizontal_angle(facing).to_radians()))
        * Mat4::from_translation(HARVESTER_PIVOT)
        // `rotate(rad(angle), Direction.WEST)`.
        * Mat4::from_quat(Quat::from_rotation_x(-angle.to_radians()))
        * Mat4::from_translation(-HARVESTER_PIVOT)
}

fn harvester_parts(machine: &Machine) -> Vec<MachinePart> {
    let angle = (machine.time as f32 / 20.0 * HARVESTER_ANIMATED_SPEED) % 360.0;
    vec![part(
        HARVESTER_BLADE,
        harvester_blade(machine.facing, angle),
    )]
}

/// `AbstractPulleyRenderer.renderAt`: `translate(0, -offset, 0)`.
fn hung_at(model: &'static str, offset: f32) -> MachinePart {
    MachinePart {
        model,
        local: Mat4::from_translation(Vec3::new(0.0, -offset, 0.0)),
        below: offset as i32,
        tint: None,
        sprite_shift: None,
        full_bright: false,
    }
}

/// `AbstractPulleyRenderer.scrollCoil` normalized V scroll fraction in `[-0.5, 0.5)`.
pub(super) fn coil_scroll_v(offset: f32, speed_modifier: f32) -> f32 {
    let scaled = f64::from(offset) * f64::from(speed_modifier * 0.5);
    let x = scaled + 3.0 / 16.0;
    (-x - (x * -2.0).floor() * 0.5) as f32
}

/// `ElevatorPulleyRenderer` belt normalized V scroll fraction in `[0, 0.5)`.
pub(super) fn elevator_belt_scroll_v(offset: f32) -> f32 {
    let x = -(f64::from(offset) + 0.5);
    ((x - x.floor()) * 0.5) as f32
}

/// The rope or hose models of `AbstractPulleyRenderer.renderSafe`.
struct RopeModels {
    magnet: &'static str,
    half_magnet: &'static str,
    half_rope: &'static str,
    rope: &'static str,
}

fn paid_out_rope(models: &RopeModels, offset: f32, running: bool) -> Vec<MachinePart> {
    let mut parts = Vec::new();
    if running || offset == 0.0 {
        let magnet = if offset > 0.25 {
            models.magnet
        } else {
            models.half_magnet
        };
        parts.push(hung_at(magnet, offset));
    }
    let fraction = offset % 1.0;
    if offset > 0.75 && !(0.25..=0.75).contains(&fraction) {
        let shift = if fraction > 0.75 {
            fraction - 1.0
        } else {
            fraction
        };
        parts.push(hung_at(models.half_rope, shift));
    }
    if !running {
        return parts;
    }
    let mut segment = 0usize;
    while (segment as f32) < offset - 1.25 && segment < MAX_ROPE_SEGMENTS {
        parts.push(hung_at(models.rope, offset - segment as f32 - 1.0));
        segment += 1;
    }
    parts
}

/// `PulleyRenderer`: shaft, coil, magnet and rope of a rope pulley.
fn rope_pulley_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = machine.axis;
    // `getRotatedCoil`: `partialFacing(coil, state, Direction.get(POSITIVE, shaftAxis))`.
    let coil = centred(facing_z_rotation(positive_direction(axis)));
    let mut coil_part = part(ROPE_COIL, coil);
    if machine.state.offset != 0.0 {
        coil_part.sprite_shift = Some(SpriteShift {
            original: "create:block/rope_pulley_coil",
            target: "create:block/rope_pulley_coil_scroll",
            scroll_u: 0.0,
            scroll_v: coil_scroll_v(machine.state.offset, 1.0),
        });
    }
    let mut parts = vec![shaft_along(machine, axis), coil_part];
    let models = RopeModels {
        magnet: PULLEY_MAGNET,
        half_magnet: ROPE_HALF_MAGNET,
        half_rope: ROPE_HALF,
        rope: ROPE,
    };
    parts.extend(paid_out_rope(
        &models,
        machine.state.offset,
        machine.state.running,
    ));
    parts
}

/// `HosePulleyRenderer`: the same, along the shaft that is clockwise of the
/// facing, always running, its offset at least 3/16.
fn hose_pulley_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = clockwise_axis(machine.facing);
    let offset = machine.state.offset.max(HOSE_MIN_OFFSET);
    let coil = centred(facing_z_rotation(positive_direction(axis)));
    let mut coil_part = part(HOSE_COIL, coil);
    coil_part.sprite_shift = Some(SpriteShift {
        original: "create:block/hose_pulley_coil",
        target: "create:block/hose_pulley_coil_scroll",
        scroll_u: 0.0,
        scroll_v: coil_scroll_v(offset, 1.0),
    });
    let mut parts = vec![shaft_along(machine, axis), coil_part];
    let models = RopeModels {
        magnet: HOSE_MAGNET,
        half_magnet: HOSE_HALF_MAGNET,
        half_rope: HOSE_HALF,
        rope: HOSE,
    };
    parts.extend(paid_out_rope(&models, offset, true));
    parts
}

/// `ElevatorPulleyRenderer`: its magnet and belt are turned by
/// `180 + horizontalAngle(facing)` about the centre, its coil faces the
/// facing, and the belt's loop counts from `offset` rather than `offset - 1`.
fn elevator_pulley_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = clockwise_axis(machine.facing);
    let offset = machine.state.offset;
    let running = machine.state.running;
    let belt_turn = centred(Quat::from_rotation_y(
        (180.0 + horizontal_angle(machine.facing)).to_radians(),
    ));
    let belt_shift = (offset != 0.0).then(|| SpriteShift {
        original: "create:block/elevator_pulley_belt",
        target: "create:block/elevator_pulley_belt_scroll",
        scroll_u: 0.0,
        scroll_v: elevator_belt_scroll_v(offset),
    });
    let hung = |model, at: f32, shift: Option<SpriteShift>| {
        let mut hung = hung_at(model, at);
        hung.local *= belt_turn;
        hung.sprite_shift = shift;
        hung
    };

    let mut parts = vec![shaft_along(machine, axis)];
    if running || offset == 0.0 {
        parts.push(hung(ELEVATOR_MAGNET, offset, None));
    }
    let mut coil_part = part(ELEVATOR_COIL, centred(facing_z_rotation(machine.facing)));
    if offset != 0.0 {
        coil_part.sprite_shift = Some(SpriteShift {
            original: "create:block/elevator_pulley_coil",
            target: "create:block/elevator_pulley_coil_scroll",
            scroll_u: 0.0,
            scroll_v: coil_scroll_v(offset, 2.0),
        });
    }
    parts.push(coil_part);
    if offset == 0.0 {
        return parts;
    }
    let fraction = offset % 1.0;
    if !(0.25..=0.75).contains(&fraction) {
        let shift = if fraction > 0.75 {
            fraction - 1.0
        } else {
            fraction
        };
        parts.push(hung(ELEVATOR_BELT_HALF, shift, belt_shift));
    }
    if !running {
        return parts;
    }
    let mut segment = 0usize;
    while (segment as f32) < offset - 0.25 && segment < MAX_ROPE_SEGMENTS {
        parts.push(hung(ELEVATOR_BELT, offset - segment as f32, belt_shift));
        segment += 1;
    }
    parts
}

fn rotate_x_degrees(degrees: f32) -> Mat4 {
    Mat4::from_rotation_x(degrees.to_radians())
}

fn direction_axis(dir: &str) -> &'static str {
    match dir {
        "east" | "west" => "x",
        "up" | "down" => "y",
        _ => "z",
    }
}

fn direction_is_positive(dir: &str) -> bool {
    matches!(dir, "up" | "south" | "east")
}

fn nearest_direction(dx: i32, dy: i32, dz: i32) -> Option<&'static str> {
    if dx == 0 && dy == 0 && dz == 0 {
        return None;
    }
    let (ax, ay, az) = (dx.abs(), dy.abs(), dz.abs());
    Some(if ax >= ay && ax >= az {
        if dx > 0 { "east" } else { "west" }
    } else if ay >= ax && ay >= az {
        if dy > 0 { "up" } else { "down" }
    } else if dz > 0 {
        "south"
    } else {
        "north"
    })
}

/// `GearboxRenderer`: four half shafts perpendicular to the box axis, reversing
/// relative to `Source`.
fn gearbox_parts(machine: &Machine) -> Vec<MachinePart> {
    let source_dir = machine.state.source.and_then(|src| {
        nearest_direction(
            src[0] - machine.pos[0],
            src[1] - machine.pos[1],
            src[2] - machine.pos[2],
        )
    });
    let mut parts = Vec::with_capacity(4);
    for dir in ["down", "up", "north", "south", "west", "east"] {
        let axis = direction_axis(dir);
        if axis == machine.axis {
            continue;
        }
        let offset = rotation_offset(axis_name_index(axis), machine.pos);
        let mut angle_deg = (machine.time * f64::from(machine.speed) * 3.0 / 10.0) % 360.0;
        if machine.speed != 0.0
            && let Some(src_dir) = source_dir
        {
            let src_axis = direction_axis(src_dir);
            if src_axis == axis {
                if src_dir != dir {
                    angle_deg *= -1.0;
                }
            } else if direction_is_positive(src_dir) == direction_is_positive(dir) {
                angle_deg *= -1.0;
            }
        }
        let angle = ((angle_deg + offset) as f32).to_radians();
        parts.push(part(
            SHAFT_HALF,
            spin(axis, angle) * centred(facing_z_rotation(dir)),
        ));
    }
    parts
}

/// `SplitShaftRenderer` for `create:sequenced_gearshift`: two half shafts along
/// `Y` when vertical, otherwise along `axis`.
fn sequenced_gearshift_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = if machine.vertical { "y" } else { machine.axis };
    let angle = shaft_angle(machine, axis);
    let dirs = match axis {
        "x" => ["west", "east"],
        "y" => ["down", "up"],
        _ => ["north", "south"],
    };
    dirs.into_iter()
        .map(|dir| {
            part(
                SHAFT_HALF,
                spin(axis, angle) * centred(facing_z_rotation(dir)),
            )
        })
        .collect()
}

/// `SpeedControllerRenderer`: shaft along `axis` and bracket above when a
/// horizontal large cogwheel sits at `pos.above()`.
fn speed_controller_parts(machine: &Machine) -> Vec<MachinePart> {
    let mut parts = vec![shaft_along(machine, machine.axis)];
    if machine.has_controller_bracket {
        let y_rot = if machine.axis == "x" { PI } else { FRAC_PI_2 };
        parts.push(MachinePart {
            model: SPEED_CONTROLLER_BRACKET,
            local: Mat4::from_translation(Vec3::Y) * centred(Quat::from_rotation_y(y_rot)),
            below: -1,
            tint: None,
            sprite_shift: None,
            full_bright: false,
        });
    }
    parts
}

/// `BacktankRenderer`: vertical shaft input and spinning back cogs.
fn backtank_parts(machine: &Machine) -> Vec<MachinePart> {
    let (shaft_model, cogs_model) = if machine.block == "create:netherite_backtank" {
        (NETHERITE_BACKTANK_SHAFT, NETHERITE_BACKTANK_COGS)
    } else {
        (COPPER_BACKTANK_SHAFT, COPPER_BACKTANK_COGS)
    };
    let shaft_local = spin("y", shaft_angle(machine, "y"));
    let cogs_angle_deg = (f64::from(machine.speed) / 4.0 * machine.time) % 360.0;
    let cogs_local = centred(Quat::from_rotation_y(
        (180.0 + horizontal_angle(machine.facing)).to_radians(),
    )) * Mat4::from_translation(Vec3::new(0.0, 6.5 / 16.0, 11.0 / 16.0))
        * rotate_x_degrees(cogs_angle_deg as f32)
        * Mat4::from_translation(Vec3::new(0.0, -6.5 / 16.0, -11.0 / 16.0));
    vec![part(shaft_model, shaft_local), part(cogs_model, cogs_local)]
}

/// `MechanicalCrafterRenderer.renderFast`: shaftless cogwheel turned to facing.
fn crafter_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = direction_axis(machine.facing);
    let y_rot = if axis == "x" { FRAC_PI_2 } else { 0.0 };
    let local = spin(axis, shaft_angle(machine, axis))
        * centred(Quat::from_rotation_y(y_rot) * Quat::from_rotation_x(FRAC_PI_2));
    vec![part(SHAFTLESS_COGWHEEL, local)]
}

/// `FlapDisplayRenderer.getRotatedModel`: `partialFacingVertical` shaftless cog.
fn display_board_parts(machine: &Machine) -> Vec<MachinePart> {
    let axis = direction_axis(machine.facing);
    let local = spin(axis, shaft_angle(machine, axis))
        * centred(
            Quat::from_rotation_y(horizontal_angle(machine.facing).to_radians())
                * Quat::from_rotation_x(FRAC_PI_2),
        );
    vec![part(SHAFTLESS_COGWHEEL, local)]
}

/// `GantryCarriageRenderer`: shaft along `DirectionalAxis` and half-speed pinion wheels.
fn gantry_carriage_parts(machine: &Machine) -> Vec<MachinePart> {
    let rot_axis =
        super::instruments::directional_axis(machine.facing, machine.along_first).unwrap_or("y");
    let facing_axis = direction_axis(machine.facing);
    let visual_pos = match machine.facing {
        "west" => [machine.pos[0] + 1, machine.pos[1], machine.pos[2]],
        "down" => [machine.pos[0], machine.pos[1] + 1, machine.pos[2]],
        "north" => [machine.pos[0], machine.pos[1], machine.pos[2] + 1],
        _ => machine.pos,
    };
    let offset = rotation_offset(axis_name_index(rot_axis), visual_pos);
    let mut angle_deg = (machine.time * f64::from(machine.speed) * 3.0 / 20.0 + offset) % 360.0;

    let gantry_axis = ["x", "y", "z"]
        .into_iter()
        .find(|&a| a != rot_axis && a != facing_axis)
        .unwrap_or("x");
    if gantry_axis == "x" && machine.facing == "up" {
        angle_deg *= -1.0;
    }
    if gantry_axis == "y" && matches!(machine.facing, "north" | "east") {
        angle_deg *= -1.0;
    }

    let x1_deg = match machine.facing {
        "up" => 0.0,
        "down" => 180.0,
        _ => 90.0,
    };
    let y2_deg: f32 = if machine.along_first ^ (facing_axis == "x") {
        0.0
    } else {
        90.0
    };
    let wheels_local = Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_rotation_y(horizontal_angle(machine.facing).to_radians())
        * rotate_x_degrees(x1_deg)
        * Mat4::from_rotation_y(y2_deg.to_radians())
        * Mat4::from_translation(Vec3::new(0.0, -9.0 / 16.0, 0.0))
        * rotate_x_degrees(-(angle_deg as f32))
        * Mat4::from_translation(Vec3::new(0.0, 9.0 / 16.0, 0.0))
        * Mat4::from_translation(Vec3::splat(-0.5));
    vec![
        shaft_along(machine, rot_axis),
        part(GANTRY_COGS, wheels_local),
    ]
}

/// `ArmRenderer.renderArm` in the arm's interpolated or resting pose, returning the
/// parts plus the head's block-local matrix for submitting any held item.
pub(super) fn arm_parts_with_head(machine: &Machine) -> (Vec<MachinePart>, Mat4) {
    let pose = arm::arm_render_pose(
        machine.arm,
        machine.pos,
        machine.speed,
        machine.time,
        machine.alpha,
    );
    let goggles = machine.arm.map_or(machine.state.goggles, |a| a.goggles);
    let inverted = machine.ceiling && goggles;

    // `msr.center()`, then `rotateXDegrees(180)` on a ceiling arm.
    let mut stack = Mat4::from_translation(Vec3::splat(0.5));
    if machine.ceiling {
        stack *= rotate_x_degrees(180.0);
    }
    // transformBase
    stack *= Mat4::from_translation(Vec3::new(0.0, 4.0 / 16.0, 0.0))
        * Mat4::from_rotation_y(pose.base.to_radians());
    let base = stack;
    // transformLowerArm
    stack *= Mat4::from_translation(Vec3::new(0.0, 2.0 / 16.0, 0.0))
        * rotate_x_degrees(pose.lower + 135.0);
    let lower = stack;
    // transformUpperArm
    stack *= Mat4::from_translation(Vec3::new(0.0, 0.0, -14.0 / 16.0))
        * rotate_x_degrees(pose.upper - 90.0);
    let upper = stack;
    // transformHead
    stack *= Mat4::from_translation(Vec3::new(0.0, 0.0, -15.0 / 16.0))
        * rotate_x_degrees(pose.head - 45.0);
    let head = stack;
    let claw = if inverted {
        head * Mat4::from_rotation_z(PI)
    } else {
        head
    };

    let mut lower_part = part(ARM_LOWER_BODY, lower);
    lower_part.tint = pose.rave_tint;
    let mut upper_part = part(ARM_UPPER_BODY, upper);
    upper_part.tint = pose.rave_tint;

    let mut parts = vec![
        part(ARM_BASE, base),
        lower_part,
        upper_part,
        part(
            if goggles {
                ARM_CLAW_BASE_GOGGLES
            } else {
                ARM_CLAW_BASE
            },
            claw,
        ),
    ];
    let grip_offset = arm::claw_grip_offset(machine.arm_has_item, machine.arm_is_block_item);
    for flip in [1.0f32, -1.0] {
        let grip = if flip > 0.0 {
            ARM_CLAW_GRIP_LOWER
        } else {
            ARM_CLAW_GRIP_UPPER
        };
        let half = head * Mat4::from_translation(Vec3::new(0.0, -flip * grip_offset, -6.0 / 16.0));
        parts.push(part(grip, half));
    }
    (parts, head)
}

fn arm_parts(machine: &Machine) -> Vec<MachinePart> {
    arm_parts_with_head(machine).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn machine(block: &'static str, facing: &'static str, speed: f32) -> Machine<'static> {
        Machine {
            block,
            facing,
            axis: "z",
            along_first: false,
            ceiling: false,
            vertical: false,
            pos: [1, 0, 0],
            speed,
            time: 0.0,
            alpha: 0.0,
            state: MachineState::default(),
            has_controller_bracket: false,
            arm: None,
            arm_has_item: false,
            arm_is_block_item: false,
        }
    }

    fn models(parts: &[MachinePart]) -> Vec<&'static str> {
        parts.iter().map(|p| p.model).collect()
    }

    #[test]
    fn horizontal_angle_follows_catnip() {
        assert_eq!(horizontal_angle("south"), 0.0);
        assert_eq!(horizontal_angle("north"), 180.0);
        assert_eq!(horizontal_angle("west"), -90.0);
        assert_eq!(horizontal_angle("east"), -270.0);
        assert_eq!(horizontal_angle("up"), 0.0);
        // The same turn `facing_z_rotation` makes for each side.
        for facing in ["north", "south", "east", "west"] {
            let turn = Quat::from_rotation_y(horizontal_angle(facing).to_radians());
            let rotation = facing_z_rotation(facing);
            let probe = Vec3::new(0.3, 0.2, 0.9);
            assert!(close(turn * probe, rotation * probe), "{facing}");
        }
    }

    #[test]
    fn saw_blade_follows_the_speed_sign() {
        let blade =
            |facing, speed| saw_parts(&machine("create:mechanical_saw", facing, speed))[0].model;
        assert_eq!(blade("east", 16.0), SAW_BLADE_HORIZONTAL_ACTIVE);
        assert_eq!(blade("east", -16.0), SAW_BLADE_HORIZONTAL_REVERSED);
        assert_eq!(blade("east", 0.0), SAW_BLADE_HORIZONTAL_INACTIVE);
        assert_eq!(blade("up", 16.0), SAW_BLADE_VERTICAL_ACTIVE);
        assert_eq!(blade("down", -16.0), SAW_BLADE_VERTICAL_REVERSED);
        assert_eq!(blade("up", 0.0), SAW_BLADE_VERTICAL_INACTIVE);
    }

    #[test]
    fn saw_blade_is_turned_to_its_facing_then_across_the_first_axis() {
        // A point 0.25 in front of the block centre along the model's +Z.
        let front = Vec3::new(0.5, 0.5, 0.75);
        let blade = |facing, along_first| {
            let mut m = machine("create:mechanical_saw", facing, 16.0);
            m.along_first = along_first;
            saw_parts(&m)[0].local
        };
        assert!(close(
            blade("east", false).transform_point3(front),
            Vec3::new(0.75, 0.5, 0.5)
        ));
        assert!(close(
            blade("north", false).transform_point3(front),
            Vec3::new(0.5, 0.5, 0.25)
        ));
        assert!(close(
            blade("up", false).transform_point3(front),
            Vec3::new(0.5, 0.75, 0.5)
        ));
        // Facing up: the model's +Z goes to +Y first; the 90 degree turn
        // about Y over it leaves +Y alone and moves the model's +X to -Z.
        assert!(close(
            blade("up", true).transform_point3(front),
            Vec3::new(0.5, 0.75, 0.5)
        ));
        let side = Vec3::new(0.75, 0.5, 0.5);
        assert!(close(blade("up", false).transform_point3(side), side));
        assert!(close(
            blade("up", true).transform_point3(side),
            Vec3::new(0.5, 0.5, 0.25)
        ));
        // The turn is ignored for a horizontal saw.
        assert!(close(
            blade("east", true).transform_point3(front),
            Vec3::new(0.75, 0.5, 0.5)
        ));
    }

    #[test]
    fn saw_shaft_sticks_out_of_the_back_or_runs_along_the_axis() {
        let shaft = |facing, along_first| {
            let mut m = machine("create:mechanical_saw", facing, 0.0);
            m.along_first = along_first;
            m.pos = [1, 0, 1];
            saw_parts(&m).remove(1)
        };
        // Facing east, the half shaft's +Z end points west.
        let east = shaft("east", false);
        assert_eq!(east.model, SHAFT_HALF);
        assert!(close(
            east.local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.25, 0.5, 0.5)
        ));
        // Facing up with the first axis: the shaft lies along X.
        let up = shaft("up", true);
        assert_eq!(up.model, SHAFT);
        assert!(close(
            up.local.transform_point3(Vec3::new(0.5, 0.75, 0.5)),
            Vec3::new(0.75, 0.5, 0.5)
        ));
        assert_eq!(shaft("up", false).model, SHAFT);
    }

    #[test]
    fn saw_shaft_turns_about_the_positive_axis() {
        // 32 rpm for one tick at a position with no tooth offset: 9.6 degrees.
        let mut m = machine("create:mechanical_saw", "north", 32.0);
        m.time = 1.0;
        m.pos = [1, 0, 0];
        let shaft = saw_parts(&m).remove(1);
        let angle = 9.6f32.to_radians();
        // North's half shaft points south; a turn about +Z carries its +X
        // side toward +Y.
        let facing_back = centred(facing_z_rotation("south"));
        let expected = spin("z", angle) * facing_back;
        let probe = Vec3::new(0.9, 0.5, 0.6);
        assert!(close(
            shaft.local.transform_point3(probe),
            expected.transform_point3(probe)
        ));
        let turned = shaft.local.transform_point3(Vec3::new(0.7, 0.5, 0.5));
        assert!(turned.y > 0.5 && (turned.z - 0.5).abs() < 1e-4);
    }

    #[test]
    fn harvester_blade_follows_the_facing() {
        let at = |facing| harvester_blade(facing, 0.0).transform_point3(Vec3::new(0.5, 0.5, 1.0));
        assert!(close(at("south"), Vec3::new(0.5, 0.5, 1.0)));
        assert!(close(at("north"), Vec3::new(0.5, 0.5, 0.0)));
        assert!(close(at("east"), Vec3::new(1.0, 0.5, 0.5)));
        assert!(close(at("west"), Vec3::new(0.0, 0.5, 0.5)));
    }

    #[test]
    fn harvester_blade_turns_about_its_west_axis_at_the_pivot() {
        let pivot_point = HARVESTER_PIVOT + Vec3::X * 0.3;
        let turned = harvester_blade("south", 90.0);
        assert!(close(turned.transform_point3(pivot_point), pivot_point));
        // `rotate(.., WEST)`: a point above the pivot swings toward +Z... for
        // a turn about -X, +Y goes to -Z.
        let above = HARVESTER_PIVOT + Vec3::Y * 0.2;
        assert!(close(
            turned.transform_point3(above),
            HARVESTER_PIVOT + Vec3::NEG_Z * 0.2
        ));
        // A placed harvester does not turn.
        let placed = harvester_parts(&machine("create:mechanical_harvester", "south", 0.0));
        assert!(close(placed[0].local.transform_point3(above), above));
    }

    fn rope(offset: f32, running: bool) -> Vec<MachinePart> {
        let models = RopeModels {
            magnet: PULLEY_MAGNET,
            half_magnet: ROPE_HALF_MAGNET,
            half_rope: ROPE_HALF,
            rope: ROPE,
        };
        paid_out_rope(&models, offset, running)
    }

    #[test]
    fn rope_hangs_in_whole_blocks_below_the_magnet() {
        let parts = rope(3.0, true);
        // The magnet at 3, the half rope at the whole step (fraction 0), then
        // ropes at 3 - i - 1 for i < 1.75: 2 and 1.
        assert_eq!(models(&parts), [PULLEY_MAGNET, ROPE_HALF, ROPE, ROPE]);
        let drops: Vec<f32> = parts
            .iter()
            .map(|p| -p.local.transform_point3(Vec3::ZERO).y)
            .collect();
        assert_eq!(drops, [3.0, 0.0, 2.0, 1.0]);
        let below: Vec<i32> = parts.iter().map(|p| p.below).collect();
        assert_eq!(below, [3, 0, 2, 1]);
    }

    #[test]
    fn rope_uses_half_models_at_the_ends_of_a_block_step() {
        // At rest only the short magnet hangs, at the pulley.
        assert_eq!(models(&rope(0.0, false)), [ROPE_HALF_MAGNET]);
        // A magnet too short to be whole at 0.2 stays half.
        assert_eq!(models(&rope(0.2, true)), [ROPE_HALF_MAGNET]);
        // 0.9: whole magnet, half rope pulled up by 0.1.
        let parts = rope(0.9, true);
        assert_eq!(models(&parts), [PULLEY_MAGNET, ROPE_HALF]);
        assert!((parts[1].local.transform_point3(Vec3::ZERO).y - 0.1).abs() < 1e-5);
        // 2.5: the fraction 0.5 is inside no half-rope window; ropes at 1.5 and 0.5.
        assert_eq!(models(&rope(2.5, true)), [PULLEY_MAGNET, ROPE, ROPE]);
        // A stopped rope pulley with the magnet paid out draws nothing.
        assert!(rope(2.5, false).is_empty());
    }

    #[test]
    fn rope_pulley_coil_faces_along_the_shaft() {
        let mut m = machine("create:rope_pulley", "north", 0.0);
        m.axis = "x";
        let parts = rope_pulley_parts(&m);
        assert_eq!(parts[0].model, SHAFT);
        assert_eq!(parts[1].model, ROPE_COIL);
        // `Direction.get(POSITIVE, X)` is east: the coil's +Z goes to +X.
        assert!(close(
            parts[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.75, 0.5, 0.5)
        ));
        // The shaft lies along X.
        assert!(close(
            parts[0].local.transform_point3(Vec3::new(0.5, 0.75, 0.5)),
            Vec3::new(0.75, 0.5, 0.5)
        ));
    }

    #[test]
    fn hose_and_elevator_shafts_run_clockwise_of_the_facing() {
        assert_eq!(clockwise_axis("north"), "x");
        assert_eq!(clockwise_axis("south"), "x");
        assert_eq!(clockwise_axis("east"), "z");
        assert_eq!(clockwise_axis("west"), "z");
        let hose = hose_pulley_parts(&machine("create:hose_pulley", "east", 0.0));
        // Facing east the shaft is along Z.
        assert!(close(
            hose[0].local.transform_point3(Vec3::new(0.5, 0.75, 0.5)),
            Vec3::new(0.5, 0.5, 0.75)
        ));
        // The hose is always out by at least 3/16, magnet whole only past 1/4.
        assert_eq!(hose[2].model, HOSE_HALF_MAGNET);
        assert!((hose[2].local.transform_point3(Vec3::ZERO).y + HOSE_MIN_OFFSET).abs() < 1e-5);
    }

    #[test]
    fn elevator_belt_counts_from_the_offset_and_turns_with_the_facing() {
        let mut m = machine("create:elevator_pulley", "south", 0.0);
        m.state = MachineState {
            offset: 2.0,
            running: true,
            goggles: false,
            source: None,
        };
        let parts = elevator_pulley_parts(&m);
        assert_eq!(
            models(&parts),
            [
                SHAFT,
                ELEVATOR_MAGNET,
                ELEVATOR_COIL,
                ELEVATOR_BELT_HALF,
                ELEVATOR_BELT,
                ELEVATOR_BELT
            ]
        );
        // Belts at offset - i for i < 1.75: 2 and 1.
        assert!((parts[4].local.transform_point3(Vec3::splat(0.5)).y - (0.5 - 2.0)).abs() < 1e-5);
        assert!((parts[5].local.transform_point3(Vec3::splat(0.5)).y - (0.5 - 1.0)).abs() < 1e-5);
        // South: 180 + horizontalAngle(south) = 180: the model's +Z side goes to -Z.
        assert!(close(
            parts[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.5, 0.5 - 2.0, 0.25)
        ));
        // The coil faces the facing: +Z goes south.
        assert!(close(
            parts[2].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.5, 0.5, 0.75)
        ));
        // Without a paid out belt only the magnet and coil are drawn.
        m.state.offset = 0.0;
        assert_eq!(
            models(&elevator_pulley_parts(&m)),
            [SHAFT, ELEVATOR_MAGNET, ELEVATOR_COIL]
        );
    }

    #[test]
    fn arm_rests_with_its_upper_arm_reaching_up_and_forward() {
        let parts = arm_parts(&machine("create:mechanical_arm", "north", 0.0));
        assert_eq!(
            models(&parts),
            [
                ARM_BASE,
                ARM_LOWER_BODY,
                ARM_UPPER_BODY,
                ARM_CLAW_BASE,
                ARM_CLAW_GRIP_LOWER,
                ARM_CLAW_GRIP_UPPER
            ]
        );
        // Base: a quarter above the block's centre... 0.5 + 4/16.
        assert!(close(
            parts[0].local.transform_point3(Vec3::ZERO),
            Vec3::new(0.5, 0.75, 0.5)
        ));
        // Lower body hinge: 2/16 higher, tilted 135 degrees about X. Its
        // model's -Z (the arm's length) points up and toward +Z:
        // Rx(135) sends -Z to (0, -sin(135)... = (0, 0.7071, 0.7071).
        let hinge = Vec3::new(0.5, 0.75 + 2.0 / 16.0, 0.5);
        assert!(close(parts[1].local.transform_point3(Vec3::ZERO), hinge));
        let length = parts[1].local.transform_point3(Vec3::NEG_Z) - hinge;
        let half_root = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(length, Vec3::new(0.0, half_root, half_root)));
        // Upper arm pivot: 14/16 along the lower arm's -Z.
        let upper_pivot = hinge + length * (14.0 / 16.0);
        assert!(close(
            parts[2].local.transform_point3(Vec3::ZERO),
            upper_pivot
        ));
        // Upper body is rotated -135 more, so its -Z points straight forward
        // (north, -Z): 135 - 135 = 0 net.
        let upper_length = parts[2].local.transform_point3(Vec3::NEG_Z) - upper_pivot;
        assert!(close(upper_length, Vec3::NEG_Z));
        // The head hangs 15/16 further, tilted another 45 down.
        let head_pivot = upper_pivot + Vec3::NEG_Z * (15.0 / 16.0);
        assert!(close(
            parts[3].local.transform_point3(Vec3::ZERO),
            head_pivot
        ));
        // Grips: lower grip 1/16 down, upper grip 1/16 up, 6/16 along the head's -Z.
        let head_forward = parts[3].local.transform_vector3(Vec3::NEG_Z);
        let head_up = parts[3].local.transform_vector3(Vec3::Y);
        assert!(close(
            parts[4].local.transform_point3(Vec3::ZERO),
            head_pivot + head_forward * (6.0 / 16.0) - head_up / 16.0
        ));
        assert!(close(
            parts[5].local.transform_point3(Vec3::ZERO),
            head_pivot + head_forward * (6.0 / 16.0) + head_up / 16.0
        ));
    }

    #[test]
    fn ceiling_arm_hangs_upside_down_and_goggles_pick_the_goggle_claw() {
        let mut m = machine("create:mechanical_arm", "north", 0.0);
        m.ceiling = true;
        let parts = arm_parts(&m);
        // Rx(180) about the centre: the base sits 4/16 below the centre.
        assert!(close(
            parts[0].local.transform_point3(Vec3::ZERO),
            Vec3::new(0.5, 0.25, 0.5)
        ));
        assert_eq!(parts[3].model, ARM_CLAW_BASE);
        m.state.goggles = true;
        let goggled = arm_parts(&m);
        assert_eq!(goggled[3].model, ARM_CLAW_BASE_GOGGLES);
        // An inverted goggle claw is turned half way about its own Z axis;
        // the grips are not.
        let plain = arm_parts(&{
            let mut other = machine("create:mechanical_arm", "north", 0.0);
            other.ceiling = true;
            other
        });
        let probe = Vec3::new(0.1, 0.2, 0.3);
        assert!(!close(
            goggled[3].local.transform_point3(probe),
            plain[3].local.transform_point3(probe)
        ));
        assert!(close(
            goggled[4].local.transform_point3(probe),
            plain[4].local.transform_point3(probe)
        ));
    }

    #[test]
    fn arm_claw_widens_for_flat_and_block_items_and_rave_tints_bodies() {
        let mut empty = machine("create:mechanical_arm", "north", 0.0);
        let empty_parts = arm_parts(&empty);
        let mut flat = machine("create:mechanical_arm", "north", 0.0);
        flat.arm_has_item = true;
        let flat_parts = arm_parts(&flat);
        let mut block = machine("create:mechanical_arm", "north", 0.0);
        block.arm_has_item = true;
        block.arm_is_block_item = true;
        let block_parts = arm_parts(&block);

        let grip_sep = |parts: &[MachinePart]| {
            let lo = parts[4].local.transform_point3(Vec3::ZERO);
            let hi = parts[5].local.transform_point3(Vec3::ZERO);
            (hi - lo).length()
        };
        assert!((grip_sep(&empty_parts) - 2.0 / 16.0).abs() < 1e-4);
        assert!((grip_sep(&flat_parts) - 10.0 / 64.0).abs() < 1e-4);
        assert!((grip_sep(&block_parts) - 6.0 / 16.0).abs() < 1e-4);

        let mut arm_state = ArmAnimState::default();
        arm_state.phase = horizon_client::world::ArmPhase::Dancing;
        empty.arm = Some(&arm_state);
        empty.speed = 32.0;
        empty.time = 5.0;
        let rave_parts = arm_parts(&empty);
        assert!(rave_parts[0].tint.is_none());
        assert!(rave_parts[1].tint.is_some());
        assert_eq!(rave_parts[1].tint, rave_parts[2].tint);
        assert!(rave_parts[3].tint.is_none());
    }

    #[test]
    fn gearbox_draws_four_perpendicular_shafts_and_reverses_from_source() {
        let mut m = machine("create:gearbox", "north", 32.0);
        m.axis = "y";
        m.pos = [1, 0, 0];
        m.time = 1.0;
        m.state.source = Some([1, 0, -1]); // north (-Z)
        let parts = gearbox_parts(&m);
        assert_eq!(parts.len(), 4);
        for p in &parts {
            assert_eq!(p.model, SHAFT_HALF);
        }
        // Parts order: north (-Z), south (+Z), west (-X), east (+X).
        let probe = Vec3::new(0.7, 0.6, 0.75);
        let expected = [
            spin("z", 9.6f32.to_radians()) * centred(facing_z_rotation("north")),
            spin("z", (-9.6f32).to_radians()) * centred(facing_z_rotation("south")),
            spin("x", (-9.6f32 + 22.5).to_radians()) * centred(facing_z_rotation("west")),
            spin("x", (9.6f32 + 22.5).to_radians()) * centred(facing_z_rotation("east")),
        ];
        for (part, expected_mat) in parts.iter().zip(expected) {
            assert!(close(
                part.local.transform_point3(probe),
                expected_mat.transform_point3(probe)
            ));
        }
    }

    #[test]
    fn sequenced_gearshift_switches_between_horizontal_and_vertical_axis() {
        let mut m = machine("create:sequenced_gearshift", "north", 0.0);
        m.axis = "x";
        let horiz = sequenced_gearshift_parts(&m);
        assert_eq!(models(&horiz), [SHAFT_HALF, SHAFT_HALF]);
        assert!(close(
            horiz[0].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.25, 0.5, 0.5)
        ));
        assert!(close(
            horiz[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.75, 0.5, 0.5)
        ));

        m.vertical = true;
        let vert = sequenced_gearshift_parts(&m);
        assert!(close(
            vert[0].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.5, 0.25, 0.5)
        ));
        assert!(close(
            vert[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.75)),
            Vec3::new(0.5, 0.75, 0.5)
        ));
    }

    #[test]
    fn speed_controller_draws_shaft_and_optional_bracket_above() {
        let mut m = machine("create:rotation_speed_controller", "north", 0.0);
        m.axis = "x";
        assert_eq!(models(&speed_controller_parts(&m)), [SHAFT]);

        m.has_controller_bracket = true;
        let parts = speed_controller_parts(&m);
        assert_eq!(models(&parts), [SHAFT, SPEED_CONTROLLER_BRACKET]);
        assert_eq!(parts[1].below, -1);
        assert!(close(
            parts[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.5)),
            Vec3::new(0.5, 1.5, 0.5)
        ));
    }

    #[test]
    fn backtanks_draw_shaft_input_and_pivoted_cogs() {
        let mut copper = machine("create:copper_backtank", "south", 32.0);
        copper.time = 2.0;
        let cp = backtank_parts(&copper);
        assert_eq!(models(&cp), [COPPER_BACKTANK_SHAFT, COPPER_BACKTANK_COGS]);
        // At 32 rpm and time=2, cogs rotate 32/4 * 2 = 16 deg around +X at (8/16, 6.5/16, 11/16).
        let copper_rest = backtank_parts(&machine("create:copper_backtank", "south", 0.0));
        let probe_pivot = Vec3::new(0.5, 6.5 / 16.0, 11.0 / 16.0);
        assert!(close(
            cp[1].local.transform_point3(probe_pivot),
            copper_rest[1].local.transform_point3(probe_pivot)
        ));
        let non_pivot = Vec3::new(0.5, 9.0 / 16.0, 11.0 / 16.0);
        assert!(!close(
            cp[1].local.transform_point3(non_pivot),
            copper_rest[1].local.transform_point3(non_pivot)
        ));

        let netherite = machine("create:netherite_backtank", "north", 0.0);
        assert_eq!(
            models(&backtank_parts(&netherite)),
            [NETHERITE_BACKTANK_SHAFT, NETHERITE_BACKTANK_COGS]
        );
    }

    #[test]
    fn crafter_and_display_board_orient_shaftless_cog_to_facing() {
        let mut crafter = machine("create:mechanical_crafter", "east", 0.0);
        crafter.pos = [1, 0, 0];
        let cp = crafter_parts(&crafter);
        assert_eq!(models(&cp), [SHAFTLESS_COGWHEEL]);
        // Native +Y axis of shaftless cog aligns with X when facing east.
        assert!(close(
            cp[0].local.transform_point3(Vec3::new(0.5, 0.75, 0.5)),
            Vec3::new(0.75, 0.5, 0.5)
        ));

        let mut board = machine("create:display_board", "north", 0.0);
        board.pos = [1, 0, 0];
        let bp = display_board_parts(&board);
        assert_eq!(models(&bp), [SHAFTLESS_COGWHEEL]);
        // Native +Y axis of shaftless cog aligns along Z (facing north sends +Y to -Z).
        assert!(close(
            bp[0].local.transform_point3(Vec3::new(0.5, 0.75, 0.5)),
            Vec3::new(0.5, 0.5, 0.25)
        ));
    }

    #[test]
    fn gantry_carriage_draws_shaft_and_half_speed_wheels() {
        let mut m = machine("create:gantry_carriage", "up", 32.0);
        m.along_first = false;
        m.time = 2.0;
        let parts = gantry_carriage_parts(&m);
        assert_eq!(models(&parts), [SHAFT, GANTRY_COGS]);
        // Wheel pivot at (0.5, 0.5 - 9/16, 0.5) in local coords stays fixed while spinning.
        let mut rest = m;
        rest.time = 0.0;
        let rest_parts = gantry_carriage_parts(&rest);
        let pivot = Vec3::new(0.5, 0.5 - 9.0 / 16.0, 0.5);
        assert!(close(
            parts[1].local.transform_point3(pivot),
            rest_parts[1].local.transform_point3(pivot)
        ));
        assert!(!close(
            parts[1].local.transform_point3(Vec3::new(0.5, 0.5, 0.5)),
            rest_parts[1]
                .local
                .transform_point3(Vec3::new(0.5, 0.5, 0.5))
        ));
    }

    #[test]
    fn every_machine_block_draws_something() {
        for block in [
            "create:mechanical_saw",
            "create:mechanical_harvester",
            "create:rope_pulley",
            "create:hose_pulley",
            "create:elevator_pulley",
            "create:mechanical_arm",
            "create:gearbox",
            "create:sequenced_gearshift",
            "create:rotation_speed_controller",
            "create:copper_backtank",
            "create:netherite_backtank",
            "create:mechanical_crafter",
            "create:display_board",
            "create:gantry_carriage",
        ] {
            assert!(is_machine(block), "{block}");
            let mut m = machine(block, "north", 0.0);
            m.axis = "x";
            assert!(!machine_parts(&m).is_empty(), "{block}");
        }
        assert!(!is_machine("create:shaft"));
    }

    #[test]
    fn pulleys_apply_coil_and_belt_sprite_shifts_when_paid_out() {
        let mut rope_rest = machine("create:rope_pulley", "north", 0.0);
        rope_rest.axis = "x";
        assert!(rope_pulley_parts(&rope_rest)[1].sprite_shift.is_none());

        let mut rope_out = rope_rest;
        rope_out.state.offset = 2.5;
        rope_out.state.running = true;
        let rope_parts = rope_pulley_parts(&rope_out);
        let rope_shift = rope_parts[1].sprite_shift.expect("rope coil scroll");
        assert_eq!(rope_shift.original, "create:block/rope_pulley_coil");
        assert_eq!(rope_shift.target, "create:block/rope_pulley_coil_scroll");
        assert!((rope_shift.scroll_v - coil_scroll_v(2.5, 1.0)).abs() < 1e-6);

        let hose = hose_pulley_parts(&machine("create:hose_pulley", "east", 0.0));
        let hose_shift = hose[1].sprite_shift.expect("hose coil always >= 3/16");
        assert_eq!(hose_shift.original, "create:block/hose_pulley_coil");
        assert_eq!(hose_shift.target, "create:block/hose_pulley_coil_scroll");

        let mut elev = machine("create:elevator_pulley", "south", 0.0);
        elev.state.offset = 2.5;
        elev.state.running = true;
        let elev_parts = elevator_pulley_parts(&elev);
        let coil_shift = elev_parts[2].sprite_shift.expect("elevator coil scroll");
        assert_eq!(
            coil_shift.target,
            "create:block/elevator_pulley_coil_scroll"
        );
        assert!((coil_shift.scroll_v - coil_scroll_v(2.5, 2.0)).abs() < 1e-6);
        let belt_shift = elev_parts[3].sprite_shift.expect("elevator belt scroll");
        assert_eq!(
            belt_shift.target,
            "create:block/elevator_pulley_belt_scroll"
        );
        assert!((belt_shift.scroll_v - elevator_belt_scroll_v(2.5)).abs() < 1e-6);
    }
}
