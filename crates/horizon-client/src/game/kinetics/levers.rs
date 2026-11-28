//! Create Group B blockstate/NBT-driven animations (fluid valves, valve handles,
//! analog levers, funnel flaps, weighted ejectors, stickers, toolboxes) plus
//! packagers/repackagers and portable storage/fluid interfaces.
//!
//! Ported from `Creators-of-Create/Create` (`FluidValveRenderer`,
//! `AnalogLeverRenderer`, `FunnelRenderer`, `FlapStuffs`, `EjectorRenderer`,
//! `StickerRenderer`, `ToolboxRenderer`, `PackagerRenderer`,
//! `PortableStorageInterfaceRenderer`, MIT) and `Creators-of-Create/Ponder` (MIT).

use super::logistics::{self, ItemPoseStyle, WorldItemDraw, depot_item_draws, parse_depot_items};
use super::machines::{MachinePart, centred, clockwise_axis, horizontal_angle, part, spin};
use super::{angle_radians, axis_y_rotation, instruments, opposite_facing, rotation_offset};
use glam::{Mat4, Quat, Vec3};
use horizon_client::world::{KineticAuxState, LerpedValue};
use horizon_world::nbt::{JavaString, Tag};

const FLUID_VALVE_POINTER: &str = "create:block/fluid_valve/pointer";
const ANALOG_LEVER_HANDLE: &str = "create:block/analog_lever/handle";
const ANALOG_LEVER_INDICATOR: &str = "create:block/analog_lever/indicator";
const FUNNEL_FLAP: &str = "create:block/funnel/flap";
const BELT_FUNNEL_FLAP: &str = "create:block/belt_funnel/flap";
const EJECTOR_TOP: &str = "create:block/weighted_ejector/top";
const STICKER_HEAD: &str = "create:block/sticker/head";
const TOOLBOX_DRAWER: &str = "create:block/toolbox/drawer";
const PACKAGER_HATCH_OPEN: &str = "create:block/packager/hatch_open";
const PACKAGER_HATCH_CLOSED: &str = "create:block/packager/hatch_closed";
const PACKAGER_TRAY: &str = "create:block/packager/tray";
const REPACKAGER_TRAY: &str = "create:block/repackager/tray";
const PSI_MIDDLE: &str = "create:block/portable_storage_interface/block_middle";
const PSI_MIDDLE_POWERED: &str = "create:block/portable_storage_interface/block_middle_powered";
const PSI_TOP: &str = "create:block/portable_storage_interface/block_top";
const PFI_MIDDLE: &str = "create:block/portable_fluid_interface/block_middle";
const PFI_MIDDLE_POWERED: &str = "create:block/portable_fluid_interface/block_middle_powered";
const PFI_TOP: &str = "create:block/portable_fluid_interface/block_top";
const PEI_MIDDLE: &str = "createaddition:block/portable_energy_interface/block_middle";
const PEI_MIDDLE_POWERED: &str =
    "createaddition:block/portable_energy_interface/block_middle_powered";
const PEI_TOP: &str = "createaddition:block/portable_energy_interface/block_top";
const SHAFT: &str = "create:block/shaft";

const PSI_ANIMATION_TICKS: i32 = 4;
const PSI_TRANSFER_TIMEOUT: i32 = 15;
const PACKAGER_CYCLE: i32 = 20;

/// Inputs for rendering one auxiliary Create animated block.
pub(super) struct AuxBlock<'a> {
    pub block: &'a str,
    pub facing: &'a str,
    pub face: &'a str,
    pub shape: &'a str,
    pub along_first: bool,
    pub enabled: bool,
    pub extended: bool,
    pub pos: [i32; 3],
    pub speed: f32,
    pub time: f64,
    pub alpha: f64,
    pub aux: Option<&'a KineticAuxState>,
}

/// Whether `block` is any of Create's 17 copper or dyed valve handles.
pub(super) fn is_valve_handle(block: &str) -> bool {
    matches!(
        block,
        "create:copper_valve_handle"
            | "create:black_valve_handle"
            | "create:blue_valve_handle"
            | "create:brown_valve_handle"
            | "create:cyan_valve_handle"
            | "create:gray_valve_handle"
            | "create:green_valve_handle"
            | "create:light_blue_valve_handle"
            | "create:light_gray_valve_handle"
            | "create:lime_valve_handle"
            | "create:magenta_valve_handle"
            | "create:orange_valve_handle"
            | "create:pink_valve_handle"
            | "create:purple_valve_handle"
            | "create:red_valve_handle"
            | "create:white_valve_handle"
            | "create:yellow_valve_handle"
    )
}

/// Partial model path for the lid of any of Create's 16 dyed toolboxes.
pub(super) fn toolbox_lid_model(block: &str) -> Option<&'static str> {
    Some(match block {
        "create:black_toolbox" => "create:block/toolbox/lid/black",
        "create:blue_toolbox" => "create:block/toolbox/lid/blue",
        "create:brown_toolbox" => "create:block/toolbox/lid/brown",
        "create:cyan_toolbox" => "create:block/toolbox/lid/cyan",
        "create:gray_toolbox" => "create:block/toolbox/lid/gray",
        "create:green_toolbox" => "create:block/toolbox/lid/green",
        "create:light_blue_toolbox" => "create:block/toolbox/lid/light_blue",
        "create:light_gray_toolbox" => "create:block/toolbox/lid/light_gray",
        "create:lime_toolbox" => "create:block/toolbox/lid/lime",
        "create:magenta_toolbox" => "create:block/toolbox/lid/magenta",
        "create:orange_toolbox" => "create:block/toolbox/lid/orange",
        "create:pink_toolbox" => "create:block/toolbox/lid/pink",
        "create:purple_toolbox" => "create:block/toolbox/lid/purple",
        "create:red_toolbox" => "create:block/toolbox/lid/red",
        "create:white_toolbox" => "create:block/toolbox/lid/white",
        "create:yellow_toolbox" => "create:block/toolbox/lid/yellow",
        _ => return None,
    })
}

pub(super) fn is_funnel(block: &str) -> bool {
    matches!(
        block,
        "create:andesite_funnel"
            | "create:brass_funnel"
            | "create:andesite_belt_funnel"
            | "create:brass_belt_funnel"
    )
}

/// Whether `block` is handled by `aux_parts_and_item`.
pub(super) fn is_aux_block(block: &str) -> bool {
    matches!(
        block,
        "create:fluid_valve"
            | "create:analog_lever"
            | "create:weighted_ejector"
            | "create:sticker"
            | "create:packager"
            | "create:repackager"
            | "create:portable_storage_interface"
            | "create:portable_fluid_interface"
            | "createaddition:portable_energy_interface"
    ) || is_funnel(block)
        || toolbox_lid_model(block).is_some()
}

/// Whether a block entity packet belongs to one of the Create Group B / logistics / PSI types.
pub(super) fn is_aux_be_tag(name: &str, tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    if fields.is_empty() {
        return false;
    }
    matches!(
        name,
        "create:analog_lever"
            | "create:fluid_valve"
            | "create:weighted_ejector"
            | "create:sticker"
            | "create:toolbox"
            | "create:funnel"
            | "create:packager"
            | "create:repackager"
            | "create:portable_storage_interface"
            | "create:portable_fluid_interface"
            | "createaddition:portable_energy_interface"
    )
}

fn read_lerped_float(tag: &Tag, existing: Option<LerpedValue>, exp: bool) -> Option<LerpedValue> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let value = match fields.get(&JavaString::from("Value"))? {
        Tag::Float(bits) => f32::from_bits(*bits),
        _ => return None,
    };
    let target = match fields.get(&JavaString::from("Target")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => value,
    };
    let speed = match fields.get(&JavaString::from("Speed")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    let forced = matches!(fields.get(&JavaString::from("Force")), Some(Tag::Byte(v)) if *v != 0);
    let mut lerped = match existing {
        Some(prev) if !forced => prev,
        _ => LerpedValue::start_with(value),
    };
    lerped.target = target;
    lerped.speed = speed;
    lerped.exp = exp;
    Some(lerped)
}

fn psi_progress(timer: i32, powered: bool) -> f32 {
    if powered || timer <= 0 {
        return 0.0;
    }
    if timer >= PSI_ANIMATION_TICKS && timer <= PSI_TRANSFER_TIMEOUT + PSI_ANIMATION_TICKS {
        1.0
    } else if timer >= PSI_TRANSFER_TIMEOUT + PSI_ANIMATION_TICKS {
        let t = (timer - PSI_TRANSFER_TIMEOUT - PSI_ANIMATION_TICKS) as f32
            / PSI_ANIMATION_TICKS as f32;
        (1.0 - t).clamp(0.0, 1.0)
    } else {
        (timer as f32 / PSI_ANIMATION_TICKS as f32).clamp(0.0, 1.0)
    }
}

/// Parses an update tag into a `KineticAuxState`, preserving chaser continuity when `existing` matches.
pub(super) fn aux_state_from_nbt(
    existing: Option<&KineticAuxState>,
    tag: &Tag,
) -> Option<KineticAuxState> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    if let Some(pointer_tag) = fields.get(&JavaString::from("Pointer")) {
        let prev = match existing {
            Some(KineticAuxState::FluidValve { pointer }) => Some(*pointer),
            _ => None,
        };
        return Some(KineticAuxState::FluidValve {
            pointer: read_lerped_float(pointer_tag, prev, false)?,
        });
    }
    if let Some(lid_tag) = fields.get(&JavaString::from("Lid")) {
        let prev = match existing {
            Some(KineticAuxState::Ejector { lid, .. }) => Some(*lid),
            _ => None,
        };
        return Some(KineticAuxState::Ejector {
            lid: read_lerped_float(lid_tag, prev, false)?,
            depot: parse_depot_items(fields),
        });
    }
    if fields.contains_key(&JavaString::from("AnimationTicks"))
        || fields.contains_key(&JavaString::from("HeldBox"))
        || fields.contains_key(&JavaString::from("InsertedBox"))
    {
        let animation_ticks = match fields.get(&JavaString::from("AnimationTicks")) {
            Some(Tag::Int(v)) => (*v).max(0),
            _ => 0,
        };
        let animation_inward = matches!(
            fields.get(&JavaString::from("AnimationInward")),
            Some(Tag::Byte(v)) if *v != 0
        );
        let held_box = fields
            .get(&JavaString::from("HeldBox"))
            .and_then(horizon_items::item_nbt::item_from_nbt);
        let inserted_box = fields
            .get(&JavaString::from("InsertedBox"))
            .and_then(horizon_items::item_nbt::item_from_nbt);
        return Some(KineticAuxState::Packager {
            animation_ticks,
            animation_inward,
            held_box,
            inserted_box,
        });
    }
    if let (Some(Tag::Int(timer)), Some(Tag::Float(dist_bits))) = (
        fields.get(&JavaString::from("Timer")),
        fields.get(&JavaString::from("Distance")),
    ) {
        let powered =
            matches!(fields.get(&JavaString::from("Powered")), Some(Tag::Byte(v)) if *v != 0);
        let distance = f32::from_bits(*dist_bits).clamp(0.0, 2.0);
        let transfer_timer = (*timer).max(0);
        let animation = match existing {
            Some(KineticAuxState::PortableInterface { animation, .. }) => *animation,
            _ => LerpedValue::start_with(psi_progress(transfer_timer, powered)),
        };
        return Some(KineticAuxState::PortableInterface {
            transfer_timer,
            distance,
            powered,
            animation,
        });
    }
    if let Some(Tag::Int(open_count)) = fields.get(&JavaString::from("OpenCount")) {
        let (lid, drawers) = match existing {
            Some(KineticAuxState::Toolbox { lid, drawers, .. }) => (*lid, *drawers),
            _ => {
                let init = if *open_count > 0 { 1.0 } else { 0.0 };
                (LerpedValue::start_with(init), LerpedValue::start_with(init))
            }
        };
        return Some(KineticAuxState::Toolbox {
            open_count: *open_count,
            lid,
            drawers,
        });
    }
    if let Some(Tag::Int(level)) = fields.get(&JavaString::from("State"))
        && fields.contains_key(&JavaString::from("ChangeTimer"))
    {
        let target = (*level as f32).clamp(0.0, 15.0);
        let lv = match existing {
            Some(KineticAuxState::AnalogLever { level }) => {
                let mut next = *level;
                next.chase_exp(target, 0.2);
                next
            }
            _ => LerpedValue::start_with(target),
        };
        return Some(KineticAuxState::AnalogLever { level: lv });
    }
    None
}

/// Advances an auxiliary block entity's animation state by one client tick.
pub(super) fn tick_aux_state(state: &mut KineticAuxState, extended_prop: bool) {
    match state {
        KineticAuxState::FluidValve { pointer } => pointer.tick(),
        KineticAuxState::Funnel { flap } => flap.tick(),
        KineticAuxState::AnalogLever { level } => level.tick(),
        KineticAuxState::Ejector { lid, .. } => lid.tick(),
        KineticAuxState::Sticker { piston } => {
            piston.chase_linear(if extended_prop { 1.0 } else { 0.0 }, 0.4);
            piston.tick();
        }
        KineticAuxState::Toolbox {
            open_count,
            lid,
            drawers,
        } => {
            let target = if *open_count > 0 { 1.0 } else { 0.0 };
            lid.chase_linear(target, 0.2);
            drawers.chase_exp(target, 0.2);
            lid.tick();
            drawers.tick();
        }
        KineticAuxState::Packager {
            animation_ticks, ..
        } => {
            if *animation_ticks > 0 {
                *animation_ticks -= 1;
            }
        }
        KineticAuxState::PortableInterface {
            transfer_timer,
            powered,
            animation,
            ..
        } => {
            if *transfer_timer > 0 {
                *transfer_timer -= 1;
                if *powered {
                    *transfer_timer = 0;
                }
            }
            animation.set_value(psi_progress(*transfer_timer, *powered));
        }
    }
}

fn axis_index(axis: &str) -> usize {
    match axis {
        "x" => 0,
        "z" => 2,
        _ => 1,
    }
}

fn shaft_part(ctx: &AuxBlock, axis: &str) -> MachinePart {
    let angle = angle_radians(
        ctx.time,
        ctx.speed,
        rotation_offset(axis_index(axis), ctx.pos),
    );
    part(SHAFT, spin(axis, angle) * centred(axis_y_rotation(axis)))
}

pub(super) fn to_y_rot(facing: &str) -> f32 {
    match facing {
        "south" => 0.0,
        "west" => 90.0,
        "north" => 180.0,
        "east" => 270.0,
        _ => 0.0,
    }
}

pub(super) fn vertical_angle(facing: &str) -> f32 {
    match facing {
        "up" => -90.0,
        "down" => 90.0,
        _ => 0.0,
    }
}

pub(super) fn dir_normal_vec3(facing: &str) -> Vec3 {
    match facing {
        "down" => Vec3::NEG_Y,
        "up" => Vec3::Y,
        "north" => Vec3::NEG_Z,
        "south" => Vec3::Z,
        "west" => Vec3::NEG_X,
        "east" => Vec3::X,
        _ => Vec3::ZERO,
    }
}

fn mix_rgb(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |shift: u32| {
        let a = ((from >> shift) & 0xFF) as f32;
        let b = ((to >> shift) & 0xFF) as f32;
        (a + (b - a) * t).round().clamp(0.0, 255.0) as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn fluid_valve_parts(ctx: &AuxBlock) -> Vec<MachinePart> {
    let shaft_axis = instruments::directional_axis(ctx.facing, ctx.along_first).unwrap_or("y");
    let pipe_axis = instruments::directional_axis(ctx.facing, !ctx.along_first).unwrap_or("z");
    let val = match ctx.aux {
        Some(KineticAuxState::FluidValve { pointer }) => pointer.get(ctx.alpha).clamp(0.0, 1.0),
        _ => {
            if ctx.enabled {
                1.0
            } else {
                0.0
            }
        }
    };
    let pointer_rot = -90.0 * val;
    let offset = if (pipe_axis != "y" && shaft_axis == "x") || pipe_axis == "y" {
        90.0
    } else {
        0.0
    };
    let x_deg: f32 = match ctx.facing {
        "up" => 0.0,
        "down" => 180.0,
        _ => 90.0,
    };
    let pointer_local = centred(
        Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
            * Quat::from_rotation_x(x_deg.to_radians())
            * Quat::from_rotation_y((offset + pointer_rot).to_radians()),
    );
    vec![
        shaft_part(ctx, shaft_axis),
        part(FLUID_VALVE_POINTER, pointer_local),
    ]
}

fn analog_lever_parts(ctx: &AuxBlock) -> Vec<MachinePart> {
    let val = match ctx.aux {
        Some(KineticAuxState::AnalogLever { level }) => level.get(ctx.alpha).clamp(0.0, 15.0),
        _ => 0.0,
    };
    let rx_deg: f32 = match ctx.face {
        "floor" => 0.0,
        "wall" => 90.0,
        _ => 180.0,
    };
    let base = centred(
        Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
            * Quat::from_rotation_x(rx_deg.to_radians()),
    );
    let handle_angle = ((val / 15.0) * 90.0).to_radians();
    let handle_local = base
        * Mat4::from_translation(Vec3::new(0.5, 1.0 / 16.0, 0.5))
        * Mat4::from_rotation_x(handle_angle)
        * Mat4::from_translation(Vec3::new(-0.5, -1.0 / 16.0, -0.5));
    let mut indicator = part(ANALOG_LEVER_INDICATOR, base);
    indicator.tint = Some(mix_rgb(0x2C0300, 0xCD0000, val / 15.0));
    vec![part(ANALOG_LEVER_HANDLE, handle_local), indicator]
}

fn funnel_parts(ctx: &AuxBlock) -> Vec<MachinePart> {
    if !matches!(ctx.facing, "north" | "south" | "east" | "west") {
        return Vec::new();
    }
    let is_belt = ctx.block.contains("belt_funnel");
    let model = if is_belt {
        BELT_FUNNEL_FLAP
    } else {
        FUNNEL_FLAP
    };
    let flap_offset = if !is_belt {
        -1.0 / 16.0
    } else {
        match ctx.shape {
            "extended" => 8.0 / 16.0,
            "pulling" | "pushing" => -2.0 / 16.0,
            _ => 0.0,
        }
    };
    let base = centred(Quat::from_rotation_y(
        horizontal_angle(opposite_facing(ctx.facing)).to_radians(),
    )) * Mat4::from_translation(Vec3::new(0.075 / 16.0, 0.0, -flap_offset));
    let flapness = match ctx.aux {
        Some(KineticAuxState::Funnel { flap }) => flap.get(ctx.alpha),
        _ => 0.0,
    };
    let pivot = Vec3::new(0.0, 10.0 / 16.0, 9.5 / 16.0);
    (0..4)
        .map(|segment| {
            let x_step = segment as f32 * (-3.05 / 16.0);
            let local = base
                * Mat4::from_translation(Vec3::new(x_step, 0.0, 0.0))
                * Mat4::from_translation(pivot)
                * Mat4::from_rotation_x(logistics::flap_angle(flapness, segment).to_radians())
                * Mat4::from_translation(-pivot);
            part(model, local)
        })
        .collect()
}

fn weighted_ejector_parts_and_items(ctx: &AuxBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let shaft_axis = clockwise_axis(ctx.facing);
    let (lid_progress, depot) = match ctx.aux {
        Some(KineticAuxState::Ejector { lid, depot }) => {
            (lid.get(ctx.alpha).clamp(0.0, 1.0), Some(depot))
        }
        _ => (0.0, None),
    };
    let angle = lid_progress * 70.0;
    let pivot = Vec3::new(0.0, 11.25 / 16.0, 0.75 / 16.0);
    let facing_rot = centred(Quat::from_rotation_y(
        (180.0 + horizontal_angle(ctx.facing)).to_radians(),
    ));
    let hinge = facing_rot
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x((-angle).to_radians())
        * Mat4::from_translation(-pivot);
    let items = match depot {
        Some(depot) => depot_item_draws(
            depot,
            ctx.alpha,
            hinge
                * centred(Quat::from_rotation_y(
                    (-180.0 - horizontal_angle(ctx.facing)).to_radians(),
                )),
        ),
        None => Vec::new(),
    };
    (
        vec![shaft_part(ctx, shaft_axis), part(EJECTOR_TOP, hinge)],
        items,
    )
}

fn sticker_parts(ctx: &AuxBlock) -> Vec<MachinePart> {
    let offset = match ctx.aux {
        Some(KineticAuxState::Sticker { piston }) => piston.get(ctx.alpha).clamp(0.0, 1.0),
        _ => {
            if ctx.extended {
                1.0
            } else {
                0.0
            }
        }
    };
    let head_local =
        centred(
            Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
                * Quat::from_rotation_x((vertical_angle(ctx.facing) + 90.0).to_radians()),
        ) * Mat4::from_translation(Vec3::new(0.0, (offset * offset) * 4.0 / 16.0, 0.0));
    vec![part(STICKER_HEAD, head_local)]
}

fn toolbox_parts(ctx: &AuxBlock, lid_model: &'static str) -> Vec<MachinePart> {
    let (lid_val, drawers_val) = match ctx.aux {
        Some(KineticAuxState::Toolbox { lid, drawers, .. }) => {
            (lid.get(ctx.alpha), drawers.get(ctx.alpha))
        }
        _ => (0.0, 0.0),
    };
    let opp_y_rot = to_y_rot(opposite_facing(ctx.facing));
    let base = centred(Quat::from_rotation_y((-opp_y_rot).to_radians()));
    let lid_local = base
        * Mat4::from_translation(Vec3::new(0.0, 6.0 / 16.0, 12.0 / 16.0))
        * Mat4::from_rotation_x((135.0 * lid_val).to_radians())
        * Mat4::from_translation(Vec3::new(0.0, -6.0 / 16.0, -12.0 / 16.0));
    let mut parts = vec![part(lid_model, lid_local)];
    for offset in [0, 1] {
        let z_shift = -drawers_val * 0.175 * (2 - offset) as f32;
        let drawer_local =
            base * Mat4::from_translation(Vec3::new(0.0, offset as f32 / 8.0, z_shift));
        parts.push(part(TOOLBOX_DRAWER, drawer_local));
    }
    parts
}

fn packager_parts_and_items(ctx: &AuxBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let opp = opposite_facing(ctx.facing);
    let opp_normal = dir_normal_vec3(opp);
    let opp_y_rot = to_y_rot(opp);

    let (anim_ticks, inward, held_box, inserted_box) = match ctx.aux {
        Some(KineticAuxState::Packager {
            animation_ticks,
            animation_inward,
            held_box,
            inserted_box,
        }) => (
            *animation_ticks,
            *animation_inward,
            held_box.as_ref(),
            inserted_box.as_ref(),
        ),
        _ => (0, true, None, None),
    };

    let hatch_open = anim_ticks > if inward { 1 } else { 5 }
        && anim_ticks < PACKAGER_CYCLE - if inward { 5 } else { 1 };
    let hatch_model = if hatch_open {
        PACKAGER_HATCH_OPEN
    } else {
        PACKAGER_HATCH_CLOSED
    };
    let hatch_local = Mat4::from_translation(opp_normal * 0.49999)
        * centred(
            Quat::from_rotation_y(horizontal_angle(opp).to_radians())
                * Quat::from_rotation_x(vertical_angle(opp).to_radians()),
        );

    let tick_cycle = if inward {
        anim_ticks as f32 - ctx.alpha as f32
    } else {
        anim_ticks as f32 - 5.0 - ctx.alpha as f32
    };
    let p = (tick_cycle / (PACKAGER_CYCLE - 5) as f32 * 2.0 - 1.0).clamp(-1.0, 1.0);
    let p = 1.0 - p * p;
    let tray_offset = p * p;
    let tray_model = if ctx.block == "create:packager" {
        PACKAGER_TRAY
    } else {
        REPACKAGER_TRAY
    };
    let tray_local = Mat4::from_translation(opp_normal * tray_offset)
        * centred(Quat::from_rotation_y(opp_y_rot.to_radians()));

    let rendered_box = if inward {
        if anim_ticks <= PACKAGER_CYCLE / 2 {
            None
        } else {
            inserted_box.cloned()
        }
    } else if anim_ticks >= PACKAGER_CYCLE / 2 {
        None
    } else {
        held_box.cloned()
    };
    let items = rendered_box
        .map(|stack| WorldItemDraw {
            stack,
            local: Mat4::from_translation(opp_normal * tray_offset)
                * Mat4::from_translation(Vec3::splat(0.5))
                * Mat4::from_rotation_y(opp_y_rot.to_radians())
                * Mat4::from_translation(Vec3::new(0.0, 2.0 / 16.0, 0.0))
                * Mat4::from_scale(Vec3::splat(1.49)),
            light_offset: [0, 0, 0],
            display: horizon_assets::item_model::DisplayContext::Fixed,
            style: ItemPoseStyle::Raw,
        })
        .into_iter()
        .collect();

    (
        vec![part(hatch_model, hatch_local), part(tray_model, tray_local)],
        items,
    )
}

fn portable_interface_parts(ctx: &AuxBlock) -> Vec<MachinePart> {
    let is_fluid = ctx.block == "create:portable_fluid_interface";
    let is_energy = ctx.block == "createaddition:portable_energy_interface";
    let (connected, ext) = match ctx.aux {
        Some(KineticAuxState::PortableInterface {
            transfer_timer,
            distance,
            powered,
            animation,
        }) => {
            let conn = !*powered
                && *transfer_timer >= PSI_ANIMATION_TICKS
                && *transfer_timer <= PSI_TRANSFER_TIMEOUT + PSI_ANIMATION_TICKS;
            let v = animation.get(ctx.alpha);
            (conn, v * v * *distance / 2.0)
        }
        _ => (false, 0.0),
    };
    let (middle_model, top_model) = if is_energy {
        (
            if connected {
                PEI_MIDDLE_POWERED
            } else {
                PEI_MIDDLE
            },
            PEI_TOP,
        )
    } else if is_fluid {
        (
            if connected {
                PFI_MIDDLE_POWERED
            } else {
                PFI_MIDDLE
            },
            PFI_TOP,
        )
    } else {
        (
            if connected {
                PSI_MIDDLE_POWERED
            } else {
                PSI_MIDDLE
            },
            PSI_TOP,
        )
    };
    let x_deg: f32 = match ctx.facing {
        "up" => 0.0,
        "down" => 180.0,
        _ => 90.0,
    };
    let rot = centred(
        Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
            * Quat::from_rotation_x(x_deg.to_radians()),
    );
    vec![
        part(
            middle_model,
            rot * Mat4::from_translation(Vec3::new(0.0, ext * 0.5 + 0.375, 0.0)),
        ),
        part(
            top_model,
            rot * Mat4::from_translation(Vec3::new(0.0, ext, 0.0)),
        ),
    ]
}

/// Emits partial model transforms and any world item draws for `ctx.block`.
pub(super) fn aux_parts_and_item(ctx: &AuxBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    match ctx.block {
        "create:fluid_valve" => (fluid_valve_parts(ctx), Vec::new()),
        "create:analog_lever" => (analog_lever_parts(ctx), Vec::new()),
        "create:andesite_funnel"
        | "create:brass_funnel"
        | "create:andesite_belt_funnel"
        | "create:brass_belt_funnel" => (funnel_parts(ctx), Vec::new()),
        "create:weighted_ejector" => weighted_ejector_parts_and_items(ctx),
        "create:sticker" => (sticker_parts(ctx), Vec::new()),
        "create:packager" | "create:repackager" => packager_parts_and_items(ctx),
        "create:portable_storage_interface"
        | "create:portable_fluid_interface"
        | "createaddition:portable_energy_interface" => (portable_interface_parts(ctx), Vec::new()),
        other => match toolbox_lid_model(other) {
            Some(lid) => (toolbox_parts(ctx, lid), Vec::new()),
            None => (Vec::new(), Vec::new()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::Compound;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn base_ctx<'a>(block: &'a str, facing: &'a str) -> AuxBlock<'a> {
        AuxBlock {
            block,
            facing,
            face: "floor",
            shape: "",
            along_first: false,
            enabled: false,
            extended: false,
            pos: [1, 0, 0],
            speed: 0.0,
            time: 0.0,
            alpha: 0.0,
            aux: None,
        }
    }

    fn lerped_tag(value: f32, target: f32, speed: f32) -> Tag {
        let mut c = Compound::new();
        c.insert(JavaString::from("Value"), Tag::Float(value.to_bits()));
        c.insert(JavaString::from("Target"), Tag::Float(target.to_bits()));
        c.insert(JavaString::from("Speed"), Tag::Float(speed.to_bits()));
        Tag::Compound(c)
    }

    #[test]
    fn fluid_valve_pointer_turns_from_closed_to_open_and_ticks() {
        let mut valve_nbt = Compound::new();
        valve_nbt.insert(JavaString::from("Pointer"), lerped_tag(0.0, 1.0, 0.25));
        let mut aux = aux_state_from_nbt(None, &Tag::Compound(valve_nbt)).unwrap();
        let closed_parts = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:fluid_valve", "up")
        })
        .0;
        assert_eq!(closed_parts.len(), 2);
        assert_eq!(closed_parts[0].model, SHAFT);
        assert_eq!(closed_parts[1].model, FLUID_VALVE_POINTER);

        for _ in 0..4 {
            tick_aux_state(&mut aux, false);
        }
        let open_parts = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:fluid_valve", "up")
        })
        .0;
        let probe = Vec3::new(0.8, 0.5, 0.5);
        assert!(!close(
            closed_parts[1].local.transform_point3(probe),
            open_parts[1].local.transform_point3(probe)
        ));
    }

    #[test]
    fn analog_lever_handle_and_red_tint_track_state_0_to_15() {
        let mut tag0 = Compound::new();
        tag0.insert(JavaString::from("State"), Tag::Int(0));
        tag0.insert(JavaString::from("ChangeTimer"), Tag::Int(0));
        let s0 = aux_state_from_nbt(None, &Tag::Compound(tag0)).unwrap();
        let p0 = aux_parts_and_item(&AuxBlock {
            aux: Some(&s0),
            ..base_ctx("create:analog_lever", "north")
        })
        .0;
        assert_eq!(p0[0].model, ANALOG_LEVER_HANDLE);
        assert_eq!(p0[1].model, ANALOG_LEVER_INDICATOR);
        assert_eq!(p0[1].tint, Some(0x2C0300));

        let mut tag15 = Compound::new();
        tag15.insert(JavaString::from("State"), Tag::Int(15));
        tag15.insert(JavaString::from("ChangeTimer"), Tag::Int(15));
        let mut s15 = aux_state_from_nbt(Some(&s0), &Tag::Compound(tag15)).unwrap();
        for _ in 0..40 {
            tick_aux_state(&mut s15, false);
        }
        let p15 = aux_parts_and_item(&AuxBlock {
            aux: Some(&s15),
            ..base_ctx("create:analog_lever", "north")
        })
        .0;
        assert_eq!(p15[1].tint, Some(0xCD0000));
        // Lever handle pitched by 90 degrees about X at pivot (0.5, 1/16, 0.5).
        let tip = Vec3::new(0.5, 0.5, 0.5);
        assert!(!close(
            p0[0].local.transform_point3(tip),
            p15[0].local.transform_point3(tip)
        ));
    }

    #[test]
    fn funnels_emit_four_flaps_on_horizontal_sides_with_shape_offsets() {
        assert!(
            aux_parts_and_item(&base_ctx("create:andesite_funnel", "down"))
                .0
                .is_empty()
        );

        let normal = aux_parts_and_item(&base_ctx("create:brass_funnel", "north")).0;
        assert_eq!(normal.len(), 4);
        for p in &normal {
            assert_eq!(p.model, FUNNEL_FLAP);
        }

        let ext = aux_parts_and_item(&AuxBlock {
            shape: "extended",
            ..base_ctx("create:andesite_belt_funnel", "south")
        })
        .0;
        let pull = aux_parts_and_item(&AuxBlock {
            shape: "pulling",
            ..base_ctx("create:andesite_belt_funnel", "south")
        })
        .0;
        assert_eq!(ext.len(), 4);
        assert_eq!(ext[0].model, BELT_FUNNEL_FLAP);
        assert!(
            (ext[0].local.transform_point3(Vec3::ZERO).z
                - pull[0].local.transform_point3(Vec3::ZERO).z)
                .abs()
                > 0.5
        );
    }

    #[test]
    fn funnel_flap_update_animates_the_rendered_flap() {
        let idle = funnel_parts(&base_ctx("create:brass_funnel", "north"));
        let mut flap = logistics::new_flap();
        logistics::kick_flap(&mut flap, true);
        let mut aux = KineticAuxState::Funnel { flap };
        tick_aux_state(&mut aux, false);
        let moving = funnel_parts(&AuxBlock {
            aux: Some(&aux),
            alpha: 1.0,
            ..base_ctx("create:brass_funnel", "north")
        });
        assert!(!close(
            idle[0].local.transform_point3(Vec3::ZERO),
            moving[0].local.transform_point3(Vec3::ZERO),
        ));
    }

    #[test]
    fn weighted_ejector_hinges_top_plate_by_lid_progress() {
        let mut nbt = Compound::new();
        nbt.insert(JavaString::from("Lid"), lerped_tag(0.0, 1.0, 0.5));
        let mut aux = aux_state_from_nbt(None, &Tag::Compound(nbt)).unwrap();
        let rest = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:weighted_ejector", "south")
        })
        .0;
        tick_aux_state(&mut aux, false);
        tick_aux_state(&mut aux, false);
        let launched = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:weighted_ejector", "south")
        })
        .0;
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0].model, SHAFT);
        assert_eq!(launched[1].model, EJECTOR_TOP);
        // Pivot (0, 11.25/16, 0.75/16) in model space stays invariant; front lifts.
        let pivot = Vec3::new(0.5, 11.25 / 16.0, 0.75 / 16.0);
        assert!(close(
            rest[1].local.transform_point3(pivot),
            launched[1].local.transform_point3(pivot)
        ));
        assert!(!close(
            rest[1].local.transform_point3(Vec3::new(0.5, 0.8, 0.2)),
            launched[1].local.transform_point3(Vec3::new(0.5, 0.8, 0.2))
        ));
    }

    #[test]
    fn sticker_piston_chases_extended_state_quadratically() {
        let mut aux = KineticAuxState::Sticker {
            piston: LerpedValue::start_with(0.0),
        };
        tick_aux_state(&mut aux, true); // prev=0.0, value=0.4
        let half = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            extended: true,
            alpha: 0.5,
            ..base_ctx("create:sticker", "up")
        })
        .0;
        let mid = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            extended: true,
            alpha: 1.0,
            ..base_ctx("create:sticker", "up")
        })
        .0;
        for _ in 0..3 {
            tick_aux_state(&mut aux, true); // 1.0
        }
        let full = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            extended: true,
            alpha: 1.0,
            ..base_ctx("create:sticker", "up")
        })
        .0;
        let rest = aux_parts_and_item(&base_ctx("create:sticker", "up")).0;
        assert_eq!(full[0].model, STICKER_HEAD);
        let dy_half = half[0].local.transform_point3(Vec3::ZERO).y
            - rest[0].local.transform_point3(Vec3::ZERO).y;
        let dy_mid = mid[0].local.transform_point3(Vec3::ZERO).y
            - rest[0].local.transform_point3(Vec3::ZERO).y;
        let dy_full = full[0].local.transform_point3(Vec3::ZERO).y
            - rest[0].local.transform_point3(Vec3::ZERO).y;
        assert!((dy_half - (0.2f32 * 0.2 * 4.0 / 16.0)).abs() < 1e-4);
        assert!((dy_mid - (0.4f32 * 0.4 * 4.0 / 16.0)).abs() < 1e-4);
        assert!((dy_full - 4.0 / 16.0).abs() < 1e-4);
    }

    #[test]
    fn toolboxes_support_all_16_colors_and_animate_lid_and_drawers() {
        let colors = [
            "black",
            "blue",
            "brown",
            "cyan",
            "gray",
            "green",
            "light_blue",
            "light_gray",
            "lime",
            "magenta",
            "orange",
            "pink",
            "purple",
            "red",
            "white",
            "yellow",
        ];
        for color in colors {
            let block = format!("create:{color}_toolbox");
            assert!(is_aux_block(&block), "{block}");
            assert!(toolbox_lid_model(&block).unwrap().ends_with(color));
        }

        let mut closed_tag = Compound::new();
        closed_tag.insert(JavaString::from("OpenCount"), Tag::Int(0));
        let closed_aux = aux_state_from_nbt(None, &Tag::Compound(closed_tag)).unwrap();
        let closed_parts = aux_parts_and_item(&AuxBlock {
            aux: Some(&closed_aux),
            ..base_ctx("create:brown_toolbox", "north")
        })
        .0;

        let mut open_tag = Compound::new();
        open_tag.insert(JavaString::from("OpenCount"), Tag::Int(1));
        let mut open_aux = aux_state_from_nbt(Some(&closed_aux), &Tag::Compound(open_tag)).unwrap();
        for _ in 0..10 {
            tick_aux_state(&mut open_aux, false);
        }
        let open_parts = aux_parts_and_item(&AuxBlock {
            aux: Some(&open_aux),
            ..base_ctx("create:brown_toolbox", "north")
        })
        .0;
        assert_eq!(open_parts.len(), 3);
        assert_eq!(open_parts[0].model, "create:block/toolbox/lid/brown");
        assert_eq!(open_parts[1].model, TOOLBOX_DRAWER);
        assert_eq!(open_parts[2].model, TOOLBOX_DRAWER);
        assert!(!close(
            closed_parts[0]
                .local
                .transform_point3(Vec3::new(0.5, 0.5, 0.5)),
            open_parts[0]
                .local
                .transform_point3(Vec3::new(0.5, 0.5, 0.5))
        ));
        assert!(!close(
            closed_parts[1].local.transform_point3(Vec3::ZERO),
            open_parts[1].local.transform_point3(Vec3::ZERO)
        ));
    }

    #[test]
    fn packager_and_repackager_animate_hatch_tray_and_box_cycle() {
        let mut held = Compound::new();
        held.insert(
            JavaString::from("id"),
            Tag::String(JavaString::from("create:cardboard_package_12x12")),
        );
        held.insert(JavaString::from("count"), Tag::Int(1));
        let mut tag = Compound::new();
        tag.insert(JavaString::from("AnimationTicks"), Tag::Int(16));
        tag.insert(JavaString::from("AnimationInward"), Tag::Byte(0));
        tag.insert(JavaString::from("HeldBox"), Tag::Compound(held));

        let mut aux = aux_state_from_nbt(None, &Tag::Compound(tag)).unwrap();
        let (early_parts, early_items) = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:packager", "north")
        });
        assert_eq!(early_parts[0].model, PACKAGER_HATCH_OPEN);
        assert_eq!(early_parts[1].model, PACKAGER_TRAY);
        assert!(early_items.is_empty());

        for _ in 0..8 {
            tick_aux_state(&mut aux, false);
        }
        let (mid_parts, mid_items) = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:repackager", "north")
        });
        assert_eq!(mid_parts[1].model, REPACKAGER_TRAY);
        let draw = mid_items.first().expect("held box visible when ticks < 10");
        assert_eq!(draw.stack.item.as_ref(), "create:cardboard_package_12x12");
    }

    #[test]
    fn portable_interfaces_extend_and_light_middle_while_connected() {
        let mut tag = Compound::new();
        tag.insert(JavaString::from("Timer"), Tag::Int(18));
        tag.insert(JavaString::from("Distance"), Tag::Float(1.5f32.to_bits()));
        tag.insert(JavaString::from("Powered"), Tag::Byte(0));
        let mut aux = aux_state_from_nbt(None, &Tag::Compound(tag)).unwrap();
        let connected = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:portable_storage_interface", "north")
        })
        .0;
        assert_eq!(connected[0].model, PSI_MIDDLE_POWERED);
        assert_eq!(connected[1].model, PSI_TOP);
        // Extended by 1.5 / 2 = 0.75 blocks along north (-Z).
        assert!(
            (connected[1]
                .local
                .transform_point3(Vec3::new(0.5, 0.5, 0.5))
                .z
                - (-0.25))
                .abs()
                < 1e-4
        );

        for _ in 0..20 {
            tick_aux_state(&mut aux, false);
        }
        let idle = aux_parts_and_item(&AuxBlock {
            aux: Some(&aux),
            ..base_ctx("create:portable_fluid_interface", "up")
        })
        .0;
        assert_eq!(idle[0].model, PFI_MIDDLE);
        assert_eq!(idle[1].model, PFI_TOP);
    }
}
