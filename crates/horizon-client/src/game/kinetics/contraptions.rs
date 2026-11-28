//! Create schematicannons, rollers, chain conveyors, sliding/folding doors,
//! bells, item drains, train stations and bogeys.
//!
//! Ported from `Creators-of-Create/Create` (`SchematicannonRenderer`,
//! `RollerRenderer`, `ChainConveyorRenderer`, `SlidingDoorRenderer`,
//! `BellRenderer`, `DeskBellRenderer`, `ItemDrainRenderer`, `StationRenderer`,
//! `StandardBogeyRenderer`, MIT) and `Creators-of-Create/Ponder` (MIT).

use super::arm::read_block_pos;
use super::levers::dir_normal_vec3;
use super::logistics::{
    ItemPoseStyle, WorldItemDraw, depot_item_draws, is_package_item, parse_depot_items,
    parse_transported_item,
};
use super::machines::{MachinePart, SpriteShift, centred, horizontal_angle, part};
use super::{angle_radians, axis_y_rotation};
use glam::{Mat4, Quat, Vec3};
use horizon_assets::item_model::DisplayContext;
pub(super) use horizon_client::mesher::is_create_animated_door as is_sliding_door;
use horizon_client::world::{KineticContraptionState, LerpedValue, TransportedItem};
use horizon_client::{hud::Rect, mesher::Vertex};
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::f32::consts::{FRAC_PI_2, PI};

const CANNON_CONNECTOR: &str = "create:block/schematicannon/connector";
const CANNON_PIPE: &str = "create:block/schematicannon/pipe";

const ROLLER_WHEEL: &str = "create:block/mechanical_roller/wheel";
const ROLLER_FRAME: &str = "create:block/mechanical_roller/frame";

const CHAIN_WHEEL: &str = "create:block/chain_conveyor/wheel";
const CHAIN_SHAFT: &str = "create:block/chain_conveyor/shaft";
const CHAIN_GUARD: &str = "create:block/chain_conveyor/guard";
pub(super) const CHAIN_SPRITE: &str = "minecraft:block/chain";

const PECULIAR_BELL: &str = "create:block/peculiar_bell";
const HAUNTED_BELL: &str = "create:block/haunted_bell";
const DESK_BELL_PLUNGER: &str = "create:block/desk_bell/plunger";
const DESK_BELL_BELL: &str = "create:block/desk_bell/bell";

const STATION_ON: &str = "create:block/track_station/flag_on";
const STATION_OFF: &str = "create:block/track_station/flag_off";
const STATION_ASSEMBLE: &str = "create:block/track_station/flag_assemble";

const BOGEY_FRAME: &str = "create:block/track/bogey/bogey_frame";
const SMALL_BOGEY_WHEELS: &str = "create:block/track/bogey/bogey_wheel";
const BOGEY_DRIVE: &str = "create:block/track/bogey/bogey_drive";
const BOGEY_DRIVE_BELT: &str = "create:block/track/bogey/bogey_drive_belt";
const BOGEY_PISTON: &str = "create:block/track/bogey/bogey_drive_piston";
const LARGE_BOGEY_WHEELS: &str = "create:block/track/bogey/bogey_drive_wheel";
const BOGEY_PIN: &str = "create:block/track/bogey/bogey_drive_wheel_pin";
const SHAFT: &str = "create:block/shaft";

const BELL_RING_DURATION: i32 = 74;

/// Inputs for rendering one Create contraption/door/bell/station/bogey block.
pub(super) struct ContraptionBlock<'a> {
    pub block: &'a str,
    pub facing: &'a str,
    pub axis: &'a str,
    pub half: &'a str,
    pub hinge: &'a str,
    pub attachment: &'a str,
    pub open: bool,
    pub visible: bool,
    pub powered: bool,
    pub assembling: bool,
    pub pos: [i32; 3],
    pub speed: f32,
    pub time: f64,
    pub alpha: f64,
    pub contraption: Option<&'a KineticContraptionState>,
}

/// Per-tick blockstate flags needed for contraption/door/bell/station states.
pub(super) struct ContraptionTickCtx {
    pub open: bool,
    pub visible: bool,
    pub powered: bool,
    pub assembling: bool,
}

/// Whether `block` is handled by this module.
pub(super) fn is_contraption_block(block: &str) -> bool {
    matches!(
        block,
        "create:schematicannon"
            | "create:mechanical_roller"
            | "create:chain_conveyor"
            | "create:peculiar_bell"
            | "create:haunted_bell"
            | "create:desk_bell"
            | "create:item_drain"
            | "create:track_station"
            | "create:small_bogey"
            | "create:large_bogey"
    ) || is_sliding_door(block)
}

/// Whether a block entity packet belongs to one of these Create block entities.
pub(super) fn is_contraption_be_tag(name: &str, tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    if fields.is_empty() {
        return false;
    }
    matches!(
        name,
        "create:schematicannon"
            | "create:mechanical_roller"
            | "create:chain_conveyor"
            | "create:sliding_door"
            | "create:peculiar_bell"
            | "create:haunted_bell"
            | "create:desk_bell"
            | "create:item_drain"
            | "create:track_station"
            | "create:bogey"
    )
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

fn dir_from_name(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "down" => 0,
        "up" => 1,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => 2,
    }
}

fn ring_ccw_axis(dir_idx: u8) -> Vec3 {
    match dir_idx {
        2 => Vec3::NEG_X, // north ccw = west
        3 => Vec3::X,     // south ccw = east
        4 => Vec3::Z,     // west ccw = south
        5 => Vec3::NEG_Z, // east ccw = north
        _ => Vec3::NEG_X,
    }
}

/// Parses an update tag into `KineticContraptionState` if it carries matching fields.
pub(super) fn contraption_state_from_nbt(
    existing: Option<&KineticContraptionState>,
    tag: &Tag,
) -> Option<KineticContraptionState> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let has = |k: &str| fields.contains_key(&JavaString::from(k));

    if has("DefaultYaw") || (has("Status") && has("RemainingFuel")) {
        let default_yaw = read_f32(fields, "DefaultYaw").unwrap_or(0.0);
        let target = match fields.get(&JavaString::from("Printer")) {
            Some(Tag::Compound(p)) => p
                .get(&JavaString::from("CurrentPos"))
                .and_then(read_block_pos),
            _ => None,
        };
        let (prev_target, prev_recoil) = match existing {
            Some(KineticContraptionState::Schematicannon {
                target: old_t,
                recoil_ticks,
                ..
            }) => (*old_t, *recoil_ticks),
            _ => (target, 0),
        };
        let flying_launch = match fields.get(&JavaString::from("FlyingBlocks")) {
            Some(Tag::List(list)) if !list.is_empty() => {
                let mut best = 0;
                for item in list {
                    if let Tag::Compound(fc) = item {
                        let total = read_i32(fc, "TotalTicks").unwrap_or(20);
                        let left = read_i32(fc, "TicksLeft").unwrap_or(total);
                        let r = (left + 1) - total + 10;
                        if r > best {
                            best = r;
                        }
                    }
                }
                best
            }
            _ => 0,
        };
        return Some(KineticContraptionState::Schematicannon {
            default_yaw,
            target,
            prev_target,
            recoil_ticks: prev_recoil.max(flying_launch),
        });
    }

    if let Some(Tag::List(list)) = fields.get(&JavaString::from("Connections")) {
        let connections = list.iter().filter_map(read_block_pos).collect();
        return Some(KineticContraptionState::ChainConveyor { connections });
    }

    if let Some(Tag::String(dir)) = fields.get(&JavaString::from("Ringing")) {
        return Some(KineticContraptionState::Bell {
            ringing: true,
            ringing_ticks: 0,
            ring_direction: dir_from_name(&dir.to_string_lossy()),
        });
    }

    // Create 6.0.10 sends a Ding marker on every press, including while powered.
    if has("Ding") {
        let animation_offset = match existing {
            Some(KineticContraptionState::DeskBell {
                animation_offset, ..
            }) => *animation_offset,
            _ => 0.0,
        };
        return Some(KineticContraptionState::DeskBell {
            animation: LerpedValue::start_with(1.0),
            animation_offset,
        });
    }

    if has("ProcessingTicks") && !has("OutputBuffer") && !has("Lid") {
        let held_item = fields
            .get(&JavaString::from("HeldItem"))
            .and_then(parse_transported_item);
        let processing_ticks = read_i32(fields, "ProcessingTicks").unwrap_or(0);
        return Some(KineticContraptionState::ItemDrain {
            held_item,
            processing_ticks,
        });
    }

    if has("TargetTrack") || has("Station") || (has("Flag") && has("OutputBuffer")) {
        let flag = match existing {
            Some(KineticContraptionState::TrackStation { flag, .. }) => *flag,
            _ => LerpedValue::start_with(0.0),
        };
        return Some(KineticContraptionState::TrackStation {
            flag,
            depot: parse_depot_items(fields),
        });
    }

    None
}

/// Advances a contraption/door/bell/drain/station animation state by one tick.
pub(super) fn tick_contraption_state(
    state: &mut KineticContraptionState,
    ctx: &ContraptionTickCtx,
) {
    match state {
        KineticContraptionState::Schematicannon {
            target,
            prev_target,
            recoil_ticks,
            ..
        } => {
            *prev_target = *target;
            if *recoil_ticks > 0 {
                *recoil_ticks -= 1;
            }
        }
        KineticContraptionState::SlidingDoor {
            animation,
            bridge_ticks,
        } => {
            animation.chase_linear(if ctx.open { 1.0 } else { 0.0 }, 0.15);
            animation.tick();
            if ctx.open && *bridge_ticks < 2 {
                *bridge_ticks += 1;
            } else if !ctx.open && ctx.visible && *bridge_ticks > 0 {
                *bridge_ticks -= 1;
            }
        }
        KineticContraptionState::Bell {
            ringing,
            ringing_ticks,
            ..
        } => {
            if *ringing {
                *ringing_ticks += 1;
                if *ringing_ticks >= BELL_RING_DURATION {
                    *ringing = false;
                    *ringing_ticks = 0;
                }
            }
        }
        KineticContraptionState::DeskBell { animation, .. } => {
            if ctx.powered && animation.value < 0.01 {
                *animation = LerpedValue::start_with(1.0);
            } else {
                animation.chase_linear(0.0, 0.05);
                animation.tick();
            }
        }
        KineticContraptionState::ItemDrain {
            held_item: Some(tis),
            processing_ticks,
        } => {
            if *processing_ticks <= 0 {
                tis.prev_belt_pos = tis.belt_pos;
                tis.prev_side_offset = tis.side_offset;
                tis.belt_pos = (tis.belt_pos + 1.0 / 16.0).min(1.0);
            } else {
                *processing_ticks -= 1;
            }
        }
        KineticContraptionState::TrackStation { flag, .. } => {
            flag.chase_linear(if ctx.assembling { 1.0 } else { 0.0 }, 0.05);
            flag.tick();
        }
        KineticContraptionState::ChainConveyor { .. }
        | KineticContraptionState::ItemDrain {
            held_item: None, ..
        } => {}
    }
}

fn schematicannon_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let (default_yaw, target, prev_target, recoil_ticks) = match ctx.contraption {
        Some(KineticContraptionState::Schematicannon {
            default_yaw,
            target,
            prev_target,
            recoil_ticks,
        }) => (*default_yaw, *target, *prev_target, *recoil_ticks),
        _ => (0.0, None, None, 0),
    };
    let (yaw_deg, pitch_deg) = if let Some(t) = target {
        let cur = Vec3::new(
            (t[0] - ctx.pos[0]) as f32,
            (t[1] - ctx.pos[1]) as f32,
            (t[2] - ctx.pos[2]) as f32,
        );
        let diff = match prev_target {
            Some(pt) => {
                let prev = Vec3::new(
                    (pt[0] - ctx.pos[0]) as f32,
                    (pt[1] - ctx.pos[1]) as f32,
                    (pt[2] - ctx.pos[2]) as f32,
                );
                prev + (cur - prev) * ctx.alpha as f32
            }
            None => cur,
        };
        let yaw = diff.x.atan2(diff.z).to_degrees();
        let dist = (diff.x * diff.x + diff.z * diff.z).sqrt();
        let pitch = dist.atan2(diff.y * 3.0 + dist * 2.0).to_degrees() + 10.0;
        (yaw, pitch)
    } else {
        (default_yaw, 40.0)
    };
    let recoil = (recoil_ticks as f32 - ctx.alpha as f32).clamp(0.0, 10.0);

    let yaw_rad = (yaw_deg + 90.0).to_radians();
    let connector_local = Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
        * Mat4::from_rotation_y(yaw_rad)
        * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
    let pipe_pivot = Vec3::new(0.5, 15.0 / 16.0, 0.5);
    let pipe_local = Mat4::from_translation(pipe_pivot)
        * Mat4::from_rotation_y(yaw_rad)
        * Mat4::from_rotation_z(pitch_deg.to_radians())
        * Mat4::from_translation(-pipe_pivot)
        * Mat4::from_translation(Vec3::new(0.0, -recoil / 100.0, 0.0));

    vec![
        part(CANNON_CONNECTOR, connector_local),
        part(CANNON_PIPE, pipe_local),
    ]
}

fn roller_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let facing = ctx.facing;
    let frame_local = centred(Quat::from_rotation_y(
        (horizontal_angle(facing) + 180.0).to_radians(),
    ));
    let angle_deg = (ctx.time as f32 / 20.0 * ctx.speed) % 360.0;
    let facing_norm = dir_normal_vec3(facing) * (17.0 / 16.0);
    let wheel_local = Mat4::from_translation(Vec3::new(0.0, -0.25, 0.0))
        * Mat4::from_translation(facing_norm)
        * centred(Quat::from_rotation_y(horizontal_angle(facing).to_radians()))
        * Mat4::from_rotation_x((-angle_deg).to_radians())
        * Mat4::from_translation(Vec3::new(0.0, -0.5, 0.5))
        * Mat4::from_rotation_y(FRAC_PI_2);
    vec![
        part(ROLLER_WHEEL, wheel_local),
        part(ROLLER_FRAME, frame_local),
    ]
}

fn chain_conveyor_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let angle = angle_radians(ctx.time, ctx.speed, 0.0);
    let spin_y = centred(Quat::from_rotation_y(angle));
    let mut parts = vec![part(CHAIN_SHAFT, spin_y), part(CHAIN_WHEEL, Mat4::IDENTITY)];
    parts.extend(
        chain_spans(ctx).map(|span| part(CHAIN_GUARD, centred(Quat::from_rotation_y(span.yaw())))),
    );
    parts
}

pub(super) struct ChainSpan {
    pub relative: [i32; 3],
    start: Vec3,
    end: Vec3,
    phase: f32,
}

pub(super) fn chain_spans<'a>(
    ctx: &ContraptionBlock<'a>,
) -> impl Iterator<Item = ChainSpan> + use<'a> {
    let connections = match ctx.contraption {
        Some(KineticContraptionState::ChainConveyor { connections }) => connections.as_slice(),
        _ => &[],
    };
    let branch = 35.0_f32
        .to_radians()
        .copysign(if ctx.speed < 0.0 { -1.0 } else { 1.0 });
    let phase = (ctx.time * f64::from(ctx.speed.abs()) / 360.0).rem_euclid(1.0) as f32 - 0.5;
    connections.iter().copied().filter_map(move |relative| {
        let [x, y, z] = relative.map(|n| n as f32);
        if x == 0.0 && z == 0.0 {
            return None;
        }
        let yaw = x.atan2(z);
        let center = Vec3::new(0.5, 6.0 / 16.0, 0.5);
        let rim = Vec3::Z * 1.25;
        Some(ChainSpan {
            relative,
            start: center + Quat::from_rotation_y(yaw - branch) * rim,
            end: center + Vec3::new(x, y, z) + Quat::from_rotation_y(yaw + PI + branch) * rim,
            phase,
        })
    })
}

impl ChainSpan {
    fn yaw(&self) -> f32 {
        let delta = self.end - self.start;
        delta.x.atan2(delta.z)
    }

    pub(super) fn draw(
        &self,
        origin: Vec3,
        sprite: &Rect,
        lights: [u32; 2],
        out: &mut Vec<Vertex>,
    ) {
        let delta = self.end - self.start;
        let length = delta.length();
        if length <= f32::EPSILON {
            return;
        }
        // Use the far strip for oversized saved connections as well, so a malformed
        // offset cannot generate an unbounded number of atlas repeats.
        let far = length > 256.0
            || (origin + (self.start + self.end) * 0.5).length_squared() >= 48.0 * 48.0;
        let pitch = delta.y.atan2(delta.x.hypot(delta.z));
        let rotation = Quat::from_rotation_y(self.yaw())
            * Quat::from_rotation_x(FRAC_PI_2 - pitch)
            * Quat::from_rotation_y(PI / 4.0);
        let radius = if far { 1.0 / 16.0 } else { 1.5 / 16.0 };
        let end = origin + self.end;
        let direction = delta / length;
        let light_at = |distance: f32| {
            let t = distance / length;
            let channel = |shift| {
                let end = ((lights[1] >> shift) & 0xff_u32) as f32;
                let start = ((lights[0] >> shift) & 0xff_u32) as f32;
                (end + (start - end) * t).round() as u32
            };
            channel(0) | (channel(8) << 8)
        };
        // Each strip uses the same vertical repeat as Create's standalone
        // texture, split at wrap boundaries to keep sampling inside the atlas.
        let mut distance = 0.0;
        let mut v = self.phase.rem_euclid(1.0);
        while distance < length {
            let next = if far {
                length
            } else {
                (distance + 1.0 - v).min(length)
            };
            let (v0, v1) = if far {
                (0.0, 1.0 / 16.0)
            } else {
                (v, (v + next - distance).min(1.0))
            };
            let a = end - direction * distance;
            let b = end - direction * next;
            let a_light = light_at(distance);
            let b_light = light_at(next);
            for (axis, near_u) in [(Vec3::Z, 0.0), (Vec3::X, 3.0 / 16.0)] {
                let cross = rotation * axis * radius;
                let (u0, u1) = if far {
                    (3.0 / 16.0, 4.0 / 16.0)
                } else {
                    (near_u, near_u + 3.0 / 16.0)
                };
                for cross in [cross, -cross] {
                    for (pos, u, v, light) in [
                        (a + cross, u1, v0, a_light),
                        (b + cross, u1, v1, b_light),
                        (b - cross, u0, v1, b_light),
                        (a - cross, u0, v0, a_light),
                    ] {
                        out.push(Vertex {
                            pos: pos.to_array(),
                            uv: [
                                sprite.u0 + u * (sprite.u1 - sprite.u0),
                                sprite.v0 + v * (sprite.v1 - sprite.v0),
                            ],
                            layer: 0,
                            color: u32::MAX,
                            light,
                            normal: horizon_client::mesher::CULL_BACK,
                            glint_uv: [0.0; 2],
                        });
                    }
                }
            }
            distance = next;
            v = 0.0;
        }
    }
}

fn door_animation(ctx: &ContraptionBlock) -> Option<f32> {
    if ctx.half == "upper" {
        return None;
    }
    let (value, bridge_ticks) = match ctx.contraption {
        Some(KineticContraptionState::SlidingDoor {
            animation,
            bridge_ticks,
        }) => (animation.get(ctx.alpha).clamp(0.0, 1.0), *bridge_ticks),
        _ => (if ctx.open { 1.0 } else { 0.0 }, 0),
    };
    (!ctx.visible || bridge_ticks != 0).then_some(value)
}

fn folding_door_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let Some(value) = door_animation(ctx) else {
        return Vec::new();
    };
    let v2 = (value * 10.0).clamp(0.0, 1.0);
    let facing_norm = dir_normal_vec3(ctx.facing);

    {
        let (fold_left, fold_right) = if ctx.block == "create:andesite_door" {
            (
                "create:block/andesite_door/fold_left",
                "create:block/andesite_door/fold_right",
            )
        } else {
            (
                "create:block/copper_door/fold_left",
                "create:block/copper_door/fold_right",
            )
        };
        let flip = ctx.hinge == "right";
        let f = if flip { -1.0f32 } else { 1.0f32 };
        let cw_y = horizontal_angle(match ctx.facing {
            "north" => "east",
            "east" => "south",
            "south" => "west",
            "west" => "north",
            other => other,
        });
        let mut parts = Vec::with_capacity(2);
        for left in [true, false] {
            let model = if left ^ flip { fold_left } else { fold_right };
            let mut m = Mat4::from_translation(
                Vec3::new(0.0, -1.0 / 512.0, 0.0) + facing_norm * (v2 / 32.0),
            ) * centred(Quat::from_rotation_y(cw_y.to_radians()));
            if flip {
                m *= Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0));
            }
            m *= Mat4::from_rotation_y((91.0 * f * value * value).to_radians());
            if !left {
                m *= Mat4::from_translation(Vec3::new(0.0, 0.0, f * 0.5))
                    * Mat4::from_rotation_y((-181.0 * f * value * value).to_radians());
            }
            if flip {
                m *= Mat4::from_translation(Vec3::new(0.0, 0.0, -0.5));
            }
            parts.push(part(model, m));
        }
        parts
    }
}

/// Slide the two closed block-state models without changing their orientation.
pub(super) fn sliding_door_halves(ctx: &ContraptionBlock) -> Option<[(&'static str, Vec3); 2]> {
    if !matches!(
        ctx.block,
        "create:train_door" | "create:framed_glass_door" | "create:brass_door"
    ) {
        return None;
    }
    let value = door_animation(ctx)?;
    let cw = match ctx.facing {
        "north" => "east",
        "east" => "south",
        "south" => "west",
        "west" => "north",
        other => other,
    };
    let move_dir = if ctx.hinge == "left" {
        dir_normal_vec3(super::opposite_facing(cw))
    } else {
        dir_normal_vec3(cw)
    };
    let offset = move_dir * (value * value * 13.0 / 16.0)
        + dir_normal_vec3(ctx.facing) * ((value * 10.0).clamp(0.0, 1.0) / 32.0);
    Some([
        ("lower", offset),
        ("upper", offset + Vec3::Y * (1.0 - 1.0 / 512.0)),
    ])
}

fn bell_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let model = if ctx.block == "create:haunted_bell" {
        HAUNTED_BELL
    } else {
        PECULIAR_BELL
    };
    let swing = match ctx.contraption {
        Some(KineticContraptionState::Bell {
            ringing: true,
            ringing_ticks,
            ring_direction,
        }) => {
            let t = (*ringing_ticks as f32 + ctx.alpha as f32) / 1.5;
            let angle = 1.2 * (t / PI).sin() / (2.5 + t / 3.0);
            centred(Quat::from_axis_angle(ring_ccw_axis(*ring_direction), angle))
        }
        _ => Mat4::IDENTITY,
    };
    let mut r_y = horizontal_angle(ctx.facing);
    if matches!(ctx.attachment, "single_wall" | "double_wall") {
        r_y += 90.0;
    }
    vec![part(
        model,
        swing * centred(Quat::from_rotation_y(r_y.to_radians())),
    )]
}

fn desk_bell_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let (p, anim_offset) = match ctx.contraption {
        Some(KineticContraptionState::DeskBell {
            animation,
            animation_offset,
        }) => (animation.get(ctx.alpha).clamp(0.0, 1.0), *animation_offset),
        _ => (0.0, 0.0),
    };
    if p < 0.004 && !ctx.powered {
        return Vec::new();
    }
    let f = 1.0 - 4.0 * ((p - 0.5).max(0.0) - 0.5).powi(2);
    let f2 = p.powf(1.25);
    let x_deg: f32 = match ctx.facing {
        "up" => 0.0,
        "down" => 180.0,
        _ => 90.0,
    };
    let base = centred(
        Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
            * Quat::from_rotation_x(x_deg.to_radians()),
    );
    let plunger = base * Mat4::from_translation(Vec3::new(0.0, f * -0.75 / 16.0, 0.0));
    let wobble_x = (f2 * 8.0 * (p * PI * 4.0 + anim_offset).sin()).to_radians();
    let wobble_z = (f2 * 8.0 * (p * PI * 4.0 + anim_offset).cos()).to_radians();
    let bell = centred(
        Quat::from_rotation_y(horizontal_angle(ctx.facing).to_radians())
            * Quat::from_rotation_x(x_deg.to_radians()),
    ) * Mat4::from_translation(Vec3::new(0.5, 0.5 - 1.0 / 16.0, 0.5))
        * Mat4::from_rotation_x(wobble_x)
        * Mat4::from_rotation_z(wobble_z)
        * Mat4::from_translation(Vec3::new(-0.5, -0.5 + 1.0 / 16.0, -0.5))
        * centred(Quat::IDENTITY);
    vec![part(DESK_BELL_PLUNGER, plunger), part(DESK_BELL_BELL, bell)]
}

fn item_drain_items(ctx: &ContraptionBlock) -> Vec<WorldItemDraw> {
    let Some(KineticContraptionState::ItemDrain {
        held_item: Some(tis),
        ..
    }) = ctx.contraption
    else {
        return Vec::new();
    };
    if tis.inserted_from < 2 {
        return Vec::new();
    }
    let offset = tis.prev_belt_pos + (tis.belt_pos - tis.prev_belt_pos) * ctx.alpha as f32;
    let side_raw =
        tis.prev_side_offset + (tis.side_offset - tis.prev_side_offset) * ctx.alpha as f32;
    let opp = match tis.inserted_from {
        2 => Vec3::Z,
        3 => Vec3::NEG_Z,
        4 => Vec3::X,
        _ => Vec3::NEG_X,
    };
    let cw_along_x = matches!(tis.inserted_from, 2 | 3);
    let side = if cw_along_x { side_raw } else { -side_raw };
    let positive = if matches!(tis.inserted_from, 3 | 5) {
        1.0f32
    } else {
        -1.0f32
    };
    let vert_angle = (positive * offset * 360.0).to_radians();
    let roll = if cw_along_x {
        Mat4::from_rotation_x(vert_angle)
    } else {
        Mat4::from_rotation_z(-vert_angle)
    };
    let local = Mat4::from_translation(
        Vec3::new(0.5, 15.0 / 16.0, 0.5)
            + opp * (0.5 - offset)
            + Vec3::new(
                if cw_along_x { side } else { 0.0 },
                0.0,
                if cw_along_x { 0.0 } else { side },
            ),
    ) * roll;
    vec![WorldItemDraw {
        stack: tis.stack.clone(),
        local,
        light_offset: [0, 0, 0],
        display: DisplayContext::Fixed,
        style: ItemPoseStyle::DepotOrBelt {
            is_package: is_package_item(&tis.stack),
            angle_deg: 0,
            copies: if tis.stack.count <= 1 {
                1
            } else {
                (tis.stack.count.ilog2() as usize) / 2 + 1
            },
        },
    }]
}

fn track_station_parts_and_items(ctx: &ContraptionBlock) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    let (flag_lv, draws) = match ctx.contraption {
        Some(KineticContraptionState::TrackStation { flag, depot }) => {
            (*flag, depot_item_draws(depot, ctx.alpha, Mat4::IDENTITY))
        }
        _ => (LerpedValue::start_with(0.0), Vec::new()),
    };
    let value = flag_lv.get(ctx.alpha).clamp(0.0, 1.0);
    let mut progress = (value * 5.0).min(1.0).powi(2);
    if flag_lv.target > 0.0 && (flag_lv.value - flag_lv.target).abs() > 1e-4 && progress == 1.0 {
        let wiggle = (value - 0.2) / 0.8;
        progress += ((wiggle * 2.0 * PI * 4.0).sin() / 8.0) / (8.0 * wiggle).max(1.0);
    }
    let flag_model = if ctx.assembling {
        STATION_ASSEMBLE
    } else if value > 0.75 {
        STATION_ON
    } else {
        STATION_OFF
    };
    let nudge = 1.0 / 512.0;
    let pivot = Vec3::new(nudge, 9.5 / 16.0, 2.0 / 16.0 + nudge);
    let y_rot = horizontal_angle(ctx.facing).to_radians();
    let flag_local = centred(Quat::from_rotation_y(y_rot))
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_x((-(progress * 90.0 + 270.0)).to_radians())
        * Mat4::from_translation(-Vec3::splat(0.5))
        * Mat4::from_translation(Vec3::new(0.5 / 16.0, 0.0, 0.0))
        * Mat4::from_rotation_y(PI)
        * Mat4::from_translation(Vec3::new(-0.5 / 16.0, 0.0, 0.0));
    (vec![part(flag_model, flag_local)], draws)
}

fn bogey_parts(ctx: &ContraptionBlock) -> Vec<MachinePart> {
    let base_y = Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_rotation_y(if ctx.axis == "x" { FRAC_PI_2 } else { 0.0 });
    let wheel_deg = angle_radians(ctx.time, ctx.speed, 0.0).to_degrees();
    let wheel_rad = wheel_deg.to_radians();

    let shaft_z_base = centred(axis_y_rotation("z"));
    let mut parts = Vec::new();
    for i in [0.0f32, 1.0f32] {
        let local = base_y
            * Mat4::from_translation(Vec3::new(-0.5, 0.25, -i))
            * centred(Quat::from_rotation_z(wheel_rad))
            * shaft_z_base;
        parts.push(part(SHAFT, local));
    }

    if ctx.block == "create:small_bogey" {
        parts.push(part(
            BOGEY_FRAME,
            base_y * Mat4::from_scale(Vec3::splat(1.0 - 1.0 / 512.0)),
        ));
        for side in [-1.0f32, 1.0f32] {
            let local = base_y
                * Mat4::from_translation(Vec3::new(0.0, 12.0 / 16.0, side))
                * Mat4::from_rotation_x(wheel_rad);
            parts.push(part(SMALL_BOGEY_WHEELS, local));
        }
        return parts;
    }

    let shaft_x_base = centred(axis_y_rotation("x"));
    for i in [0.0f32, 1.0f32] {
        let local = base_y
            * Mat4::from_translation(Vec3::new(-0.5, 0.25, 0.5 + i * -2.0))
            * centred(Quat::from_rotation_x(wheel_rad))
            * shaft_x_base;
        parts.push(part(SHAFT, local));
    }
    let tiny = Mat4::from_scale(Vec3::splat(1.0 - 1.0 / 512.0));
    parts.push(part(BOGEY_DRIVE, base_y * tiny));

    let belt_raw = f64::from(5.0 / 16.0 * wheel_rad);
    let belt_scroll = ((belt_raw - belt_raw.floor()) * 0.5) as f32;
    let mut belt = part(BOGEY_DRIVE_BELT, base_y * tiny);
    belt.sprite_shift = Some(SpriteShift {
        original: "create:block/bogey/belt",
        target: "create:block/bogey/belt_scroll",
        scroll_u: 0.0,
        scroll_v: belt_scroll,
    });
    parts.push(belt);

    parts.push(part(
        BOGEY_PISTON,
        base_y * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.25 * wheel_rad.sin())),
    ));
    parts.push(part(
        LARGE_BOGEY_WHEELS,
        base_y
            * Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
            * Mat4::from_rotation_x(wheel_rad),
    ));
    parts.push(part(
        BOGEY_PIN,
        base_y
            * Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))
            * Mat4::from_rotation_x(wheel_rad)
            * Mat4::from_translation(Vec3::new(0.0, 0.25, 0.0))
            * Mat4::from_rotation_x(-wheel_rad),
    ));
    parts
}

/// Emits partial model parts and world item draws for one Create contraption/door/bell/bogey block.
pub(super) fn contraption_parts_and_items(
    ctx: &ContraptionBlock,
) -> (Vec<MachinePart>, Vec<WorldItemDraw>) {
    match ctx.block {
        "create:schematicannon" => (schematicannon_parts(ctx), Vec::new()),
        "create:mechanical_roller" => (roller_parts(ctx), Vec::new()),
        "create:chain_conveyor" => (chain_conveyor_parts(ctx), Vec::new()),
        "create:peculiar_bell" | "create:haunted_bell" => (bell_parts(ctx), Vec::new()),
        "create:desk_bell" => (desk_bell_parts(ctx), Vec::new()),
        "create:item_drain" => (Vec::new(), item_drain_items(ctx)),
        "create:track_station" => track_station_parts_and_items(ctx),
        "create:small_bogey" | "create:large_bogey" => (bogey_parts(ctx), Vec::new()),
        "create:andesite_door" | "create:copper_door" => (folding_door_parts(ctx), Vec::new()),
        _ => (Vec::new(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn stack(id: &str, count: u32) -> horizon_items::items::ItemStack {
        horizon_items::items::ItemStack {
            item: Arc::from(id),
            count,
            damage: 0,
            extra: None,
        }
    }

    fn base_ctx<'a>(block: &'a str, facing: &'a str) -> ContraptionBlock<'a> {
        ContraptionBlock {
            block,
            facing,
            axis: "z",
            half: "lower",
            hinge: "left",
            attachment: "floor",
            open: false,
            visible: true,
            powered: false,
            assembling: false,
            pos: [0, 0, 0],
            speed: 0.0,
            time: 0.0,
            alpha: 0.0,
            contraption: None,
        }
    }

    #[test]
    fn schematicannon_aims_connector_and_pipe_with_recoil() {
        let mut printer = Compound::new();
        printer.insert(
            JavaString::from("CurrentPos"),
            Tag::IntArray(vec![10, 6, 0]),
        );
        let mut fc = Compound::new();
        fc.insert(JavaString::from("TotalTicks"), Tag::Int(20));
        fc.insert(JavaString::from("TicksLeft"), Tag::Int(18));
        let mut cannon = Compound::new();
        cannon.insert(
            JavaString::from("DefaultYaw"),
            Tag::Float(45.0f32.to_bits()),
        );
        cannon.insert(JavaString::from("Printer"), Tag::Compound(printer));
        cannon.insert(
            JavaString::from("FlyingBlocks"),
            Tag::List(vec![Tag::Compound(fc)]),
        );

        let mut st = contraption_state_from_nbt(None, &Tag::Compound(cannon)).unwrap();
        let (parts, _) = contraption_parts_and_items(&ContraptionBlock {
            contraption: Some(&st),
            ..base_ctx("create:schematicannon", "north")
        });
        assert_eq!(parts[0].model, CANNON_CONNECTOR);
        assert_eq!(parts[1].model, CANNON_PIPE);

        tick_contraption_state(
            &mut st,
            &ContraptionTickCtx {
                open: false,
                visible: true,
                powered: false,
                assembling: false,
            },
        );
        if let KineticContraptionState::Schematicannon { recoil_ticks, .. } = st {
            assert_eq!(recoil_ticks, 8);
        } else {
            panic!("expected schematicannon");
        }
    }

    #[test]
    fn roller_after_pose_keeps_the_drum_on_its_axle_and_the_frame_fixed() {
        let ctx = base_ctx("create:mechanical_roller", "south");
        let (idle, _) = contraption_parts_and_items(&ctx);
        let (roller, _) = contraption_parts_and_items(&ContraptionBlock {
            speed: 96.0,
            time: 11.0,
            ..ctx
        });
        assert_eq!(roller[0].model, ROLLER_WHEEL);
        assert_eq!(roller[1].model, ROLLER_FRAME);
        let wheel = roller[0].local;
        // The model's Z axle becomes X after the 90-degree model alignment.
        let axle_origin = Vec3::new(0.5, 0.5, 0.0);
        assert!(
            wheel
                .transform_point3(axle_origin)
                .abs_diff_eq(Vec3::new(0.0, -0.25, 17.0 / 16.0), 1e-5)
        );
        assert!(wheel.transform_vector3(Vec3::Z).abs_diff_eq(Vec3::X, 1e-5));
        let (sin, cos) = (11.0_f32 * 96.0 / 20.0).to_radians().sin_cos();
        assert!(
            wheel
                .transform_vector3(Vec3::Y)
                .abs_diff_eq(Vec3::new(0.0, cos, -sin), 1e-5)
        );
        assert_eq!(roller[1].local, idle[1].local);
        assert_eq!(roller[1].local.transform_vector3(Vec3::Y), Vec3::Y);
    }

    #[test]
    fn chain_conveyor_after_pose_spins_only_the_vertical_shaft() {
        let mut cc = Compound::new();
        cc.insert(
            JavaString::from("Connections"),
            Tag::List(vec![
                Tag::IntArray(vec![4, 0, 0]),
                Tag::IntArray(vec![0, 0, -5]),
            ]),
        );
        let st = contraption_state_from_nbt(None, &Tag::Compound(cc)).unwrap();
        let (cc_parts, _) = contraption_parts_and_items(&ContraptionBlock {
            speed: 32.0,
            time: 11.0,
            axis: "x", // The fixture's generic axis must not affect the conveyor.
            contraption: Some(&st),
            ..base_ctx("create:chain_conveyor", "north")
        });
        assert_eq!(cc_parts[0].model, CHAIN_SHAFT);
        assert_eq!(cc_parts[1].model, CHAIN_WHEEL);
        assert_eq!(cc_parts[2].model, CHAIN_GUARD);
        assert_eq!(cc_parts[3].model, CHAIN_GUARD);
        assert_eq!(cc_parts[1].local, Mat4::IDENTITY);
        let shaft = cc_parts[0].local;
        let center = Vec3::splat(0.5);
        assert!(shaft.transform_point3(center).abs_diff_eq(center, 1e-6));
        assert_eq!(shaft.transform_vector3(Vec3::Y), Vec3::Y);
        let (sin, cos) = (11.0_f32 * 32.0 * 3.0 / 10.0).to_radians().sin_cos();
        assert!(
            shaft
                .transform_vector3(Vec3::X)
                .abs_diff_eq(Vec3::new(cos, 0.0, -sin), 1e-6)
        );
    }

    #[test]
    fn paired_chain_spans_follow_the_wheel_rims_and_scroll_inside_the_atlas() {
        let state = |x| KineticContraptionState::ChainConveyor {
            connections: vec![[x, 0, 0]],
        };
        let left = state(4);
        let right = state(-4);
        let span = |state, speed, time| {
            chain_spans(&ContraptionBlock {
                contraption: Some(state),
                speed,
                time,
                ..base_ctx("create:chain_conveyor", "north")
            })
            .next()
            .unwrap()
        };
        let forward = span(&left, 32.0, 0.0);
        let returning = span(&right, 32.0, 0.0);
        let reversed = span(&left, -32.0, 0.0);
        let center = Vec3::new(0.5, 6.0 / 16.0, 0.5);
        assert!(((forward.start - center).length() - 1.25).abs() < 1e-6);
        assert!(((forward.end - center - Vec3::X * 4.0).length() - 1.25).abs() < 1e-6);
        assert!(forward.start.z > center.z && returning.start.z < center.z);
        assert!((returning.end + Vec3::X * 4.0).abs_diff_eq(reversed.start, 1e-6));
        assert!((returning.start + Vec3::X * 4.0).abs_diff_eq(reversed.end, 1e-6));

        let sprite = Rect {
            u0: 0.2,
            v0: 0.3,
            u1: 0.4,
            v1: 0.5,
            w: 16,
            h: 16,
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        let lights = [0, 0xf0f0];
        forward.draw(Vec3::ZERO, &sprite, lights, &mut before);
        span(&left, 32.0, 3.0).draw(Vec3::ZERO, &sprite, lights, &mut after);
        assert!(!before.is_empty());
        assert_eq!(before[0].light, lights[1]);
        assert_eq!(before[before.len() - 3].light, lights[0]);
        assert_ne!(before[0].uv, after[0].uv);
        assert!(before.iter().chain(&after).all(|vertex| {
            (sprite.u0..=sprite.u1).contains(&vertex.uv[0])
                && (sprite.v0..=sprite.v1).contains(&vertex.uv[1])
        }));
    }

    #[test]
    fn sliding_and_folding_doors_animate_when_opening_and_hide_when_settled_closed() {
        let mut door = KineticContraptionState::SlidingDoor {
            animation: LerpedValue::start_with(0.0),
            bridge_ticks: 0,
        };
        assert!(
            sliding_door_halves(&ContraptionBlock {
                contraption: Some(&door),
                ..base_ctx("create:train_door", "east")
            })
            .is_none()
        );

        let tick = ContraptionTickCtx {
            open: true,
            visible: false,
            powered: false,
            assembling: false,
        };
        for _ in 0..4 {
            tick_contraption_state(&mut door, &tick);
        }
        for block in [
            "create:train_door",
            "create:brass_door",
            "create:framed_glass_door",
        ] {
            for (hinge, sign) in [("left", -1.0), ("right", 1.0)] {
                let ctx = ContraptionBlock {
                    open: true,
                    visible: false,
                    hinge,
                    alpha: 1.0 / 3.0,
                    contraption: Some(&door),
                    ..base_ctx(block, "east")
                };
                let [(lower, bottom), (upper, top)] = sliding_door_halves(&ctx).unwrap();
                assert_eq!((lower, upper), ("lower", "upper"));
                let expected = Vec3::new(1.0 / 32.0, 0.0, sign * 13.0 / 64.0);
                assert!(bottom.abs_diff_eq(expected, 1e-6));
                assert!((top - bottom).abs_diff_eq(Vec3::Y * (1.0 - 1.0 / 512.0), 1e-6));
                assert!(
                    sliding_door_halves(&ContraptionBlock {
                        half: "upper",
                        ..ctx
                    })
                    .is_none()
                );
            }
        }

        let (folding, _) = contraption_parts_and_items(&ContraptionBlock {
            open: true,
            visible: false,
            contraption: Some(&door),
            ..base_ctx("create:andesite_door", "south")
        });
        assert_eq!(folding[0].model, "create:block/andesite_door/fold_left");
        assert_eq!(folding[1].model, "create:block/andesite_door/fold_right");
        assert!(
            contraption_parts_and_items(&ContraptionBlock {
                half: "upper",
                visible: false,
                contraption: Some(&door),
                ..base_ctx("create:copper_door", "south")
            })
            .0
            .is_empty()
        );
    }

    #[test]
    fn bells_and_desk_bell_animate_ring_cycles() {
        let mut tag = Compound::new();
        tag.insert(
            JavaString::from("Ringing"),
            Tag::String(JavaString::from("east")),
        );
        let mut bell = contraption_state_from_nbt(None, &Tag::Compound(tag)).unwrap();
        let rest_bell = contraption_parts_and_items(&base_ctx("create:peculiar_bell", "north")).0;
        for _ in 0..4 {
            tick_contraption_state(
                &mut bell,
                &ContraptionTickCtx {
                    open: false,
                    visible: true,
                    powered: false,
                    assembling: false,
                },
            );
        }
        let swinging = contraption_parts_and_items(&ContraptionBlock {
            contraption: Some(&bell),
            ..base_ctx("create:haunted_bell", "north")
        })
        .0;
        assert_eq!(swinging[0].model, HAUNTED_BELL);
        assert!(
            (swinging[0].local.transform_point3(Vec3::Y)
                - rest_bell[0].local.transform_point3(Vec3::Y))
            .length()
                > 1e-3
        );

        let mut desk = KineticContraptionState::DeskBell {
            animation: LerpedValue::start_with(0.0),
            animation_offset: 0.0,
        };
        tick_contraption_state(
            &mut desk,
            &ContraptionTickCtx {
                open: false,
                visible: true,
                powered: true,
                assembling: false,
            },
        );
        let (desk_parts, _) = contraption_parts_and_items(&ContraptionBlock {
            powered: true,
            contraption: Some(&desk),
            ..base_ctx("create:desk_bell", "up")
        });
        assert_eq!(desk_parts[0].model, DESK_BELL_PLUNGER);
        assert_eq!(desk_parts[1].model, DESK_BELL_BELL);
    }

    #[test]
    fn desk_bell_ding_restarts_a_pulse_in_progress() {
        let mut fields = Compound::new();
        fields.insert(JavaString::from("Ding"), Tag::Byte(1));
        let tag = Tag::Compound(fields);
        let playing = KineticContraptionState::DeskBell {
            animation: LerpedValue::start_with(0.4),
            animation_offset: 0.25,
        };
        let Some(KineticContraptionState::DeskBell {
            animation,
            animation_offset,
        }) = contraption_state_from_nbt(Some(&playing), &tag)
        else {
            panic!("desk bell update did not restart the animation");
        };
        assert_eq!(animation.value, 1.0);
        assert_eq!(animation_offset, 0.25);
    }

    #[test]
    fn item_drain_and_track_station_emit_items_and_flag() {
        let mut drain = KineticContraptionState::ItemDrain {
            held_item: Some(TransportedItem {
                stack: stack("minecraft:water_bucket", 1),
                belt_pos: 0.25,
                prev_belt_pos: 0.25,
                side_offset: 0.0,
                prev_side_offset: 0.0,
                angle: 0,
                inserted_from: 2,
                locked: false,
                locked_externally: false,
            }),
            processing_ticks: 0,
        };
        tick_contraption_state(
            &mut drain,
            &ContraptionTickCtx {
                open: false,
                visible: true,
                powered: false,
                assembling: false,
            },
        );
        let (_, drain_draws) = contraption_parts_and_items(&ContraptionBlock {
            contraption: Some(&drain),
            ..base_ctx("create:item_drain", "north")
        });
        assert_eq!(drain_draws.len(), 1);
        assert_eq!(drain_draws[0].stack.item.as_ref(), "minecraft:water_bucket");

        let mut st = KineticContraptionState::TrackStation {
            flag: LerpedValue::start_with(0.0),
            depot: Default::default(),
        };
        let (off_parts, _) = contraption_parts_and_items(&ContraptionBlock {
            contraption: Some(&st),
            ..base_ctx("create:track_station", "north")
        });
        assert_eq!(off_parts[0].model, STATION_OFF);

        for _ in 0..25 {
            tick_contraption_state(
                &mut st,
                &ContraptionTickCtx {
                    open: false,
                    visible: true,
                    powered: false,
                    assembling: true,
                },
            );
        }
        let (asm_parts, _) = contraption_parts_and_items(&ContraptionBlock {
            assembling: true,
            contraption: Some(&st),
            ..base_ctx("create:track_station", "north")
        });
        assert_eq!(asm_parts[0].model, STATION_ASSEMBLE);
    }

    #[test]
    fn small_and_large_bogeys_emit_wheels_pistons_and_scrolling_drive_belt() {
        let (small, _) = contraption_parts_and_items(&ContraptionBlock {
            axis: "z",
            speed: 32.0,
            time: 5.0,
            ..base_ctx("create:small_bogey", "north")
        });
        assert_eq!(small.len(), 5);
        assert_eq!(small[2].model, BOGEY_FRAME);
        assert_eq!(small[3].model, SMALL_BOGEY_WHEELS);

        let (large, _) = contraption_parts_and_items(&ContraptionBlock {
            axis: "x",
            speed: 32.0,
            time: 5.0,
            ..base_ctx("create:large_bogey", "north")
        });
        assert_eq!(large.len(), 9);
        assert_eq!(large[5].model, BOGEY_DRIVE_BELT);
        assert_eq!(
            large[5].sprite_shift.unwrap().target,
            "create:block/bogey/belt_scroll"
        );
        assert_eq!(large[6].model, BOGEY_PISTON);
        assert_eq!(large[7].model, LARGE_BOGEY_WHEELS);
        assert_eq!(large[8].model, BOGEY_PIN);
        let idle = bogey_parts(&ContraptionBlock {
            axis: "x",
            ..base_ctx("create:large_bogey", "north")
        });
        assert_eq!(large[4].local, idle[4].local);
        assert_eq!(
            large[4].local.transform_point3(Vec3::ZERO),
            Vec3::splat(0.5)
        );
        assert_ne!(large[7].local, idle[7].local);
    }

    #[test]
    fn bogey_after_pose_keeps_frames_upright_and_wheels_on_their_axles() {
        let (sin, cos) = (11.0_f32 * 32.0 * 3.0 / 10.0).to_radians().sin_cos();
        for (axis, axle, forward) in [("x", Vec3::NEG_Z, Vec3::X), ("z", Vec3::X, Vec3::Z)] {
            for (block, frame, wheels, height) in [
                (
                    "create:small_bogey",
                    BOGEY_FRAME,
                    SMALL_BOGEY_WHEELS,
                    12.0 / 16.0,
                ),
                ("create:large_bogey", BOGEY_DRIVE, LARGE_BOGEY_WHEELS, 1.0),
            ] {
                let ctx = ContraptionBlock {
                    axis,
                    ..base_ctx(block, "south")
                };
                let idle = bogey_parts(&ctx);
                let active = bogey_parts(&ContraptionBlock {
                    speed: 32.0,
                    time: 11.0,
                    ..ctx
                });
                let frame_pose = active
                    .iter()
                    .find(|part| part.model == frame)
                    .unwrap()
                    .local;
                let idle_frame = idle.iter().find(|part| part.model == frame).unwrap().local;
                assert_eq!(frame_pose, idle_frame);
                let center = Vec3::splat(0.5);
                assert_eq!(frame_pose.transform_point3(Vec3::ZERO), center);
                assert_eq!(
                    frame_pose.transform_vector3(Vec3::Y),
                    Vec3::Y * (1.0 - 1.0 / 512.0)
                );
                let offsets: &[f32] = if wheels == SMALL_BOGEY_WHEELS {
                    &[-1.0, 1.0]
                } else {
                    &[0.0]
                };
                let poses: Vec<_> = active.iter().filter(|part| part.model == wheels).collect();
                assert_eq!(poses.len(), offsets.len());
                for (wheel, &offset) in poses.into_iter().zip(offsets) {
                    assert!(
                        wheel
                            .local
                            .transform_point3(Vec3::ZERO)
                            .abs_diff_eq(center + Vec3::Y * height + forward * offset, 1e-6)
                    );
                    assert!(
                        wheel
                            .local
                            .transform_vector3(Vec3::X)
                            .abs_diff_eq(axle, 1e-6)
                    );
                    assert!(
                        wheel
                            .local
                            .transform_vector3(Vec3::Y)
                            .abs_diff_eq(Vec3::Y * cos + forward * sin, 1e-6)
                    );
                }
            }
        }
    }
}
