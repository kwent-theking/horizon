//! Create belts, tunnels, blaze burners, depots, chutes, basins and package frogports.
//!
//! Ported from `Creators-of-Create/Create` (`BeltRenderer`, `BeltTunnelRenderer`,
//! `BlazeBurnerRenderer`, `DepotRenderer`, `ChuteRenderer`, `BasinRenderer`,
//! `FrogportRenderer`, `FlapStuffs`, MIT) and `Creators-of-Create/Ponder` (MIT).

use super::arm::read_block_pos;
use super::levers::to_y_rot;
use super::machines::{
    MachinePart, SpriteShift, centred, clockwise_axis, horizontal_angle, part, spin,
};
use super::{angle_radians, axis_y_rotation, opposite_facing, rotation_offset};
use glam::{Mat4, Quat, Vec3};
use horizon_assets::item_model::DisplayContext;
use horizon_client::world::{DepotItems, KineticLogisticsState, LerpedValue, TransportedItem};
use horizon_items::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::f32::consts::{FRAC_PI_2, PI};

const BELT_START: &str = "create:block/belt/start";
const BELT_MIDDLE: &str = "create:block/belt/middle";
const BELT_END: &str = "create:block/belt/end";
const BELT_START_BOTTOM: &str = "create:block/belt/start_bottom";
const BELT_MIDDLE_BOTTOM: &str = "create:block/belt/middle_bottom";
const BELT_END_BOTTOM: &str = "create:block/belt/end_bottom";
const BELT_DIAGONAL_START: &str = "create:block/belt/diagonal_start";
const BELT_DIAGONAL_MIDDLE: &str = "create:block/belt/diagonal_middle";
const BELT_DIAGONAL_END: &str = "create:block/belt/diagonal_end";
const BELT_PULLEY: &str = "create:block/belt_pulley";

const BELT_TUNNEL_FLAP: &str = "create:block/belt_tunnel/flap";

const BLAZE_INERT: &str = "create:block/blaze_burner/blaze/inert";
const BLAZE_IDLE: &str = "create:block/blaze_burner/blaze/idle";
const BLAZE_ACTIVE: &str = "create:block/blaze_burner/blaze/active";
const BLAZE_SUPER: &str = "create:block/blaze_burner/blaze/super";
const BLAZE_SUPER_ACTIVE: &str = "create:block/blaze_burner/blaze/super_active";
const BLAZE_GOGGLES: &str = "create:block/blaze_burner/goggles";
const BLAZE_GOGGLES_SMALL: &str = "create:block/blaze_burner/goggles_small";
const BLAZE_RODS_SMALL: &str = "create:block/blaze_burner/rods_small";
const BLAZE_RODS_LARGE: &str = "create:block/blaze_burner/rods_large";
const BLAZE_SUPER_RODS_SMALL: &str = "create:block/blaze_burner/superheated_rods_small";
const BLAZE_SUPER_RODS_LARGE: &str = "create:block/blaze_burner/superheated_rods_large";
const BLAZE_FLAME: &str = "create:block/blaze_burner/flame";
const TRAIN_HAT: &str = "create:entity/train_hat";
const LOGISTICS_HAT: &str = "create:entity/logistics_hat";

const FROGPORT_BODY: &str = "create:block/package_frogport/body";
const FROGPORT_HEAD: &str = "create:block/package_frogport/head";
const FROGPORT_HEAD_GOGGLES: &str = "create:block/package_frogport/head_goggles";
const FROGPORT_TONGUE: &str = "create:block/package_frogport/tongue";

const DYE_SCROLL: [&str; 16] = [
    "create:block/belt/white_scroll",
    "create:block/belt/orange_scroll",
    "create:block/belt/magenta_scroll",
    "create:block/belt/light_blue_scroll",
    "create:block/belt/yellow_scroll",
    "create:block/belt/lime_scroll",
    "create:block/belt/pink_scroll",
    "create:block/belt/gray_scroll",
    "create:block/belt/light_gray_scroll",
    "create:block/belt/cyan_scroll",
    "create:block/belt/purple_scroll",
    "create:block/belt/blue_scroll",
    "create:block/belt/brown_scroll",
    "create:block/belt/green_scroll",
    "create:block/belt/red_scroll",
    "create:block/belt/black_scroll",
];

const DYE_DIAGONAL_SCROLL: [&str; 16] = [
    "create:block/belt/white_diagonal_scroll",
    "create:block/belt/orange_diagonal_scroll",
    "create:block/belt/magenta_diagonal_scroll",
    "create:block/belt/light_blue_diagonal_scroll",
    "create:block/belt/yellow_diagonal_scroll",
    "create:block/belt/lime_diagonal_scroll",
    "create:block/belt/pink_diagonal_scroll",
    "create:block/belt/gray_diagonal_scroll",
    "create:block/belt/light_gray_diagonal_scroll",
    "create:block/belt/cyan_diagonal_scroll",
    "create:block/belt/purple_diagonal_scroll",
    "create:block/belt/blue_diagonal_scroll",
    "create:block/belt/brown_diagonal_scroll",
    "create:block/belt/green_diagonal_scroll",
    "create:block/belt/red_diagonal_scroll",
    "create:block/belt/black_diagonal_scroll",
];

const OUTPUT_ANIMATION_TIME: i32 = 10;

/// Inputs for rendering one Create belt, tunnel, burner or item-carrier block.
pub(super) struct LogisticsBlock<'a> {
    pub block: &'a str,
    pub facing: &'a str,
    pub slope: &'a str,
    pub part: &'a str,
    pub shape: &'a str,
    pub blaze: &'a str,
    pub pos: [i32; 3],
    pub speed: f32,
    pub time: f64,
    pub alpha: f64,
    pub logistics: Option<&'a KineticLogisticsState>,
}

/// How an emitted item draw finalizes its local matrix once `is_block_item` is known.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ItemPoseStyle {
    /// Already positioned, oriented and scaled in block-local space.
    Raw,
    /// Positioned at the depot/belt carrier surface (`DepotRenderer.renderItem`).
    DepotOrBelt {
        is_package: bool,
        angle_deg: i32,
        copies: usize,
    },
}

/// One item stack submitted from a Create kinetic or logistics block entity.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WorldItemDraw {
    pub stack: ItemStack,
    pub local: Mat4,
    pub light_offset: [i32; 3],
    pub display: DisplayContext,
    pub style: ItemPoseStyle,
}

/// Per-tick world neighbors needed to tick logistics states.
pub(super) struct LogisticsTickCtx {
    pub player_xz: Option<[f64; 2]>,
    pub facing: String,
    pub blaze: String,
    pub valid_block_above: bool,
    pub mixer_running_above: bool,
    pub belt_speed: f32,
}

/// Whether `block` is drawn or item-rendered by this module.
pub(super) fn is_logistics_block(block: &str) -> bool {
    matches!(
        block,
        "create:belt"
            | "create:andesite_tunnel"
            | "create:brass_tunnel"
            | "create:blaze_burner"
            | "createaddition:liquid_blaze_burner"
            | "create:depot"
            | "create:chute"
            | "create:smart_chute"
            | "create:basin"
            | "create:package_frogport"
    )
}

/// Whether a block entity packet belongs to one of these Create block entity types.
pub(super) fn is_logistics_be_tag(name: &str, tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    if fields.is_empty() {
        return false;
    }
    matches!(
        name,
        "create:belt"
            | "create:andesite_tunnel"
            | "create:brass_tunnel"
            | "create:blaze_heater"
            | "create:blaze_burner"
            | "createaddition:liquid_blaze_burner"
            | "create:depot"
            | "create:chute"
            | "create:smart_chute"
            | "create:basin"
            | "create:package_frogport"
    )
}

pub(super) fn is_package_item(stack: &ItemStack) -> bool {
    let id = stack.item.as_ref();
    id.starts_with("create:") && (id.contains("_package_") || id.ends_with("_package"))
}

fn count_log2_copies(count: u32) -> usize {
    if count <= 1 {
        1
    } else {
        (count.ilog2() as usize) / 2 + 1
    }
}

/// Expands `draw` into concrete block-local model matrices matching Create's `renderItem`.
pub(super) fn item_draw_matrices(draw: &WorldItemDraw, is_block_item: bool) -> Vec<Mat4> {
    match draw.style {
        ItemPoseStyle::Raw => vec![draw.local],
        ItemPoseStyle::DepotOrBelt {
            is_package,
            angle_deg,
            copies,
        } => {
            let mut cursor = draw.local * Mat4::from_rotation_y((angle_deg as f32).to_radians());
            let mut out = Vec::with_capacity(copies.max(1));
            for i in 0..copies.max(1) {
                let mut m = cursor;
                if is_package {
                    m *= Mat4::from_translation(Vec3::new(0.0, 4.0 / 16.0, 0.0))
                        * Mat4::from_scale(Vec3::splat(1.5));
                } else {
                    if is_block_item && i > 0 {
                        let seed = ((angle_deg as u32).wrapping_mul(1103515245)
                            ^ (i as u32 * 2654435761)) as f32;
                        let jx = (seed % 16.0) / 16.0 * 0.0625 * i as f32;
                        let jz = ((seed / 17.0) % 16.0) / 16.0 * 0.0625 * i as f32;
                        m *= Mat4::from_translation(Vec3::new(jx, 0.0, jz));
                    }
                    m *= Mat4::from_scale(Vec3::splat(0.5));
                    if !is_block_item {
                        m *= Mat4::from_translation(Vec3::new(0.0, -3.0 / 16.0, 0.0))
                            * Mat4::from_rotation_x(FRAC_PI_2);
                    }
                }
                out.push(m);
                if !is_block_item {
                    cursor *= Mat4::from_rotation_y(10.0f32.to_radians());
                }
                cursor *= Mat4::from_translation(Vec3::new(
                    0.0,
                    if is_block_item {
                        1.0 / 64.0
                    } else {
                        1.0 / 16.0
                    },
                    0.0,
                ));
            }
            out
        }
    }
}

fn read_f32(fields: &Compound, key: &str) -> Option<f32> {
    match fields.get(&JavaString::from(key))? {
        Tag::Float(bits) => Some(f32::from_bits(*bits)),
        Tag::Double(bits) => Some(f64::from_bits(*bits) as f32),
        _ => None,
    }
}

fn read_i32(fields: &Compound, key: &str) -> Option<i32> {
    match fields.get(&JavaString::from(key))? {
        Tag::Int(v) => Some(*v),
        Tag::Byte(v) => Some(i32::from(*v)),
        Tag::Short(v) => Some(i32::from(*v)),
        _ => None,
    }
}

fn read_bool(fields: &Compound, key: &str) -> bool {
    matches!(fields.get(&JavaString::from(key)), Some(Tag::Byte(v)) if *v != 0)
}

fn dye_index(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "white" => Some(0),
        "orange" => Some(1),
        "magenta" => Some(2),
        "light_blue" => Some(3),
        "yellow" => Some(4),
        "lime" => Some(5),
        "pink" => Some(6),
        "gray" => Some(7),
        "light_gray" => Some(8),
        "cyan" => Some(9),
        "purple" => Some(10),
        "blue" => Some(11),
        "brown" => Some(12),
        "green" => Some(13),
        "red" => Some(14),
        "black" => Some(15),
        _ => None,
    }
}

pub(super) fn parse_transported_item(tag: &Tag) -> Option<TransportedItem> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let stack = fields
        .get(&JavaString::from("Item"))
        .and_then(horizon_items::item_nbt::item_from_nbt)?;
    let belt_pos = read_f32(fields, "Pos").unwrap_or(0.0);
    let side_offset = read_f32(fields, "Offset").unwrap_or(0.0);
    Some(TransportedItem {
        stack,
        belt_pos,
        prev_belt_pos: read_f32(fields, "PrevPos").unwrap_or(belt_pos),
        side_offset,
        prev_side_offset: read_f32(fields, "PrevOffset").unwrap_or(side_offset),
        angle: read_i32(fields, "Angle").unwrap_or(0),
        inserted_from: read_i32(fields, "InDirection").unwrap_or(1).clamp(0, 5) as u8,
        locked: read_bool(fields, "Locked"),
        locked_externally: read_bool(fields, "LockedExternally"),
    })
}

fn parse_item_handler(tag: Option<&Tag>) -> Vec<(usize, ItemStack)> {
    let Some(Tag::Compound(fields)) = tag else {
        return Vec::new();
    };
    let Some(Tag::List(items)) = fields.get(&JavaString::from("Items")) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|entry| {
            let Tag::Compound(c) = entry else {
                return None;
            };
            let slot = read_i32(c, "Slot").unwrap_or(0).max(0) as usize;
            let stack = horizon_items::item_nbt::item_from_nbt(entry)?;
            Some((slot, stack))
        })
        .collect()
}

/// Parses `DepotBehaviour.write` fields (`HeldItem`, `Incoming`, `OutputBuffer`).
pub(super) fn parse_depot_items(fields: &Compound) -> DepotItems {
    let held_item = fields
        .get(&JavaString::from("HeldItem"))
        .and_then(parse_transported_item);
    let incoming = match fields.get(&JavaString::from("Incoming")) {
        Some(Tag::List(list)) => list.iter().filter_map(parse_transported_item).collect(),
        _ => Vec::new(),
    };
    let output_buffer = parse_item_handler(fields.get(&JavaString::from("OutputBuffer")));
    DepotItems {
        held_item,
        incoming,
        output_buffer,
    }
}

fn dir_3d_name(idx: u8) -> &'static str {
    match idx {
        0 => "down",
        1 => "up",
        2 => "north",
        3 => "south",
        4 => "west",
        5 => "east",
        _ => "north",
    }
}

fn dir_3d_normal(idx: u8) -> Vec3 {
    match idx {
        0 => Vec3::NEG_Y,
        1 => Vec3::Y,
        2 => Vec3::NEG_Z,
        3 => Vec3::Z,
        4 => Vec3::NEG_X,
        5 => Vec3::X,
        _ => Vec3::ZERO,
    }
}

fn wrap_degrees_diff(from: f32, to: f32) -> f32 {
    let mut d = (to - from) % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

fn pos_hash(pos: [i32; 3]) -> i32 {
    let h = pos[0].wrapping_mul(73856093)
        ^ pos[1].wrapping_mul(19349663)
        ^ pos[2].wrapping_mul(83492791);
    if h == i32::MIN { 0 } else { h.abs() }
}

/// Parses a Create logistics/item-carrier block entity tag into `KineticLogisticsState`.
pub(super) fn logistics_state_from_nbt(
    existing: Option<&KineticLogisticsState>,
    tag: &Tag,
) -> Option<KineticLogisticsState> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let has = |k: &str| fields.contains_key(&JavaString::from(k));

    if has("PlacedYaw") || has("AnimatedPackage") || has("AddressFilter") {
        let passive_yaw = read_f32(fields, "PlacedYaw").unwrap_or(0.0);
        let goggles = has("Goggles");
        let open_count = read_i32(fields, "OpenCount").unwrap_or(0);
        let target_rel = match fields.get(&JavaString::from("Target")) {
            Some(Tag::Compound(tc)) => tc
                .get(&JavaString::from("relative_pos"))
                .and_then(read_block_pos),
            _ => None,
        };
        let (prev_prog, prev_ant, prev_manual, prev_pkg, prev_dep) = match existing {
            Some(KineticLogisticsState::Frogport {
                animation_progress,
                anticipation_progress,
                manual_open,
                animated_package,
                depositing,
                ..
            }) => (
                *animation_progress,
                *anticipation_progress,
                *manual_open,
                animated_package.clone(),
                *depositing,
            ),
            _ => (
                LerpedValue::start_with(0.0),
                LerpedValue::start_with(0.0),
                LerpedValue::start_with(0.0),
                None,
                false,
            ),
        };
        let (animation_progress, animated_package, depositing) = if let Some(pkg) = fields
            .get(&JavaString::from("AnimatedPackage"))
            .and_then(horizon_items::item_nbt::item_from_nbt)
        {
            let mut prog = LerpedValue::start_with(0.0);
            prog.chase_linear(1.0, 0.1);
            (prog, Some(pkg), read_bool(fields, "Deposit"))
        } else {
            (prev_prog, prev_pkg, prev_dep)
        };
        let mut anticipation_progress = prev_ant;
        if has("Anticipate") {
            anticipation_progress.chase_linear(1.0, 0.1);
        }
        return Some(KineticLogisticsState::Frogport {
            passive_yaw,
            goggles,
            target_rel,
            animated_package,
            depositing,
            open_count,
            animation_progress,
            anticipation_progress,
            manual_open: prev_manual,
        });
    }

    if has("IsController") || (has("Length") && has("Index")) {
        let is_controller = read_bool(fields, "IsController");
        let belt_length = read_i32(fields, "Length").unwrap_or(0);
        let dye = match fields.get(&JavaString::from("Dye")) {
            Some(Tag::String(s)) => dye_index(&s.to_string_lossy()),
            _ => None,
        };
        let mut items = Vec::new();
        let mut lazy_item = None;
        if is_controller
            && let Some(Tag::Compound(inv)) = fields.get(&JavaString::from("Inventory"))
        {
            if let Some(Tag::List(list)) = inv.get(&JavaString::from("Items")) {
                items = list.iter().filter_map(parse_transported_item).collect();
            }
            lazy_item = inv
                .get(&JavaString::from("LazyItem"))
                .and_then(parse_transported_item);
        }
        return Some(KineticLogisticsState::Belt {
            is_controller,
            belt_length,
            dye,
            items,
            lazy_item,
        });
    }

    if let Some(Tag::List(flap_list)) = fields.get(&JavaString::from("Flaps")) {
        let existing_flaps = match existing {
            Some(KineticLogisticsState::BeltTunnel { flaps }) => flaps.as_slice(),
            _ => &[],
        };
        let mut flaps = Vec::new();
        for entry in flap_list {
            let Tag::Int(dir) = entry else {
                continue;
            };
            if !(0..=5).contains(dir) {
                continue;
            }
            let dir_u8 = *dir as u8;
            let lv = existing_flaps
                .iter()
                .find(|(d, _)| *d == dir_u8)
                .map(|(_, v)| *v)
                .unwrap_or_else(new_flap);
            flaps.push((dir_u8, lv));
        }
        return Some(KineticLogisticsState::BeltTunnel { flaps });
    }

    if has("fuelLevel") || has("isCreative") || has("burnTimeRemaining") {
        let (head_angle, head_animation, stock_keeper) = match existing {
            Some(KineticLogisticsState::BlazeBurner {
                head_angle,
                head_animation,
                stock_keeper,
                ..
            }) => (*head_angle, *head_animation, *stock_keeper),
            _ => (
                LerpedValue::start_with(0.0),
                LerpedValue::start_with(0.0),
                false,
            ),
        };
        return Some(KineticLogisticsState::BlazeBurner {
            goggles: has("Goggles"),
            hat: has("TrainHat"),
            stock_keeper,
            head_angle,
            head_animation,
        });
    }

    if has("OutputBuffer") && !has("Lid") {
        return Some(KineticLogisticsState::Depot {
            items: parse_depot_items(fields),
        });
    }

    if has("ItemPosition") {
        let item = fields
            .get(&JavaString::from("Item"))
            .and_then(horizon_items::item_nbt::item_from_nbt);
        let item_pos = read_f32(fields, "ItemPosition").unwrap_or(0.0);
        return Some(KineticLogisticsState::Chute {
            item,
            item_pos: LerpedValue::start_with(item_pos),
            pull: read_f32(fields, "Pull").unwrap_or(0.0),
            push: read_f32(fields, "Push").unwrap_or(0.0),
            bottom_pull_distance: read_f32(fields, "BottomAirFlowDistance").unwrap_or(0.0),
        });
    }

    if has("InputItems") || has("OutputItems") {
        let (mut visualized_outputs, ingredient_rotation, ingredient_rotation_speed) =
            match existing {
                Some(KineticLogisticsState::Basin {
                    visualized_outputs,
                    ingredient_rotation,
                    ingredient_rotation_speed,
                    ..
                }) => (
                    visualized_outputs.clone(),
                    *ingredient_rotation,
                    *ingredient_rotation_speed,
                ),
                _ => (
                    Vec::new(),
                    LerpedValue::start_with(0.0),
                    LerpedValue::start_with(0.0),
                ),
            };
        if let Some(Tag::List(vis)) = fields.get(&JavaString::from("VisualizedItems")) {
            for entry in vis {
                if let Some(stack) = horizon_items::item_nbt::item_from_nbt(entry) {
                    visualized_outputs.push((OUTPUT_ANIMATION_TIME, stack));
                }
            }
        }
        return Some(KineticLogisticsState::Basin {
            input_items: parse_item_handler(fields.get(&JavaString::from("InputItems")))
                .into_iter()
                .map(|(_, s)| s)
                .collect(),
            output_items: parse_item_handler(fields.get(&JavaString::from("OutputItems")))
                .into_iter()
                .map(|(_, s)| s)
                .collect(),
            visualized_outputs,
            ingredient_rotation,
            ingredient_rotation_speed,
        });
    }

    None
}

/// Advances one logistics block entity animation state by one client tick.
pub(super) fn tick_logistics_state(
    state: &mut KineticLogisticsState,
    pos: [i32; 3],
    ctx: &LogisticsTickCtx,
) {
    match state {
        KineticLogisticsState::Belt {
            is_controller,
            belt_length,
            items,
            ..
        } => {
            if *is_controller && *belt_length > 0 && ctx.belt_speed != 0.0 {
                let direction = if matches!(ctx.facing.as_str(), "north" | "east" | "down") {
                    -1.0
                } else {
                    1.0
                };
                let delta = direction * ctx.belt_speed / 480.0;
                let max_pos = (*belt_length as f32 - 0.1).max(0.0);
                for it in items.iter_mut() {
                    it.prev_belt_pos = it.belt_pos;
                    it.prev_side_offset = it.side_offset;
                    if !it.locked && !it.locked_externally {
                        it.belt_pos = (it.belt_pos + delta).clamp(0.1, max_pos);
                    }
                }
            }
        }
        KineticLogisticsState::BeltTunnel { flaps } => {
            for (_, lv) in flaps.iter_mut() {
                lv.tick();
            }
        }
        KineticLogisticsState::BlazeBurner {
            head_angle,
            head_animation,
            ..
        } => {
            let active = matches!(ctx.blaze.as_str(), "fading" | "kindled" | "seething")
                && ctx.valid_block_above;
            if !active {
                let target_base = match ctx.player_xz {
                    Some([px, pz]) => {
                        let dx = px - (f64::from(pos[0]) + 0.5);
                        let dz = pz - (f64::from(pos[2]) + 0.5);
                        ((-dz.atan2(dx)).to_degrees() - 90.0) as f32
                    }
                    None => 0.0,
                };
                let target = head_angle.value + wrap_degrees_diff(head_angle.value, target_base);
                head_angle.chase_exp(target, 0.25);
            } else {
                let target = (horizontal_angle(&ctx.facing) + 180.0) % 360.0;
                let shortest = head_angle.value + wrap_degrees_diff(head_angle.value, target);
                head_angle.chase_exp(shortest, 0.125);
            }
            head_angle.tick();
            head_animation.chase_exp(if active { 1.0 } else { 0.0 }, 0.25);
            head_animation.tick();
        }
        KineticLogisticsState::Chute {
            item,
            item_pos,
            pull,
            push,
            bottom_pull_distance,
        } => {
            if item.is_some() {
                let motion = (*push + *pull) / 64.0;
                let step =
                    (motion.clamp(-20.0, 20.0) + if motion <= 0.0 { -4.0 } else { 0.0 }) / 20.0;
                let lower = (-bottom_pull_distance.max(0.0)).min(item_pos.value);
                let next = (item_pos.value + step).clamp(lower, 1.0);
                item_pos.set_value(next);
            }
        }
        KineticLogisticsState::Basin {
            visualized_outputs,
            ingredient_rotation,
            ingredient_rotation_speed,
            ..
        } => {
            for (t, _) in visualized_outputs.iter_mut() {
                *t -= 1;
            }
            visualized_outputs.retain(|(t, _)| *t > 0);
            ingredient_rotation_speed
                .chase_exp(if ctx.mixer_running_above { 8.0 } else { 0.0 }, 0.25);
            ingredient_rotation_speed.tick();
            let next_rot = ingredient_rotation.value + ingredient_rotation_speed.value;
            ingredient_rotation.set_value(next_rot);
        }
        KineticLogisticsState::Frogport {
            animated_package,
            open_count,
            animation_progress,
            anticipation_progress,
            manual_open,
            ..
        } => {
            if (anticipation_progress.value - 1.0).abs() < 1e-4 {
                *anticipation_progress = LerpedValue::start_with(0.0);
            }
            manual_open.chase_linear(if *open_count > 0 { 1.0 } else { 0.0 }, 0.35);
            anticipation_progress.tick();
            manual_open.tick();
            if (animation_progress.target - 1.0).abs() < 1e-4 {
                animation_progress.tick();
                if animation_progress.value >= 1.0 - 1e-4 {
                    *anticipation_progress = LerpedValue::start_with(0.0);
                    *animation_progress = LerpedValue::start_with(0.0);
                    *animated_package = None;
                }
            }
        }
        KineticLogisticsState::Depot { .. } => {}
    }
}

/// Builds item draw commands for a `DepotItems` carrier (`DepotRenderer.renderItemsOf`).
pub(super) fn depot_item_draws(
    items: &DepotItems,
    alpha: f64,
    base_local: Mat4,
) -> Vec<WorldItemDraw> {
    let mut draws = Vec::new();
    let surface = base_local * Mat4::from_translation(Vec3::new(0.5, 15.0 / 16.0, 0.5));

    let push_transported = |tis: &TransportedItem, draws: &mut Vec<WorldItemDraw>| {
        let offset = tis.prev_belt_pos + (tis.belt_pos - tis.prev_belt_pos) * alpha as f32;
        let side_raw =
            tis.prev_side_offset + (tis.side_offset - tis.prev_side_offset) * alpha as f32;
        let mut local = surface;
        let dir_idx = tis.inserted_from;
        if dir_idx >= 2 {
            let opp_norm = -dir_3d_normal(dir_idx);
            local *= Mat4::from_translation(opp_norm * (0.5 - offset));
            let cw_along_x = matches!(dir_idx, 2 | 3); // north/south clockwise -> east/west (X axis)
            let side = if cw_along_x { side_raw } else { -side_raw };
            local *= Mat4::from_translation(Vec3::new(
                if cw_along_x { side } else { 0.0 },
                0.0,
                if cw_along_x { 0.0 } else { side },
            ));
        }
        draws.push(WorldItemDraw {
            stack: tis.stack.clone(),
            local,
            light_offset: [0, 0, 0],
            display: DisplayContext::Fixed,
            style: ItemPoseStyle::DepotOrBelt {
                is_package: is_package_item(&tis.stack),
                angle_deg: tis.angle,
                copies: count_log2_copies(tis.stack.count),
            },
        });
    };

    for tis in &items.incoming {
        push_transported(tis, &mut draws);
    }
    if let Some(held) = &items.held_item {
        push_transported(held, &mut draws);
    }
    for (slot, stack) in &items.output_buffer {
        let angle_step = (360.0 / 8.0 * (*slot as f32)).to_radians();
        let local = surface
            * Mat4::from_rotation_y(angle_step)
            * Mat4::from_translation(Vec3::new(0.35, 0.0, 0.0));
        let item_angle = (((*slot as u32)
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223))
            % 360) as i32;
        draws.push(WorldItemDraw {
            stack: stack.clone(),
            local,
            light_offset: [0, 0, 0],
            display: DisplayContext::Fixed,
            style: ItemPoseStyle::DepotOrBelt {
                is_package: is_package_item(stack),
                angle_deg: item_angle,
                copies: count_log2_copies(stack.count),
            },
        });
    }
    draws
}

fn belt_parts_and_items(ctx: &LogisticsBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let slope = if ctx.slope.is_empty() {
        "horizontal"
    } else {
        ctx.slope
    };
    let belt_part = if ctx.part.is_empty() {
        "middle"
    } else {
        ctx.part
    };
    let facing = ctx.facing;
    let downward = slope == "downward";
    let upward = slope == "upward";
    let diagonal = downward || upward;
    let sideways = slope == "sideways";
    let along_x = matches!(facing, "east" | "west");
    let axis_dir = if matches!(facing, "east" | "south" | "up") {
        1.0f64
    } else {
        -1.0f64
    };
    let mut start = belt_part == "start";
    let mut end = belt_part == "end";
    if downward || (slope == "vertical" && axis_dir > 0.0) {
        std::mem::swap(&mut start, &mut end);
    }

    let local_rot = centred(
        Quat::from_rotation_y(
            (horizontal_angle(facing)
                + if upward { 180.0 } else { 0.0 }
                + if sideways { 270.0 } else { 0.0 })
            .to_radians(),
        ) * Quat::from_rotation_z(if sideways { FRAC_PI_2 } else { 0.0 })
            * Quat::from_rotation_x(if !diagonal && slope != "horizontal" {
                FRAC_PI_2
            } else {
                0.0
            }),
    );

    let (is_controller, belt_length, dye, items, lazy_item) = match ctx.logistics {
        Some(KineticLogisticsState::Belt {
            is_controller,
            belt_length,
            dye,
            items,
            lazy_item,
        }) => (
            *is_controller,
            *belt_length,
            *dye,
            items.as_slice(),
            lazy_item.as_ref(),
        ),
        _ => (false, 0, None, &[][..], None),
    };

    let mut parts = Vec::new();
    for bottom in [true, false] {
        let model = if diagonal {
            if start {
                BELT_DIAGONAL_START
            } else if end {
                BELT_DIAGONAL_END
            } else {
                BELT_DIAGONAL_MIDDLE
            }
        } else if bottom {
            if start {
                BELT_START_BOTTOM
            } else if end {
                BELT_END_BOTTOM
            } else {
                BELT_MIDDLE_BOTTOM
            }
        } else if start {
            BELT_START
        } else if end {
            BELT_END
        } else {
            BELT_MIDDLE
        };
        let mut p = part(model, local_rot);
        if ctx.speed != 0.0 || dye.is_some() {
            let mut belt_speed = ctx.speed;
            let flip = (diagonal && (downward ^ along_x))
                || (!sideways && !diagonal && along_x)
                || (sideways && axis_dir < 0.0);
            if flip {
                belt_speed = -belt_speed;
            }
            let scroll_mult = if diagonal { 3.0 / 8.0 } else { 0.5 };
            let t = ctx.time * axis_dir;
            let raw = f64::from(belt_speed) * t / (31.5 * 16.0) + if bottom { 0.5 } else { 0.0 };
            let scroll_v = ((raw - raw.floor()) * scroll_mult) as f32;
            let original = if diagonal {
                "create:block/belt_diagonal"
            } else if bottom {
                "create:block/belt_offset"
            } else {
                "create:block/belt"
            };
            let target = match dye {
                Some(idx) if (idx as usize) < 16 => {
                    if diagonal {
                        DYE_DIAGONAL_SCROLL[idx as usize]
                    } else {
                        DYE_SCROLL[idx as usize]
                    }
                }
                _ => {
                    if diagonal {
                        "create:block/belt_diagonal_scroll"
                    } else {
                        "create:block/belt_scroll"
                    }
                }
            };
            p.sprite_shift = Some(SpriteShift {
                original,
                target,
                scroll_u: 0.0,
                scroll_v,
            });
        }
        parts.push(p);
        if diagonal {
            break;
        }
    }

    if belt_part != "middle" {
        let pulley_axis = if sideways {
            "y"
        } else {
            clockwise_axis(facing)
        };
        let angle = angle_radians(
            ctx.time,
            ctx.speed,
            rotation_offset(
                match pulley_axis {
                    "x" => 0,
                    "y" => 1,
                    _ => 2,
                },
                ctx.pos,
            ),
        );
        let base = centred(axis_y_rotation(pulley_axis));
        parts.push(part(BELT_PULLEY, spin(pulley_axis, angle) * base));
    }

    let mut draws = Vec::new();
    if is_controller && belt_length > 0 && slope != "sideways" && slope != "vertical" {
        let dir_norm = match facing {
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => [0, 0, -1],
        };
        let verticality = if upward {
            1
        } else if downward {
            -1
        } else {
            0
        };
        let belt_start_offset = Vec3::new(
            0.5 - 0.5 * dir_norm[0] as f32,
            15.0 / 16.0,
            0.5 - 0.5 * dir_norm[2] as f32,
        );
        let cw_along_x = matches!(facing, "north" | "south");
        let tilt_forward = (downward ^ (axis_dir > 0.0)) == !along_x;
        let slope_deg = if tilt_forward { -45.0f32 } else { 45.0f32 };

        for tis in items.iter().chain(lazy_item.into_iter()) {
            let offset = if ctx.speed == 0.0 {
                tis.belt_pos
            } else {
                tis.prev_belt_pos + (tis.belt_pos - tis.prev_belt_pos) * ctx.alpha as f32
            };
            let side_raw = if ctx.speed == 0.0 {
                tis.side_offset
            } else {
                tis.prev_side_offset + (tis.side_offset - tis.prev_side_offset) * ctx.alpha as f32
            };
            let vertical_move = if offset < 0.5 {
                0.0
            } else {
                verticality as f32 * (offset.min(belt_length as f32 - 0.5) - 0.5)
            };
            let mut local = Mat4::from_translation(
                belt_start_offset
                    + Vec3::new(
                        dir_norm[0] as f32 * offset,
                        vertical_move,
                        dir_norm[2] as f32 * offset,
                    ),
            );
            let side = if cw_along_x { side_raw } else { -side_raw };
            local *= Mat4::from_translation(Vec3::new(
                if cw_along_x { side } else { 0.0 },
                0.0,
                if cw_along_x { 0.0 } else { side },
            ));
            let on_slope =
                slope != "horizontal" && offset >= 0.5 && offset <= belt_length as f32 - 0.5;
            if on_slope {
                let tilt_rot = if !along_x {
                    Mat4::from_rotation_x(slope_deg.to_radians())
                } else {
                    Mat4::from_rotation_z(slope_deg.to_radians())
                };
                local *= tilt_rot * Mat4::from_translation(Vec3::new(0.0, 1.0 / 8.0, 0.0));
            }
            let seg = offset.floor() as i32;
            draws.push(WorldItemDraw {
                stack: tis.stack.clone(),
                local,
                light_offset: [dir_norm[0] * seg, verticality * seg, dir_norm[2] * seg],
                display: DisplayContext::Fixed,
                style: ItemPoseStyle::DepotOrBelt {
                    is_package: is_package_item(&tis.stack),
                    angle_deg: tis.angle,
                    copies: count_log2_copies(tis.stack.count),
                },
            });
        }
    }

    (parts, draws)
}

pub(super) fn new_flap() -> LerpedValue {
    let mut flap = LerpedValue::start_with(0.25);
    flap.chase_exp(0.0, 0.05);
    flap
}

pub(super) fn kick_flap(flap: &mut LerpedValue, inward: bool) {
    flap.set_value(if inward { -1.0 } else { 1.0 });
}

pub(super) fn flap_angle(flapness: f32, segment: usize) -> f32 {
    let intensity = if segment == 3 {
        1.5
    } else {
        (segment + 1) as f32
    };
    let abs = flapness.abs();
    let mut angle = ((1.0 - abs) * PI * intensity).sin() * 30.0 * flapness;
    if flapness < 0.0 {
        angle *= 0.5;
    }
    angle
}

fn tunnel_parts(ctx: &LogisticsBlock) -> Vec<MachinePart> {
    let Some(KineticLogisticsState::BeltTunnel { flaps }) = ctx.logistics else {
        return Vec::new();
    };
    let pivot = Vec3::new(0.0, 10.0 / 16.0, 1.0 / 16.0);
    let mut parts = Vec::new();
    for &(dir_idx, ref lv) in flaps {
        let dir = dir_3d_name(dir_idx);
        if !matches!(dir, "north" | "south" | "east" | "west") {
            continue;
        }
        let flapness = lv.get(ctx.alpha);
        let base = centred(Quat::from_rotation_y(
            horizontal_angle(opposite_facing(dir)).to_radians(),
        )) * Mat4::from_translation(Vec3::new(0.075 / 16.0, 0.0, 0.0));
        for segment in 0..4usize {
            let x_step = segment as f32 * (-3.05 / 16.0);
            let local = base
                * Mat4::from_translation(Vec3::new(x_step, 0.0, 0.0))
                * Mat4::from_translation(pivot)
                * Mat4::from_rotation_x(flap_angle(flapness, segment).to_radians())
                * Mat4::from_translation(-pivot);
            parts.push(part(BELT_TUNNEL_FLAP, local));
        }
    }
    parts
}

fn blaze_ordinal(blaze: &str) -> u8 {
    match blaze {
        "smouldering" => 1,
        "fading" => 2,
        "kindled" => 3,
        "seething" => 4,
        _ => 0,
    }
}

fn blaze_burner_parts(ctx: &LogisticsBlock) -> Vec<MachinePart> {
    let ord = blaze_ordinal(ctx.blaze);
    if ord == 0 {
        return Vec::new();
    }
    let (goggles, hat, stock_keeper, anim, angle_deg) = match ctx.logistics {
        Some(KineticLogisticsState::BlazeBurner {
            goggles,
            hat,
            stock_keeper,
            head_angle,
            head_animation,
        }) => (
            *goggles,
            *hat,
            *stock_keeper,
            head_animation.get(ctx.alpha) * 0.175,
            head_angle.get(ctx.alpha),
        ),
        _ => (
            false,
            false,
            false,
            0.0,
            (horizontal_angle(ctx.facing) + 180.0) % 360.0,
        ),
    };
    let fading_plus = ord >= 2;
    let kindled_plus = ord >= 3;
    let seething = ord >= 4;
    let block_above = anim > 0.125;

    let blaze_model = if seething {
        if block_above {
            BLAZE_SUPER_ACTIVE
        } else {
            BLAZE_SUPER
        }
    } else if fading_plus {
        if block_above && kindled_plus {
            BLAZE_ACTIVE
        } else {
            BLAZE_IDLE
        }
    } else {
        BLAZE_INERT
    };

    let render_tick = ctx.time as f32 + ((pos_hash(ctx.pos) % 13) * 16) as f32;
    let offset_mult = if fading_plus { 64.0 } else { 16.0 };
    let two_pi = std::f32::consts::TAU;
    let offset = ((render_tick / 16.0) % two_pi).sin() / offset_mult;
    let offset1 = ((render_tick / 16.0 + PI) % two_pi).sin() / offset_mult;
    let offset2 = ((render_tick / 16.0 + FRAC_PI_2) % two_pi).sin() / offset_mult;
    let head_y = offset - anim * 0.75;
    let head_rot = centred(Quat::from_rotation_y(angle_deg.to_radians()));

    let full_bright_part = |model: &'static str, local: Mat4| {
        let mut p = part(model, local);
        p.full_bright = true;
        p
    };

    let mut parts = vec![full_bright_part(
        blaze_model,
        Mat4::from_translation(Vec3::new(0.0, head_y, 0.0)) * head_rot,
    )];

    if goggles {
        let goggles_model = if blaze_model == BLAZE_INERT {
            BLAZE_GOGGLES_SMALL
        } else {
            BLAZE_GOGGLES
        };
        parts.push(full_bright_part(
            goggles_model,
            Mat4::from_translation(Vec3::new(0.0, head_y + 0.5, 0.0)) * head_rot,
        ));
    }

    if hat || stock_keeper {
        let hat_model = if hat { TRAIN_HAT } else { LOGISTICS_HAT };
        let inert = blaze_model == BLAZE_INERT;
        let hat_y =
            Mat4::from_translation(Vec3::new(0.0, head_y + if inert { 0.5 } else { 0.75 }, 0.0));
        let scale = if inert {
            centred(Quat::IDENTITY)
                * Mat4::from_scale(Vec3::splat(0.75))
                * Mat4::from_translation(Vec3::splat(-0.5))
        } else {
            Mat4::IDENTITY
        };
        let local = hat_y
            * scale
            * centred(Quat::from_rotation_y((angle_deg + 180.0).to_radians()))
            * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5));
        parts.push(full_bright_part(hat_model, local));
    }

    if fading_plus {
        let (rods_small, rods_large) = if seething {
            (BLAZE_SUPER_RODS_SMALL, BLAZE_SUPER_RODS_LARGE)
        } else {
            (BLAZE_RODS_SMALL, BLAZE_RODS_LARGE)
        };
        parts.push(full_bright_part(
            rods_small,
            Mat4::from_translation(Vec3::new(0.0, offset1 + anim + 0.125, 0.0)),
        ));
        parts.push(full_bright_part(
            rods_large,
            Mat4::from_translation(Vec3::new(0.0, offset2 + anim - 3.0 / 16.0, 0.0)),
        ));
    }

    if fading_plus && block_above {
        let speed = f64::from(1.0 / 32.0 + 1.0 / 64.0 * f32::from(ord));
        let v_raw = speed * ctx.time;
        let v_scroll = ((v_raw - v_raw.floor()) * 0.5) as f32;
        let u_raw = speed * ctx.time * 0.5;
        let u_scroll = ((u_raw - u_raw.floor()) * 0.5) as f32;
        let mut flame = full_bright_part(BLAZE_FLAME, head_rot);
        flame.sprite_shift = Some(SpriteShift {
            original: "create:block/blaze_burner_flame",
            target: if seething {
                "create:block/blaze_burner_flame_superheated_scroll"
            } else {
                "create:block/blaze_burner_flame_scroll"
            },
            scroll_u: u_scroll,
            scroll_v: v_scroll,
        });
        parts.push(flame);
    }

    parts
}

fn chute_items(ctx: &LogisticsBlock) -> Vec<WorldItemDraw> {
    let Some(KineticLogisticsState::Chute {
        item: Some(stack),
        item_pos,
        bottom_pull_distance,
        ..
    }) = ctx.logistics
    else {
        return Vec::new();
    };
    let pos = item_pos.get(ctx.alpha);
    let visible = if ctx.block == "create:smart_chute" {
        pos <= 0.0
    } else {
        ctx.facing == "down"
            && (ctx.shape == "window" || (*bottom_pull_distance != 0.0 && pos <= 0.5))
    };
    if !visible {
        return Vec::new();
    }
    let base = Mat4::from_translation(Vec3::new(0.5, pos, 0.5));
    let local = if is_package_item(stack) {
        base * Mat4::from_scale(Vec3::splat(1.5))
    } else {
        base * Mat4::from_scale(Vec3::splat(0.5))
            * Mat4::from_rotation_x((pos * 180.0).to_radians())
            * Mat4::from_rotation_y((pos * 180.0).to_radians())
    };
    vec![WorldItemDraw {
        stack: stack.clone(),
        local,
        light_offset: [0, 0, 0],
        display: DisplayContext::Fixed,
        style: ItemPoseStyle::Raw,
    }]
}

fn basin_items(ctx: &LogisticsBlock) -> Vec<WorldItemDraw> {
    let Some(KineticLogisticsState::Basin {
        input_items,
        output_items,
        visualized_outputs,
        ingredient_rotation,
        ..
    }) = ctx.logistics
    else {
        return Vec::new();
    };
    let mut draws = Vec::new();
    let total = input_items.len() + output_items.len();
    if total > 0 {
        let level = 0.25f32; // Without synced fluid units on client, items rest in the basin floor zone.
        let rot_y = ingredient_rotation.get(ctx.alpha).to_radians();
        let center_rot =
            Mat4::from_translation(Vec3::new(0.5, 0.2, 0.5)) * Mat4::from_rotation_y(rot_y);
        let radius = if total == 1 { 0.0 } else { 0.125 };
        let angle_part = 360.0 / total as f32;
        let mut remaining = total;
        let mut rng_state = pos_hash(ctx.pos) as u32;
        let mut next_jitter = || -> f32 {
            rng_state = rng_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let unit = ((rng_state >> 8) as f32) / ((1u32 << 24) as f32);
            (unit * 2.0 - 1.0) / 16.0
        };
        for stack in input_items.iter().chain(output_items.iter()) {
            let theta = (angle_part * remaining as f32).to_radians();
            let offset = Vec3::new(radius * theta.cos(), level, radius * theta.sin());
            let base = center_rot
                * Mat4::from_translation(offset)
                * Mat4::from_rotation_y((angle_part * remaining as f32 + 35.0).to_radians())
                * Mat4::from_rotation_x(65.0f32.to_radians());
            let copies = (stack.count / 8 + 1) as usize;
            for _ in 0..copies {
                let jitter = Vec3::new(next_jitter(), next_jitter(), next_jitter());
                draws.push(WorldItemDraw {
                    stack: stack.clone(),
                    local: base * Mat4::from_translation(jitter),
                    light_offset: [0, 0, 0],
                    display: DisplayContext::Ground,
                    style: ItemPoseStyle::Raw,
                });
            }
            remaining -= 1;
        }
    }

    if ctx.facing != "down" && !visualized_outputs.is_empty() {
        let dir_vec = match ctx.facing {
            "north" => Vec3::NEG_Z,
            "south" => Vec3::Z,
            "west" => Vec3::NEG_X,
            "east" => Vec3::X,
            "up" => Vec3::Y,
            _ => Vec3::ZERO,
        };
        let out_vec = Vec3::splat(0.5) + dir_vec * 0.55 - Vec3::new(0.0, 0.5, 0.0);
        for (ticks_left, stack) in visualized_outputs {
            let progress =
                1.0 - (*ticks_left as f32 - ctx.alpha as f32) / OUTPUT_ANIMATION_TIME as f32;
            if progress > 0.35 {
                continue;
            }
            let local = Mat4::from_translation(
                out_vec
                    + Vec3::new(0.0, (-(progress * progress * 2.0)).max(-0.55), 0.0)
                    + dir_vec * (progress * 0.5),
            ) * Mat4::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
                * Mat4::from_rotation_x((progress * 180.0).to_radians());
            draws.push(WorldItemDraw {
                stack: stack.clone(),
                local,
                light_offset: [0, 0, 0],
                display: DisplayContext::Ground,
                style: ItemPoseStyle::Raw,
            });
        }
    }

    draws
}

fn frogport_parts_and_items(ctx: &LogisticsBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let (
        passive_yaw,
        goggles,
        target_rel,
        animated_package,
        depositing,
        animation_progress,
        anticipation_progress,
        manual_open,
    ) = match ctx.logistics {
        Some(KineticLogisticsState::Frogport {
            passive_yaw,
            goggles,
            target_rel,
            animated_package,
            depositing,
            animation_progress,
            anticipation_progress,
            manual_open,
            ..
        }) => (
            *passive_yaw,
            *goggles,
            *target_rel,
            animated_package.as_ref(),
            *depositing,
            *animation_progress,
            *anticipation_progress,
            *manual_open,
        ),
        _ => (
            0.0,
            false,
            None,
            None,
            false,
            LerpedValue::start_with(0.0),
            LerpedValue::start_with(0.0),
            LerpedValue::start_with(0.0),
        ),
    };

    let animating = (animation_progress.target - 1.0).abs() < 1e-4;
    let diff = match target_rel {
        Some([dx, dy, dz]) => {
            Vec3::new(dx as f32, dy as f32, dz as f32)
                - Vec3::new(0.0, if animating && depositing { 0.0 } else { 0.75 }, 0.0)
        }
        None => Vec3::ZERO,
    };
    let yaw = match target_rel {
        Some([dx, _, dz]) => (dx as f32).atan2(dz as f32).to_degrees() + 180.0,
        None => passive_yaw,
    };

    let (mut head_pitch, tongue_pitch, mut tongue_length) = if target_rel.is_some() {
        let horiz = (diff.x * diff.x + diff.z * diff.z).sqrt() + 3.0 / 16.0;
        let tp = diff.y.atan2(horiz).to_degrees();
        let tl = diff.length().max(1.0);
        let hp = (tp * 2.0).clamp(60.0, 100.0);
        (hp, tp, tl)
    } else {
        (80.0f32, 0.0f32, 0.0f32)
    };

    let mut draws = Vec::new();
    let head_pitch_mult = if animating {
        let progress = animation_progress.get(ctx.alpha);
        let (scale, item_distance, hp_mult) = if depositing {
            let modifier = (1.0 - ((progress - 0.25) * 4.0 - 1.0).powi(4)).max(0.0);
            let dist =
                (tongue_length * ((progress - 0.25) * 3.0).min(1.0)).max(tongue_length * modifier);
            tongue_length *= (1.0 - ((progress * 1.25 - 0.25) * 4.0 - 1.0).powi(4)).max(0.0);
            let hp = (1.0 - ((progress * 1.25) * 2.0 - 1.0).powi(4)).max(0.0);
            (0.25 + progress * 0.75, dist, hp)
        } else {
            tongue_length *= (1.0 - progress * 1.25).max(0.0).powi(5);
            let hp = 1.0 - (((progress * 1.5).powi(2) - 0.5) * 2.0).clamp(0.0, 1.0);
            let sc = (1.0 - progress * 1.25).max(0.5);
            (sc, tongue_length, hp)
        };
        if let Some(pkg) = animated_package
            && scale >= 0.45
        {
            let dir = if diff.length_squared() > 1e-6 {
                diff.normalize()
            } else {
                Vec3::ZERO
            };
            let offset = Vec3::new(0.0, 3.0 / 16.0, 0.0) + dir * item_distance
                - Vec3::new(0.0, if depositing { 0.75 } else { 0.0 }, 0.0);
            let local = Mat4::from_translation(offset)
                * Mat4::from_translation(Vec3::splat(0.5))
                * Mat4::from_scale(Vec3::splat(scale));
            draws.push(WorldItemDraw {
                stack: pkg.clone(),
                local,
                light_offset: [0, 0, 0],
                display: DisplayContext::Fixed,
                style: ItemPoseStyle::Raw,
            });
        }
        hp_mult
    } else {
        tongue_length = 0.0;
        let ant = anticipation_progress.get(ctx.alpha);
        if ant > 0.0 {
            (1.0 - ((ant * 1.25) * 2.0 - 1.0).powi(4)).max(0.0)
        } else {
            0.0
        }
    };

    head_pitch *= head_pitch_mult;
    let manual = manual_open.get(ctx.alpha);
    head_pitch = head_pitch.max(manual * 60.0);
    tongue_length = tongue_length.max(manual * 0.25);

    let yaw_rot = centred(Quat::from_rotation_y(yaw.to_radians()));
    let pivot = Vec3::new(8.0 / 16.0, 10.0 / 16.0, 11.0 / 16.0);
    let head_local = yaw_rot
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(head_pitch.to_radians())
        * Mat4::from_translation(-pivot);
    let head_model = if goggles {
        FROGPORT_HEAD_GOGGLES
    } else {
        FROGPORT_HEAD
    };
    let mut parts = vec![part(FROGPORT_BODY, yaw_rot), part(head_model, head_local)];

    if tongue_length > 1e-4 {
        let z_scale = tongue_length / (7.0 / 16.0);
        let tongue_local = yaw_rot
            * Mat4::from_translation(pivot)
            * Mat4::from_rotation_x(tongue_pitch.to_radians())
            * Mat4::from_scale(Vec3::new(1.0, 1.0, z_scale))
            * Mat4::from_translation(-pivot);
        parts.push(part(FROGPORT_TONGUE, tongue_local));
    }

    (parts, draws)
}

/// Emits partial model parts and world item draws for one Create logistics block.
pub(super) fn logistics_parts_and_items(
    ctx: &LogisticsBlock,
) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    match ctx.block {
        "create:belt" => belt_parts_and_items(ctx),
        "create:andesite_tunnel" | "create:brass_tunnel" => (tunnel_parts(ctx), Vec::new()),
        "create:blaze_burner" | "createaddition:liquid_blaze_burner" => {
            (blaze_burner_parts(ctx), Vec::new())
        }
        "create:depot" => match ctx.logistics {
            Some(KineticLogisticsState::Depot { items }) => (
                Vec::new(),
                depot_item_draws(items, ctx.alpha, Mat4::IDENTITY),
            ),
            _ => (Vec::new(), Vec::new()),
        },
        "create:chute" | "create:smart_chute" => (Vec::new(), chute_items(ctx)),
        "create:basin" => (Vec::new(), basin_items(ctx)),
        "create:package_frogport" => frogport_parts_and_items(ctx),
        _ => (Vec::new(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn stack(id: &str, count: u32) -> ItemStack {
        ItemStack {
            item: Arc::from(id),
            count,
            damage: 0,
            extra: None,
        }
    }

    fn base_ctx<'a>(block: &'a str, facing: &'a str) -> LogisticsBlock<'a> {
        LogisticsBlock {
            block,
            facing,
            slope: "horizontal",
            part: "middle",
            shape: "",
            blaze: "none",
            pos: [0, 0, 0],
            speed: 0.0,
            time: 0.0,
            alpha: 0.0,
            logistics: None,
        }
    }

    #[test]
    fn belt_renders_segments_pulley_dyed_shift_and_items() {
        let mut inv = Compound::new();
        let mut item_nbt = Compound::new();
        item_nbt.insert(
            JavaString::from("id"),
            Tag::String(JavaString::from("minecraft:iron_ingot")),
        );
        item_nbt.insert(JavaString::from("count"), Tag::Int(1));
        let mut tis = Compound::new();
        tis.insert(JavaString::from("Item"), Tag::Compound(item_nbt));
        tis.insert(JavaString::from("Pos"), Tag::Float(1.5f32.to_bits()));
        tis.insert(JavaString::from("PrevPos"), Tag::Float(1.0f32.to_bits()));
        tis.insert(JavaString::from("Offset"), Tag::Float(0.0f32.to_bits()));
        tis.insert(JavaString::from("Angle"), Tag::Int(90));
        inv.insert(
            JavaString::from("Items"),
            Tag::List(vec![Tag::Compound(tis)]),
        );

        let mut belt = Compound::new();
        belt.insert(JavaString::from("IsController"), Tag::Byte(1));
        belt.insert(JavaString::from("Length"), Tag::Int(3));
        belt.insert(JavaString::from("Index"), Tag::Int(0));
        belt.insert(
            JavaString::from("Dye"),
            Tag::String(JavaString::from("BLUE")),
        );
        belt.insert(JavaString::from("Inventory"), Tag::Compound(inv));

        let st = logistics_state_from_nbt(None, &Tag::Compound(belt)).unwrap();
        let ctx = LogisticsBlock {
            part: "start",
            speed: 64.0,
            time: 5.0,
            alpha: 0.5,
            logistics: Some(&st),
            ..base_ctx("create:belt", "north")
        };
        let (parts, items) = logistics_parts_and_items(&ctx);
        assert_eq!(parts.len(), 3); // bottom + top + pulley
        assert_eq!(parts[0].model, BELT_START_BOTTOM);
        assert_eq!(parts[1].model, BELT_START);
        assert_eq!(parts[2].model, BELT_PULLEY);
        assert_eq!(
            parts[1].sprite_shift.unwrap().target,
            "create:block/belt/blue_scroll"
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].stack.item.as_ref(), "minecraft:iron_ingot");

        // Diagonal upward swaps start/end on downward only and uses diagonal scroll.
        let diag = LogisticsBlock {
            slope: "downward",
            part: "start",
            speed: 32.0,
            logistics: Some(&st),
            ..base_ctx("create:belt", "east")
        };
        let (d_parts, _) = logistics_parts_and_items(&diag);
        assert_eq!(d_parts.len(), 2); // 1 diagonal belt + 1 pulley
        assert_eq!(d_parts[0].model, BELT_DIAGONAL_END);
        assert_eq!(
            d_parts[0].sprite_shift.unwrap().target,
            "create:block/belt/blue_diagonal_scroll"
        );
    }

    #[test]
    fn belt_items_follow_signed_speed_on_both_axes() {
        let initial = KineticLogisticsState::Belt {
            is_controller: true,
            belt_length: 3,
            dye: None,
            items: vec![TransportedItem {
                stack: stack("minecraft:iron_ingot", 1),
                belt_pos: 1.5,
                prev_belt_pos: 1.0,
                side_offset: 0.0,
                prev_side_offset: 0.0,
                angle: 0,
                inserted_from: 0,
                locked: false,
                locked_externally: false,
            }],
            lazy_item: None,
        };
        for (facing, forward, reverse) in [
            ("north", 1.0, 2.0),
            ("south", 2.0, 1.0),
            ("east", 1.0, 2.0),
            ("west", 2.0, 1.0),
        ] {
            for (belt_speed, expected_pos) in [(240.0, forward), (-240.0, reverse)] {
                let mut state = initial.clone();
                let ctx = LogisticsTickCtx {
                    player_xz: None,
                    facing: facing.into(),
                    blaze: String::new(),
                    valid_block_above: false,
                    mixer_running_above: false,
                    belt_speed,
                };
                tick_logistics_state(&mut state, [0, 0, 0], &ctx);
                let KineticLogisticsState::Belt { items, .. } = state else {
                    panic!("expected belt state");
                };
                assert_eq!(
                    items[0].belt_pos, expected_pos,
                    "{facing}, {belt_speed} RPM"
                );
                assert_eq!(items[0].prev_belt_pos, 1.5);
            }
        }
    }

    #[test]
    fn tunnel_parses_flaps_and_emits_four_flaps_per_horizontal_side() {
        let mut tag = Compound::new();
        tag.insert(
            JavaString::from("Flaps"),
            Tag::List(vec![Tag::Int(2), Tag::Int(3), Tag::Int(1)]),
        );
        let st = logistics_state_from_nbt(None, &Tag::Compound(tag)).unwrap();
        let (parts, items) = logistics_parts_and_items(&LogisticsBlock {
            logistics: Some(&st),
            ..base_ctx("create:andesite_tunnel", "north")
        });
        assert!(items.is_empty());
        assert_eq!(parts.len(), 8); // only horizontal sides (north + south), 4 flaps each
        assert!(parts.iter().all(|p| p.model == BELT_TUNNEL_FLAP));
    }

    #[test]
    fn blaze_burner_draws_head_rods_and_scrolling_flame_by_heat_level() {
        assert!(blaze_burner_parts(&base_ctx("create:blaze_burner", "north")).is_empty());

        let mut tag = Compound::new();
        tag.insert(JavaString::from("fuelLevel"), Tag::Int(2));
        tag.insert(JavaString::from("Goggles"), Tag::Byte(1));
        tag.insert(JavaString::from("TrainHat"), Tag::Byte(1));
        let mut st = logistics_state_from_nbt(None, &Tag::Compound(tag)).unwrap();

        let tick_ctx = LogisticsTickCtx {
            player_xz: Some([10.0, 0.5]),
            facing: "south".into(),
            blaze: "seething".into(),
            valid_block_above: true,
            mixer_running_above: false,
            belt_speed: 0.0,
        };
        for _ in 0..25 {
            tick_logistics_state(&mut st, [0, 0, 0], &tick_ctx);
        }

        let (parts, _) = logistics_parts_and_items(&LogisticsBlock {
            blaze: "seething",
            time: 10.0,
            logistics: Some(&st),
            ..base_ctx("create:blaze_burner", "south")
        });
        let models: Vec<&str> = parts.iter().map(|p| p.model).collect();
        assert_eq!(
            models,
            [
                BLAZE_SUPER_ACTIVE,
                BLAZE_GOGGLES,
                TRAIN_HAT,
                BLAZE_SUPER_RODS_SMALL,
                BLAZE_SUPER_RODS_LARGE,
                BLAZE_FLAME,
            ]
        );
        assert!(parts.iter().all(|p| p.full_bright));
        assert_eq!(
            parts[5].sprite_shift.unwrap().target,
            "create:block/blaze_burner_flame_superheated_scroll"
        );
    }

    #[test]
    fn smart_chute_draws_the_item_at_or_below_its_outlet() {
        for (position, visible) in [(0.25, false), (0.0, true), (-0.25, true)] {
            let mut chute = KineticLogisticsState::Chute {
                item: Some(stack("minecraft:cobblestone", 1)),
                item_pos: LerpedValue::start_with(position),
                pull: 0.0,
                push: 0.0,
                bottom_pull_distance: 0.0,
            };
            let (_, draws) = logistics_parts_and_items(&LogisticsBlock {
                logistics: Some(&chute),
                ..base_ctx("create:smart_chute", "north")
            });
            assert_eq!(draws.len(), usize::from(visible));
            if let Some(draw) = draws.first() {
                assert_eq!(
                    draw.local.transform_point3(Vec3::ZERO),
                    Vec3::new(0.5, position, 0.5)
                );
            }
            if position < 0.0 {
                tick_logistics_state(
                    &mut chute,
                    [0, 0, 0],
                    &LogisticsTickCtx {
                        player_xz: None,
                        facing: "down".into(),
                        blaze: String::new(),
                        valid_block_above: false,
                        mixer_running_above: false,
                        belt_speed: 0.0,
                    },
                );
                let KineticLogisticsState::Chute { item_pos, .. } = chute else {
                    unreachable!();
                };
                assert_eq!(item_pos.value, position);
            }
        }
    }

    #[test]
    fn depot_chute_and_basin_emit_world_items() {
        let depot = KineticLogisticsState::Depot {
            items: DepotItems {
                held_item: Some(TransportedItem {
                    stack: stack("minecraft:diamond", 1),
                    belt_pos: 0.5,
                    prev_belt_pos: 0.5,
                    side_offset: 0.0,
                    prev_side_offset: 0.0,
                    angle: 45,
                    inserted_from: 1,
                    locked: false,
                    locked_externally: false,
                }),
                incoming: Vec::new(),
                output_buffer: vec![(0, stack("minecraft:emerald", 1))],
            },
        };
        let (_, depot_draws) = logistics_parts_and_items(&LogisticsBlock {
            logistics: Some(&depot),
            ..base_ctx("create:depot", "north")
        });
        assert_eq!(depot_draws.len(), 2);
        assert_eq!(item_draw_matrices(&depot_draws[0], false).len(), 1);

        let mut chute = KineticLogisticsState::Chute {
            item: Some(stack("minecraft:cobblestone", 1)),
            item_pos: LerpedValue::start_with(0.4),
            pull: 0.0,
            push: 0.0,
            bottom_pull_distance: 0.0,
        };
        let (_, window_draws) = logistics_parts_and_items(&LogisticsBlock {
            shape: "window",
            logistics: Some(&chute),
            ..base_ctx("create:chute", "down")
        });
        assert_eq!(window_draws.len(), 1);
        tick_logistics_state(
            &mut chute,
            [0, 0, 0],
            &LogisticsTickCtx {
                player_xz: None,
                facing: "down".into(),
                blaze: String::new(),
                valid_block_above: false,
                mixer_running_above: false,
                belt_speed: 0.0,
            },
        );
        if let KineticLogisticsState::Chute { item_pos, .. } = chute {
            assert!((item_pos.value - 0.2).abs() < 1e-4);
        }

        let mut basin = KineticLogisticsState::Basin {
            input_items: vec![stack("minecraft:wheat", 16)],
            output_items: vec![stack("minecraft:bread", 1)],
            visualized_outputs: vec![(9, stack("minecraft:cake", 1))],
            ingredient_rotation: LerpedValue::start_with(0.0),
            ingredient_rotation_speed: LerpedValue::start_with(0.0),
        };
        let (_, basin_draws) = logistics_parts_and_items(&LogisticsBlock {
            facing: "north",
            logistics: Some(&basin),
            ..base_ctx("create:basin", "north")
        });
        // 16 wheat -> 3 copies, 1 bread -> 1 copy, 1 visualized cake (progress 0.1 <= 0.35) -> 5 draws.
        assert_eq!(basin_draws.len(), 5);
        tick_logistics_state(
            &mut basin,
            [0, 0, 0],
            &LogisticsTickCtx {
                player_xz: None,
                facing: "north".into(),
                blaze: String::new(),
                valid_block_above: false,
                mixer_running_above: true,
                belt_speed: 0.0,
            },
        );
        if let KineticLogisticsState::Basin {
            ingredient_rotation,
            ..
        } = basin
        {
            assert!(ingredient_rotation.value > 0.0);
        }
    }

    #[test]
    fn frogport_animates_head_tongue_and_package_transfer() {
        let mut tag = Compound::new();
        tag.insert(JavaString::from("PlacedYaw"), Tag::Float(90.0f32.to_bits()));
        tag.insert(JavaString::from("Goggles"), Tag::Byte(1));
        tag.insert(JavaString::from("Deposit"), Tag::Byte(1));
        let mut target = Compound::new();
        target.insert(
            JavaString::from("relative_pos"),
            Tag::IntArray(vec![0, 2, 3]),
        );
        tag.insert(JavaString::from("Target"), Tag::Compound(target));
        let mut pkg = Compound::new();
        pkg.insert(
            JavaString::from("id"),
            Tag::String(JavaString::from("create:cardboard_package_12x12")),
        );
        pkg.insert(JavaString::from("count"), Tag::Int(1));
        tag.insert(JavaString::from("AnimatedPackage"), Tag::Compound(pkg));

        let mut st = logistics_state_from_nbt(None, &Tag::Compound(tag)).unwrap();
        let tick_ctx = LogisticsTickCtx {
            player_xz: None,
            facing: "north".into(),
            blaze: String::new(),
            valid_block_above: false,
            mixer_running_above: false,
            belt_speed: 0.0,
        };
        for _ in 0..4 {
            tick_logistics_state(&mut st, [0, 0, 0], &tick_ctx);
        }
        let (parts, items) = logistics_parts_and_items(&LogisticsBlock {
            logistics: Some(&st),
            ..base_ctx("create:package_frogport", "north")
        });
        assert_eq!(parts[0].model, FROGPORT_BODY);
        assert_eq!(parts[1].model, FROGPORT_HEAD_GOGGLES);
        assert_eq!(parts.len(), 3); // tongue visible mid-flight
        assert_eq!(parts[2].model, FROGPORT_TONGUE);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].stack.item.as_ref(),
            "create:cardboard_package_12x12"
        );
    }
}
