//! Create and addon kinetic animation engine.
//!
//! Block entities report their `Speed` (rpm) in network update tags. Dynamic parts
//! are drawn turned, translated or oscillated by the game, while static casings
//! stay in the chunk terrain mesh.
use super::Game;
use horizon_client::mesher::Vertex;
use horizon_client::world::MachineState;
use horizon_world::nbt::{JavaString, Tag};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub mod addons;
mod aeronautics;
mod arm;
mod contraptions;
mod create_piston;
mod deployer;
mod extra_addons;
mod flap_display;
pub(super) mod fluids;
mod indicators;
mod instruments;
mod levers;
mod logistics;
mod machines;
#[cfg(test)]
mod scene_test;
pub mod specs;

pub use addons::*;
pub(super) use extra_addons::CANNON_MOUNT;
pub use specs::*;

pub(super) const KINETIC_BLOCK_ENTITY: &str = "create:simple_kinetic";

/// Degrees per tick per rpm (`getAngleForBe`: `time * speed * 3 / 10`).
const DEGREES_PER_TICK_PER_RPM: f64 = 3.0 / 10.0;
/// `KineticBlockEntityVisual.rotationOffset` for small cogwheels (8 teeth).
const HALF_TOOTH_OFFSET: f64 = 22.5;
/// Half-tooth offset for large cogwheels (16 teeth).
const LARGE_TOOTH_OFFSET: f64 = 11.25;

/// `Speed` of a kinetic block entity's tag: a missing field in a valid tag is stopped (0.0).
pub(super) fn speed_from_nbt(tag: &Tag) -> Option<f32> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    if fields.is_empty() {
        return None;
    }
    Some(match fields.get(&JavaString::from("Speed")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        Some(Tag::Double(bits)) => f64::from_bits(*bits) as f32,
        _ => 0.0,
    })
}

/// The angle in degrees a bearing's top keeps while it does not turn: its
/// `Angle` when it is not `Running` or stands still
/// (`MechanicalBearingBlockEntity.getInterpolatedAngle`). None for a turning
/// bearing and for a block entity that is no bearing.
pub(super) fn held_angle_from_nbt(tag: &Tag, speed: f32) -> Option<f32> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let running = match fields.get(&JavaString::from("Running"))? {
        Tag::Byte(value) => *value != 0,
        _ => return None,
    };
    if running && speed != 0.0 {
        return None;
    }
    Some(match fields.get(&JavaString::from("Angle")) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    })
}

/// A gauge's `Value`: where its dial points, 0 at rest to 1.125 overstressed.
pub(super) fn dial_from_nbt(tag: &Tag) -> Option<f32> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    match fields.get(&JavaString::from("Value"))? {
        Tag::Float(bits) => Some(f32::from_bits(*bits)),
        _ => None,
    }
}

/// A pulley's, gearbox's or arm's `Offset`, `Running`, `Goggles` and `Source`
/// (`LinearActuatorBlockEntity`, `HosePulleyBlockEntity`, `ArmBlockEntity`,
/// `KineticBlockEntity`). A hose pulley keeps its offset in a `LerpedFloat`
/// compound whose `Value` is the offset. None for a tag with none of these.
pub(super) fn machine_state_from_nbt(tag: &Tag) -> Option<MachineState> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let offset = match fields.get(&JavaString::from("Offset")) {
        Some(Tag::Float(bits)) => Some(f32::from_bits(*bits)),
        Some(Tag::Compound(lerped)) => match lerped.get(&JavaString::from("Value")) {
            Some(Tag::Float(bits)) => Some(f32::from_bits(*bits)),
            _ => None,
        },
        _ => None,
    };
    let flag = |name: &str| match fields.get(&JavaString::from(name)) {
        Some(Tag::Byte(value)) => Some(*value != 0),
        _ => None,
    };
    let goggles = flag("Goggles");
    let source = fields
        .get(&JavaString::from("Source"))
        .and_then(arm::read_block_pos);
    if offset.is_none() && goggles.is_none() && source.is_none() {
        return None;
    }
    Some(MachineState {
        offset: offset.unwrap_or(0.0),
        running: flag("Running").unwrap_or(false),
        goggles: goggles.unwrap_or(false),
        source,
    })
}

/// Lines logged for Create arms in a run: what a server sends of an arm's
/// `Phase`, target and progress, before the arm's pose reads them.
const ARM_LOG_LINES: u32 = 60;

/// `Horizon: arm nbt` for a mechanical arm's tag, when it differs from that
/// arm's last one by more than the movement's progress.
fn log_arm_nbt(pos: (i32, i32, i32), tag: &Tag) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LAST: Mutex<Option<HashMap<(i32, i32, i32), String>>> = Mutex::new(None);
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if LOGGED.load(Ordering::Relaxed) >= ARM_LOG_LINES {
        return;
    }
    let (Some(key), Some(line)) = (describe_arm_nbt(tag, false), describe_arm_nbt(tag, true))
    else {
        return;
    };
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let last = last.get_or_insert_with(HashMap::new);
    if last.get(&pos) == Some(&key) {
        return;
    }
    last.insert(pos, key);
    LOGGED.fetch_add(1, Ordering::Relaxed);
    log::info!("Horizon: arm nbt {pos:?} {line}");
}

/// An arm's own fields (`ArmBlockEntity.write`): scalars by value, lists and
/// compounds by size and keys. None for a tag that is not an arm's.
fn describe_arm_nbt(tag: &Tag, with_progress: bool) -> Option<String> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let has = |key: &str| fields.contains_key(&JavaString::from(key));
    if !has("Phase") && !has("InteractionPoints") {
        return None;
    }
    let parts: Vec<String> = [
        "Phase",
        "TargetPointIndex",
        "MovementProgress",
        "InteractionPoints",
        "HeldItem",
        "Goggles",
        "Powered",
    ]
    .iter()
    .filter(|key| with_progress || **key != "MovementProgress")
    .filter_map(|key| {
        fields
            .get(&JavaString::from(*key))
            .map(|value| format!("{key}={}", short_tag(value)))
    })
    .collect();
    Some(parts.join(" "))
}

fn short_tag(tag: &Tag) -> String {
    let keys = |map: &horizon_world::nbt::Compound| {
        map.keys()
            .map(JavaString::to_string_lossy)
            .collect::<Vec<_>>()
            .join(",")
    };
    match tag {
        Tag::Byte(v) => v.to_string(),
        Tag::Short(v) => v.to_string(),
        Tag::Int(v) => v.to_string(),
        Tag::Long(v) => v.to_string(),
        Tag::Float(bits) => format!("{:.3}", f32::from_bits(*bits)),
        Tag::Double(bits) => format!("{:.3}", f64::from_bits(*bits)),
        Tag::String(text) => text.to_string_lossy(),
        Tag::List(items) => match items.first() {
            Some(Tag::Compound(first)) => format!("list{}{{{}}}", items.len(), keys(first)),
            _ => format!("list{}", items.len()),
        },
        Tag::Compound(map) => match map.get(&JavaString::from("id")) {
            Some(Tag::String(id)) => format!("{{id={} {}}}", id.to_string_lossy(), keys(map)),
            _ => format!("{{{}}}", keys(map)),
        },
        other => format!("tag{}", other.id()),
    }
}

/// `EncasedFanRenderer`'s propeller speed: five times the shaft's, at least
/// 80 rpm either way while it turns.
pub(super) fn fan_speed(speed: f32) -> f32 {
    let fast = speed * 5.0;
    if fast > 0.0 {
        fast.clamp(80.0, 64.0 * 20.0)
    } else if fast < 0.0 {
        fast.clamp(-64.0 * 20.0, -80.0)
    } else {
        0.0
    }
}

/// Create turns every part about the positive direction of its axis
/// (`kineticRotationTransform`). A part here spins about its model axis turned
/// by `base`; facing north, west or down that points the negative way, and
/// the angle must flip to turn the same way.
pub(super) fn spin_sign(base: glam::Quat, model_axis: glam::Vec3) -> f32 {
    let axis = base * model_axis;
    if axis.x + axis.y + axis.z < 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// Whether the tag contains a readable kinetic `Speed` field.
pub(super) fn has_kinetic_speed(tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    fields.contains_key(&JavaString::from("Speed"))
}

/// Whether `name` is a Create mechanical arm block entity carrying arm animation fields.
pub(super) fn is_arm_block_entity_tag(name: &str, tag: &Tag) -> bool {
    name == "create:mechanical_arm" && arm::is_arm_tag(tag)
}

pub(super) fn is_deployer_be_tag(name: &str, tag: &Tag) -> bool {
    deployer::is_deployer_be_tag(name, tag)
}

pub(super) fn is_piston_be_tag(name: &str, tag: &Tag) -> bool {
    create_piston::is_piston_be_tag(name, tag)
}

pub(super) fn is_create_printer_tag(name: &str, tag: &Tag) -> bool {
    name == extra_addons::PRINTER && extra_addons::printer_ticks_from_nbt(tag).is_some()
}

/// Whether the tag carries auxiliary Create animation state, including Aeronautics poses.
pub(super) fn is_create_aux_block_entity_tag(name: &str, tag: &Tag) -> bool {
    levers::is_aux_be_tag(name, tag) || aeronautics::has_pose_tag(name, tag)
}

/// Whether `name` is a Create belt, tunnel, blaze burner, depot, chute, basin or frogport BE tag.
pub(super) fn is_create_logistics_block_entity_tag(name: &str, tag: &Tag) -> bool {
    logistics::is_logistics_be_tag(name, tag)
}

/// Whether `name` is a Create schematicannon, roller, chain conveyor, door, bell, drain, station or bogey BE tag.
pub(super) fn is_create_contraption_block_entity_tag(name: &str, tag: &Tag) -> bool {
    contraptions::is_contraption_be_tag(name, tag)
}

/// Checks whether a block entity identifier belongs to Create or one of its kinetic addons.
pub(super) fn is_kinetic_block_entity(name: &str) -> bool {
    name == KINETIC_BLOCK_ENTITY
        || name == extra_addons::ORE_KINETIC_INPUT
        || name.starts_with("create:")
        || name.starts_with("createaddition:")
        || name.starts_with("create_enchantment_industry:")
        || name.starts_with("create_mechanical_extruder:")
        || name.starts_with("create_new_age:")
        || aeronautics::is_kinetic_be(name)
}

/// `KineticBlockEntityVisual.shouldOffset`: the coordinates off the axis sum
/// to an even number (Java's `% 2 == 0`, so negative odd sums are not).
pub(super) fn should_offset(axis: usize, pos: [i32; 3]) -> bool {
    let sum: i64 = (0..3)
        .filter(|&i| i != axis)
        .map(|i| i64::from(pos[i]))
        .sum();
    sum % 2 == 0
}

/// `getRotationOffsetForPosition` for small cogwheels.
pub(super) fn rotation_offset(axis: usize, pos: [i32; 3]) -> f64 {
    if should_offset(axis, pos) {
        HALF_TOOTH_OFFSET
    } else {
        0.0
    }
}

/// Tooth offset for large cogwheels to mesh with small cogs without clipping.
pub(super) fn large_cog_rotation_offset(axis: usize, pos: [i32; 3]) -> f64 {
    if should_offset(axis, pos) {
        LARGE_TOOTH_OFFSET
    } else {
        0.0
    }
}

/// `getAngleForBe`, in radians: `((time * speed * 3 / 10 + offset) % 360)`.
pub(super) fn angle_radians(time: f64, speed: f32, offset: f64) -> f32 {
    let degrees = (time * f64::from(speed) * DEGREES_PER_TICK_PER_RPM + offset) % 360.0;
    degrees.to_radians() as f32
}

pub(super) fn axis_index(name: &str) -> Option<usize> {
    match name {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    }
}

/// Converts a blockstate facing property to a base rotation quaternion for native +Y models.
pub(super) fn facing_y_rotation(facing: &str) -> glam::Quat {
    use std::f32::consts::PI;
    match facing {
        "up" => glam::Quat::IDENTITY,
        "down" => glam::Quat::from_rotation_x(PI),
        "north" => glam::Quat::from_rotation_x(-PI / 2.0),
        "south" => glam::Quat::from_rotation_x(PI / 2.0),
        "east" => glam::Quat::from_rotation_z(-PI / 2.0),
        "west" => glam::Quat::from_rotation_z(PI / 2.0),
        _ => glam::Quat::IDENTITY,
    }
}

/// Converts a blockstate facing property to a base rotation quaternion for native +Z models.
pub(super) fn facing_z_rotation(facing: &str) -> glam::Quat {
    use std::f32::consts::PI;
    match facing {
        "south" => glam::Quat::IDENTITY,
        "north" => glam::Quat::from_rotation_y(PI),
        "east" => glam::Quat::from_rotation_y(PI / 2.0),
        "west" => glam::Quat::from_rotation_y(-PI / 2.0),
        "up" => glam::Quat::from_rotation_x(-PI / 2.0),
        "down" => glam::Quat::from_rotation_x(PI / 2.0),
        _ => glam::Quat::IDENTITY,
    }
}

/// Converts a blockstate axis property (`"x"`, `"y"`, `"z"`) to a base rotation quaternion for native +Y models.
pub(super) fn axis_y_rotation(axis: &str) -> glam::Quat {
    use std::f32::consts::PI;
    match axis {
        "x" => glam::Quat::from_rotation_z(-PI / 2.0),
        "z" => glam::Quat::from_rotation_x(PI / 2.0),
        _ => glam::Quat::IDENTITY,
    }
}

/// Converts `"face"` (`"floor"`/`"ceiling"`/`"wall"`) and `"facing"` to a base rotation quaternion for native +Y models.
pub(super) fn attach_face_y_rotation(face: &str, facing: &str) -> glam::Quat {
    let (rx_deg, ry_deg): (f32, f32) = match face {
        "ceiling" => (
            180.0,
            match facing {
                "west" => 90.0,
                "north" => 180.0,
                "east" => 270.0,
                _ => 0.0,
            },
        ),
        "wall" => (
            90.0,
            match facing {
                "east" => 90.0,
                "south" => 180.0,
                "west" => 270.0,
                _ => 0.0,
            },
        ),
        _ => (
            0.0,
            match facing {
                "east" => 90.0,
                "south" => 180.0,
                "west" => 270.0,
                _ => 0.0,
            },
        ),
    };
    (glam::Quat::from_rotation_y(-ry_deg.to_radians())
        * glam::Quat::from_rotation_x(-rx_deg.to_radians()))
    .normalize()
}

pub(super) fn opposite_facing(facing: &str) -> &str {
    match facing {
        "up" => "down",
        "down" => "up",
        "north" => "south",
        "south" => "north",
        "east" => "west",
        "west" => "east",
        other => other,
    }
}

/// Checks whether a block has an animation specification.
pub fn spec_for_block(name: &str) -> Option<&'static AnimBlockSpec> {
    specs::core_spec(name)
        .or_else(|| addons::addon_spec(name))
        .or_else(|| aeronautics::Part::for_block(name).map(|_| &specs::MACHINE_SPEC))
}

/// Whether the named block is handled by the kinetic animation engine.
pub fn is_kinetic_block(name: &str) -> bool {
    spec_for_block(name).is_some()
}

// ---------------------------------------------------------------------------
// Partial model vertex template caching
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TemplateQuad {
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub sprite: String,
    pub local_uv: [[f32; 2]; 4],
    pub orig_uv_size: [f32; 2],
    pub shade: u32,
    pub emission: u8,
}

#[derive(Clone, Debug)]
pub struct BakedTemplate {
    pub quads: Arc<[TemplateQuad]>,
}

struct TemplateCache {
    library: std::sync::Weak<horizon_assets::resolve::ModelLibrary>,
    atlas_generation: u64,
    templates: HashMap<&'static str, Arc<BakedTemplate>>,
}

/// Tests that bake through the process-wide template cache hold this, so one
/// test's library or atlas generation never clears another's templates.
#[cfg(test)]
pub(super) static TEMPLATE_CACHE_TESTS: Mutex<()> = Mutex::new(());

fn get_or_bake_template(
    library: &Arc<horizon_assets::resolve::ModelLibrary>,
    sprites: &HashMap<String, horizon_client::hud::Rect>,
    atlas_generation: u64,
    model_id: &'static str,
) -> Option<Arc<BakedTemplate>> {
    static CACHE: Mutex<Option<TemplateCache>> = Mutex::new(None);
    let lib_weak = Arc::downgrade(library);

    let mut cache_guard = CACHE.lock().ok()?;
    let cache = cache_guard.get_or_insert_with(|| TemplateCache {
        library: lib_weak.clone(),
        atlas_generation,
        templates: HashMap::new(),
    });
    if !cache.library.ptr_eq(&lib_weak) || cache.atlas_generation != atlas_generation {
        cache.library = lib_weak;
        cache.atlas_generation = atlas_generation;
        cache.templates.clear();
    }
    if let Some(existing) = cache.templates.get(model_id) {
        return Some(existing.clone());
    }

    let resolved = library.resolve(model_id).ok()?;
    let quads = horizon_assets::bake::bake_block_model(
        &resolved,
        horizon_assets::bake::BlockModelTransform::new([0; 3], false),
    )
    .ok()?;

    let mut template_quads = Vec::with_capacity(quads.len());
    let mut skipped = 0usize;
    for quad in &quads {
        let sprite_name = quad.material.sprite.as_str();
        let key = sprite_name
            .strip_prefix("minecraft:")
            .unwrap_or(sprite_name);
        let (matched_sprite, r) = if let Some(r) = sprites.get(key) {
            (key, r)
        } else if let Some(r) = sprites.get(sprite_name) {
            (sprite_name, r)
        } else {
            skipped += 1;
            continue;
        };
        let (u0, v0, u1, v1) = (r.u0, r.v0, r.u1, r.v1);
        let orig_uv_size = [u1 - u0, v1 - v0];

        let mut uv = [[0.0f32; 2]; 4];
        for k in 0..4 {
            uv[k] = [
                u0 + quad.uv[k][0] * orig_uv_size[0],
                v0 + quad.uv[k][1] * orig_uv_size[1],
            ];
        }

        use horizon_assets::model::Direction as D;
        let shade: u32 = match quad.facing {
            D::Up => 255,
            D::Down => 127,
            D::North | D::South => 204,
            D::West | D::East => 153,
        };

        template_quads.push(TemplateQuad {
            positions: quad.positions,
            uv,
            sprite: matched_sprite.to_owned(),
            local_uv: quad.uv,
            orig_uv_size,
            shade,
            emission: quad.light_emission,
        });
    }
    if skipped > 0 {
        log::warn!(
            "Horizon: kinetic template {model_id} skipped {skipped} quad(s) with missing sprites"
        );
    }

    let baked = Arc::new(BakedTemplate {
        quads: template_quads.into(),
    });
    cache.templates.insert(model_id, baked.clone());
    Some(baked)
}

fn emit_template_quads(
    template: &BakedTemplate,
    sprites: &HashMap<String, horizon_client::hud::Rect>,
    matrix: glam::Mat4,
    light_val: u32,
    tint: Option<u32>,
    sprite_shift: Option<machines::SpriteShift>,
    full_bright: bool,
    out: &mut Vec<Vertex>,
) {
    let full_light = horizon_client::block_entity_render::vertex_light(0xFF);
    for q in template.quads.iter() {
        let color = match tint {
            Some(rgb) => {
                let r = (((rgb >> 16) & 0xFF) * q.shade) / 255;
                let g = (((rgb >> 8) & 0xFF) * q.shade) / 255;
                let b = ((rgb & 0xFF) * q.shade) / 255;
                0xFF00_0000 | (b << 16) | (g << 8) | r
            }
            None => 0xFF00_0000 | (q.shade << 16) | (q.shade << 8) | q.shade,
        };
        let light = if full_bright {
            full_light
        } else if q.emission > 0 {
            let em = u32::from(q.emission) * 16;
            (light_val & 0xFF) | ((light_val >> 8 & 0xFF).max(em) << 8)
        } else {
            light_val
        };
        let shifted = sprite_shift
            .filter(|shift| q.sprite == shift.original)
            .and_then(|shift| {
                let key = shift
                    .target
                    .strip_prefix("minecraft:")
                    .unwrap_or(shift.target);
                sprites
                    .get(key)
                    .or_else(|| sprites.get(shift.target))
                    .map(|tgt| (shift, tgt))
            });
        for k in 0..4 {
            let p = glam::Vec3::from_array(q.positions[k]);
            let world_pos = matrix.transform_point3(p);
            let uv = match shifted {
                Some((shift, tgt)) => {
                    let tgt_w = tgt.u1 - tgt.u0;
                    let tgt_h = tgt.v1 - tgt.v0;
                    [
                        tgt.u0 + q.local_uv[k][0] * q.orig_uv_size[0] + shift.scroll_u * tgt_w,
                        tgt.v0 + q.local_uv[k][1] * q.orig_uv_size[1] + shift.scroll_v * tgt_h,
                    ]
                }
                None => q.uv[k],
            };
            out.push(Vertex {
                pos: world_pos.to_array(),
                uv,
                layer: 0,
                color,
                light,
                normal: [0, 0, 0, 0],
                glint_uv: [0.0, 0.0],
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Game integration
// ---------------------------------------------------------------------------

impl Game<'_> {
    pub(super) fn apply_create_flap(&mut self, event: horizon_client::net::CreateFlap) {
        use horizon_client::net::CreateFlap;
        use horizon_client::world::{KineticAuxState, KineticLogisticsState};

        match event {
            CreateFlap::Tunnel {
                pos: [x, y, z],
                sides,
            } => {
                let name = &self.res.facts().get(self.world.block(x, y, z)).name;
                if matches!(
                    name.as_ref(),
                    "create:andesite_tunnel" | "create:brass_tunnel"
                ) && let Some(KineticLogisticsState::BeltTunnel { flaps }) =
                    self.world.kinetic_logistics.get_mut(&(x, y, z))
                {
                    for (side, flap) in flaps {
                        if let Some(inward) = sides.get(usize::from(*side)).copied().flatten() {
                            logistics::kick_flap(flap, inward);
                        }
                    }
                }
            }
            CreateFlap::Funnel {
                pos: [x, y, z],
                inward,
            } => {
                let name = &self.res.facts().get(self.world.block(x, y, z)).name;
                if !levers::is_funnel(name) {
                    return;
                }
                let state = self.world.kinetic_aux.entry((x, y, z)).or_insert_with(|| {
                    KineticAuxState::Funnel {
                        flap: logistics::new_flap(),
                    }
                });
                if !matches!(state, KineticAuxState::Funnel { .. }) {
                    *state = KineticAuxState::Funnel {
                        flap: logistics::new_flap(),
                    };
                }
                if let KineticAuxState::Funnel { flap } = state {
                    logistics::kick_flap(flap, inward);
                }
            }
        }
    }

    /// Stores the speed of a kinetic block entity at `pos`.
    pub(super) fn store_kinetic_speed(&mut self, pos: (i32, i32, i32), tag: &Tag) {
        log_arm_nbt(pos, tag);
        let Some(speed) = speed_from_nbt(tag) else {
            return;
        };
        if has_kinetic_speed(tag) || !self.world.kinetic_speeds.contains_key(&pos) {
            self.world.kinetic_speeds.insert(pos, speed);
        }
        let block = self.world.block(pos.0, pos.1, pos.2);
        let block = self.res.facts().get(block).name.as_ref();
        if let Some(part) = aeronautics::Part::for_block(block)
            && let Some(pose) = part.read_pose(
                self.world.aeronautics_part_poses.get(&pos).copied(),
                tag,
                self.world.game_time,
                speed,
            )
        {
            self.world.aeronautics_part_poses.insert(pos, pose);
        }
        if block == extra_addons::PRINTER
            && let Some(ticks) = extra_addons::printer_ticks_from_nbt(tag)
        {
            self.world.create_printer_ticks.insert(pos, ticks);
        }
        if deployer::is_deployer_be_tag(block, tag) {
            let state = self.world.create_deployers.entry(pos).or_default();
            deployer::apply_deployer_tag(state, tag);
        }
        if create_piston::is_piston_block(block)
            && let Some(state) =
                create_piston::piston_state_from_nbt(self.world.create_pistons.get(&pos), tag)
        {
            self.world.create_pistons.insert(pos, state);
        }
        match held_angle_from_nbt(tag, speed) {
            Some(angle) => self.world.kinetic_held_angles.insert(pos, angle),
            None => self.world.kinetic_held_angles.remove(&pos),
        };
        match dial_from_nbt(tag) {
            Some(value) => self.world.kinetic_dials.insert(pos, value),
            None => self.world.kinetic_dials.remove(&pos),
        };
        match machine_state_from_nbt(tag) {
            Some(state) => self.world.kinetic_machines.insert(pos, state),
            None => self.world.kinetic_machines.remove(&pos),
        };
        if arm::is_arm_tag(tag) {
            let ceiling = self
                .pack
                .assets
                .registry
                .state(self.world.block(pos.0, pos.1, pos.2))
                .and_then(|s| s.properties.get("ceiling"))
                .is_some_and(|v| v == "true");
            let existing = self.world.kinetic_arms.get(&pos).cloned();
            if let Some(arm_state) = arm::apply_arm_nbt(
                existing.as_ref(),
                [pos.0, pos.1, pos.2],
                ceiling,
                tag,
                |p| {
                    let st = self.world.block(p[0], p[1], p[2]);
                    self.pack
                        .assets
                        .registry
                        .state(st)
                        .and_then(|s| {
                            s.properties
                                .get("facing")
                                .or_else(|| s.properties.get("horizontal_facing"))
                        })
                        .cloned()
                },
            ) {
                self.world.kinetic_arms.insert(pos, arm_state);
            }
        }
        if let Some(aux_state) = levers::aux_state_from_nbt(self.world.kinetic_aux.get(&pos), tag) {
            self.world.kinetic_aux.insert(pos, aux_state);
        }
        if let Some(log_state) =
            logistics::logistics_state_from_nbt(self.world.kinetic_logistics.get(&pos), tag)
        {
            self.world.kinetic_logistics.insert(pos, log_state);
        }
        if let Some(con_state) =
            contraptions::contraption_state_from_nbt(self.world.kinetic_contraptions.get(&pos), tag)
        {
            self.world.kinetic_contraptions.insert(pos, con_state);
        }
    }

    /// Advances each Create schematicannon, door, bell, drain and track station animation state.
    pub(super) fn tick_create_deployers(&mut self) {
        for (pos, state) in &mut self.world.create_deployers {
            let speed = self.world.kinetic_speeds.get(pos).copied().unwrap_or(0.0);
            deployer::tick_deployer_state(state, speed);
        }
    }

    pub(super) fn apply_create_visual_fixture(&mut self, [x, y, z]: [i32; 3], tag: &Tag) -> bool {
        let pos = (x, y, z);
        let state = self.world.block(x, y, z);
        let name = self.res.facts().get(state).name.clone();
        if fluids::is_container(&name) {
            self.apply_create_fluid_tag(pos, &name, tag)
        } else if indicators::is_nixie_block(&name) {
            self.apply_create_indicator_tag(pos, "create:nixie_tube", tag);
            true
        } else if name.as_ref() == "create:track_signal" {
            self.apply_create_indicator_tag(pos, &name, tag);
            true
        } else if name.as_ref() == "create:display_board" {
            self.apply_create_indicator_tag(pos, "create:flap_display", tag);
            self.store_kinetic_speed(pos, tag);
            true
        } else if name.as_ref() == CANNON_MOUNT {
            self.apply_create_cannon_mount_tag(pos, tag)
        } else if is_create_printer_tag(&name, tag)
            || name.as_ref() == extra_addons::ORE_KINETIC_INPUT
        {
            self.store_kinetic_speed(pos, tag);
            true
        } else if deployer::is_deployer_be_tag(&name, tag) || create_piston::is_piston_block(&name)
        {
            self.store_kinetic_speed(pos, tag);
            true
        } else {
            false
        }
    }

    pub(super) fn apply_create_cannon_mount_tag(
        &mut self,
        pos: (i32, i32, i32),
        tag: &Tag,
    ) -> bool {
        let state = self.world.block(pos.0, pos.1, pos.2);
        if self.res.facts().get(state).name.as_ref() != CANNON_MOUNT {
            return false;
        }
        let Some(data) =
            extra_addons::cannon_mount_from_nbt(self.world.create_cannon_mounts.get(&pos), tag)
        else {
            return false;
        };
        self.world.create_cannon_mounts.insert(pos, data);
        true
    }

    pub(super) fn apply_create_indicator_tag(
        &mut self,
        pos: (i32, i32, i32),
        kind: &str,
        tag: &Tag,
    ) {
        if !matches!(tag, Tag::Compound(fields) if !fields.is_empty()) {
            return;
        }
        let state = self.world.block(pos.0, pos.1, pos.2);
        let block = self.res.facts().get(state).name.as_ref();
        if kind == "create:nixie_tube" && indicators::is_nixie_block(block) {
            indicators::apply_nixie_tag(self.world.create_nixies.entry(pos).or_default(), tag);
        } else if kind == "create:track_signal" && block == kind {
            self.world
                .create_track_signals
                .insert(pos, indicators::track_signal_state(tag));
        } else if kind == "create:flap_display" && block == "create:display_board" {
            let mut state = self
                .world
                .create_display_boards
                .remove(&pos)
                .unwrap_or_default();
            if flap_display::apply_flap_display_tag(&mut state, tag, self.world.game_time) {
                self.world.create_display_boards.insert(pos, state);
            }
        }
    }

    pub(super) fn tick_create_indicators(&mut self) {
        use horizon_client::world::CreateFluidState::Tank;
        for mount in self.world.create_cannon_mounts.values_mut() {
            mount.yaw.tick();
            mount.pitch.tick();
        }
        self.world.create_printer_ticks.retain(|_, ticks| {
            *ticks = ticks.saturating_sub(1);
            *ticks >= 0
        });
        let positions: Vec<_> = self.world.create_whistles.keys().copied().collect();
        for pos @ (x, y, z) in positions {
            let id = self.world.block(x, y, z);
            let active = self.pack.assets.registry.state(id).is_some_and(|block| {
                let props = &block.properties;
                if !props.get("powered").is_some_and(|v| v == "true") { return false; }
                let facing = props.get("facing").map(String::as_str).unwrap_or("north");
                let wall = props.get("wall").is_some_and(|v| v == "true");
                let [dx, dy, dz] = indicators::whistle_attachment(facing, wall);
                let Some(Tank(attached)) = self.world.create_fluids.get(&(x+dx, y+dy, z+dz)) else { return false; };
                match attached.controller {
                    Some([x,y,z]) => matches!(self.world.create_fluids.get(&(x,y,z)), Some(Tank(tank)) if tank.boiler_active),
                    None => attached.boiler_active,
                }
            });
            if let Some(state) = self.world.create_whistles.get_mut(&pos) {
                indicators::tick_whistle_state(state, active);
            }
        }
    }

    pub(super) fn tick_create_pistons(&mut self) {
        let positions: Vec<_> = self.world.create_pistons.keys().copied().collect();
        for pos @ (x, y, z) in positions {
            let id = self.world.block(x, y, z);
            let facing = self
                .pack
                .assets
                .registry
                .state(id)
                .and_then(|s| s.properties.get("facing"))
                .map(String::as_str)
                .unwrap_or("north");
            let speed = self.world.kinetic_speeds.get(&pos).copied().unwrap_or(0.0);
            if let Some(state) = self.world.create_pistons.get_mut(&pos) {
                create_piston::tick_piston_state(state, facing, speed);
            }
        }
    }

    pub(super) fn tick_kinetic_contraptions(&mut self) {
        if self.world.kinetic_contraptions.is_empty() {
            return;
        }
        let positions: Vec<(i32, i32, i32)> =
            self.world.kinetic_contraptions.keys().copied().collect();
        for pos in positions {
            let props = self
                .pack
                .assets
                .registry
                .state(self.world.block(pos.0, pos.1, pos.2))
                .map(|s| &s.properties);
            let ctx = contraptions::ContraptionTickCtx {
                open: props
                    .and_then(|p| p.get("open"))
                    .is_some_and(|v| v == "true"),
                visible: props
                    .and_then(|p| p.get("visible"))
                    .is_none_or(|v| v == "true"),
                powered: props
                    .and_then(|p| p.get("powered"))
                    .is_some_and(|v| v == "true"),
                assembling: props
                    .and_then(|p| p.get("assembling"))
                    .is_some_and(|v| v == "true"),
            };
            if let Some(state) = self.world.kinetic_contraptions.get_mut(&pos) {
                contraptions::tick_contraption_state(state, &ctx);
            }
        }
    }

    /// Advances each Create belt, tunnel, blaze burner, basin, chute and frogport animation state.
    pub(super) fn tick_kinetic_logistics(&mut self) {
        if self.world.kinetic_logistics.is_empty() {
            return;
        }
        let player_xz = self
            .player
            .as_ref()
            .filter(|_| !self.spectator())
            .map(|p| [p.position.x, p.position.z]);
        let positions: Vec<(i32, i32, i32)> =
            self.world.kinetic_logistics.keys().copied().collect();
        for pos in positions {
            let st = self.world.block(pos.0, pos.1, pos.2);
            let props = self.pack.assets.registry.state(st).map(|s| &s.properties);
            let facing = props
                .and_then(|p| p.get("facing").or_else(|| p.get("horizontal_facing")))
                .map_or_else(|| "north".to_owned(), |v| v.clone());
            let blaze = props
                .and_then(|p| p.get("blaze"))
                .cloned()
                .unwrap_or_default();
            let above_name = self
                .res
                .facts()
                .get(self.world.block(pos.0, pos.1 + 1, pos.2))
                .name
                .as_ref();
            let valid_block_above = matches!(
                above_name,
                "create:basin" | "create:fluid_tank" | "create:creative_fluid_tank"
            );
            let two_above = (pos.0, pos.1 + 2, pos.2);
            let mixer_running_above = self
                .res
                .facts()
                .get(self.world.block(two_above.0, two_above.1, two_above.2))
                .name
                .as_ref()
                == "create:mechanical_mixer"
                && self
                    .world
                    .kinetic_machines
                    .get(&two_above)
                    .is_some_and(|m| m.running);
            let belt_speed = self.world.kinetic_speeds.get(&pos).copied().unwrap_or(0.0);
            let ctx = logistics::LogisticsTickCtx {
                player_xz,
                facing,
                blaze,
                valid_block_above,
                mixer_running_above,
                belt_speed,
            };
            if let Some(state) = self.world.kinetic_logistics.get_mut(&pos) {
                logistics::tick_logistics_state(state, [pos.0, pos.1, pos.2], &ctx);
            }
        }
    }

    /// Advances each auxiliary Create block's client animation state once per tick.
    pub(super) fn tick_kinetic_aux(&mut self) {
        if self.world.kinetic_aux.is_empty() {
            return;
        }
        let positions: Vec<(i32, i32, i32)> = self.world.kinetic_aux.keys().copied().collect();
        for pos in positions {
            let extended = matches!(
                self.world.kinetic_aux.get(&pos),
                Some(horizon_client::world::KineticAuxState::Sticker { .. })
            ) && self
                .pack
                .assets
                .registry
                .state(self.world.block(pos.0, pos.1, pos.2))
                .and_then(|s| s.properties.get("extended"))
                .is_some_and(|v| v == "true");
            if let Some(state) = self.world.kinetic_aux.get_mut(&pos) {
                levers::tick_aux_state(state, extended);
            }
        }
    }

    /// Advances each mechanical arm's movement progress and joint targets once per client tick.
    pub(super) fn tick_kinetic_arms(&mut self) {
        if self.world.kinetic_arms.is_empty() {
            return;
        }
        let positions: Vec<(i32, i32, i32)> = self.world.kinetic_arms.keys().copied().collect();
        for pos in positions {
            let speed = self.world.kinetic_speeds.get(&pos).copied().unwrap_or(0.0);
            let ceiling = self
                .pack
                .assets
                .registry
                .state(self.world.block(pos.0, pos.1, pos.2))
                .and_then(|s| s.properties.get("ceiling"))
                .is_some_and(|v| v == "true");
            let target_facing = self.world.kinetic_arms.get(&pos).and_then(|arm| {
                let point =
                    arm::targeted_point(arm.phase, arm.target_index, &arm.inputs, &arm.outputs)?;
                let wp = [
                    pos.0 + point.rel_pos[0],
                    pos.1 + point.rel_pos[1],
                    pos.2 + point.rel_pos[2],
                ];
                let st = self.world.block(wp[0], wp[1], wp[2]);
                self.pack
                    .assets
                    .registry
                    .state(st)
                    .and_then(|s| {
                        s.properties
                            .get("facing")
                            .or_else(|| s.properties.get("horizontal_facing"))
                    })
                    .cloned()
            });
            if let Some(arm) = self.world.kinetic_arms.get_mut(&pos) {
                arm::tick_arm_state(arm, [pos.0, pos.1, pos.2], speed, ceiling, |_| {
                    target_facing.clone()
                });
            }
        }
    }

    /// Keeps `kinetic_speeds` in step with kinetic blocks placed in the world.
    pub(super) fn sync_kinetic_part(&mut self, pos: (i32, i32, i32), name: &str) {
        if name != "create:deployer" {
            self.world.create_deployers.remove(&pos);
        }
        if self
            .world
            .create_fluids
            .get(&pos)
            .is_some_and(|state| !state.matches_block(name))
        {
            self.world.create_fluids.remove(&pos);
        }
        if !create_piston::is_piston_block(name) {
            self.world.create_pistons.remove(&pos);
        }
        if name == "create:steam_whistle" {
            self.world.create_whistles.entry(pos).or_default();
        } else {
            self.world.create_whistles.remove(&pos);
        }
        if !indicators::is_nixie_block(name) {
            self.world.create_nixies.remove(&pos);
        }
        if name != "create:track_signal" {
            self.world.create_track_signals.remove(&pos);
        }
        if name != "create:display_board" {
            self.world.create_display_boards.remove(&pos);
        }
        if name != extra_addons::PRINTER {
            self.world.create_printer_ticks.remove(&pos);
        }
        if name != CANNON_MOUNT {
            self.world.create_cannon_mounts.remove(&pos);
        }
        if self
            .world
            .aeronautics_part_poses
            .get(&pos)
            .is_some_and(|&pose| {
                aeronautics::Part::for_block(name).is_none_or(|part| !part.keeps_pose(pose))
            })
        {
            self.world.aeronautics_part_poses.remove(&pos);
        }
        if is_kinetic_block(name) {
            self.world.kinetic_speeds.entry(pos).or_insert(0.0);
            if name == "create:sticker" {
                let extended = self
                    .pack
                    .assets
                    .registry
                    .state(self.world.block(pos.0, pos.1, pos.2))
                    .and_then(|s| s.properties.get("extended"))
                    .is_some_and(|v| v == "true");
                self.world.kinetic_aux.entry(pos).or_insert_with(|| {
                    horizon_client::world::KineticAuxState::Sticker {
                        piston: horizon_client::world::LerpedValue::start_with(if extended {
                            1.0
                        } else {
                            0.0
                        }),
                    }
                });
            } else if name == "create:blaze_burner" || name == "createaddition:liquid_blaze_burner"
            {
                self.world.kinetic_logistics.entry(pos).or_insert_with(|| {
                    horizon_client::world::KineticLogisticsState::BlazeBurner {
                        goggles: false,
                        hat: false,
                        stock_keeper: false,
                        head_angle: horizon_client::world::LerpedValue::start_with(0.0),
                        head_animation: horizon_client::world::LerpedValue::start_with(0.0),
                    }
                });
            } else if contraptions::is_sliding_door(name) {
                let st = self.world.block(pos.0, pos.1, pos.2);
                let props = self.pack.assets.registry.state(st).map(|s| &s.properties);
                if props
                    .and_then(|p| p.get("half"))
                    .is_none_or(|v| v != "upper")
                {
                    let open = props
                        .and_then(|p| p.get("open"))
                        .is_some_and(|v| v == "true");
                    self.world
                        .kinetic_contraptions
                        .entry(pos)
                        .or_insert_with(|| {
                            horizon_client::world::KineticContraptionState::SlidingDoor {
                                animation: horizon_client::world::LerpedValue::start_with(
                                    if open { 1.0 } else { 0.0 },
                                ),
                                bridge_ticks: 0,
                            }
                        });
                }
            } else if name == "create:desk_bell" {
                self.world
                    .kinetic_contraptions
                    .entry(pos)
                    .or_insert_with(
                        || horizon_client::world::KineticContraptionState::DeskBell {
                            animation: horizon_client::world::LerpedValue::start_with(0.0),
                            animation_offset: ((pos.0.wrapping_mul(31) ^ pos.2).abs() % 64) as f32
                                * 0.1,
                        },
                    );
            } else if name == "create:track_station" {
                self.world
                    .kinetic_contraptions
                    .entry(pos)
                    .or_insert_with(|| {
                        horizon_client::world::KineticContraptionState::TrackStation {
                            flag: horizon_client::world::LerpedValue::start_with(0.0),
                            depot: Default::default(),
                        }
                    });
            }
        } else {
            self.world.kinetic_speeds.remove(&pos);
            self.world.kinetic_held_angles.remove(&pos);
            self.world.kinetic_dials.remove(&pos);
            self.world.kinetic_machines.remove(&pos);
            self.world.kinetic_arms.remove(&pos);
            self.world.kinetic_aux.remove(&pos);
            self.world.kinetic_logistics.remove(&pos);
            self.world.kinetic_contraptions.remove(&pos);
        }
    }

    fn logistics_frustum_pad(state: &horizon_client::world::KineticLogisticsState) -> f32 {
        match state {
            horizon_client::world::KineticLogisticsState::Belt {
                is_controller: true,
                belt_length,
                ..
            } => (*belt_length as f32).clamp(1.5, 20.0),
            horizon_client::world::KineticLogisticsState::Frogport {
                target_rel: Some(rel),
                ..
            } => rel
                .iter()
                .map(|v| v.abs() as f32)
                .fold(3.0f32, f32::max)
                .clamp(3.0, 24.0),
            horizon_client::world::KineticLogisticsState::Frogport { .. }
            | horizon_client::world::KineticLogisticsState::Chute { .. } => 3.0,
            _ => 1.5,
        }
    }

    fn submit_world_item_draws(
        &self,
        at: glam::Vec3,
        origin: [i32; 3],
        default_light: u32,
        draws: &[logistics::WorldItemDraw],
        out: &mut Vec<Vertex>,
    ) {
        for draw in draws {
            let resolved = self.resolve_item(&draw.stack, draw.display, None, 0);
            if resolved.is_empty() {
                continue;
            }
            let is_block_item = self
                .res
                .items()
                .get(&draw.stack.item)
                .is_some_and(|i| i.block.is_some())
                && resolved.uses_block_light();
            let light = if draw.light_offset == [0, 0, 0] {
                default_light
            } else {
                horizon_client::block_entity_render::vertex_light(self.world.light(
                    origin[0] + draw.light_offset[0],
                    origin[1] + draw.light_offset[1],
                    origin[2] + draw.light_offset[2],
                ))
            };
            for local in logistics::item_draw_matrices(draw, is_block_item) {
                let item_pose = glam::Mat4::from_translation(at) * local;
                self.submit_item_world(&resolved, item_pose, light, out);
            }
        }
    }

    /// Draws all kinetic blocks and partial models in view.
    pub(super) fn kinetic_geometry(&self, camera: glam::DVec3, alpha: f64, out: &mut Vec<Vertex>) {
        if self.world.kinetic_speeds.is_empty() {
            return;
        }
        let time = self.world.game_time as f64 + alpha;
        let range = f64::from(self.settings.render_distance) * 16.0;
        let range_sq = range * range;

        for (&(x, y, z), &speed) in &self.world.kinetic_speeds {
            let corner = glam::DVec3::new(f64::from(x), f64::from(y), f64::from(z));
            let at = (corner - camera).as_vec3();
            let board = self.world.create_display_boards.get(&(x, y, z));
            let piston = self.world.create_pistons.get(&(x, y, z));
            let piston_reach = piston.map_or(0.0, |state| state.render_reach());
            let piston_axis = if piston_reach > 0.0 {
                self.pack
                    .assets
                    .registry
                    .state(self.world.block(x, y, z))
                    .and_then(|s| s.properties.get("facing"))
                    .map_or(glam::Vec3::ZERO, |facing| {
                        levers::dir_normal_vec3(facing).abs() * piston_reach
                    })
            } else {
                glam::Vec3::ZERO
            };
            let board_bounds = board.map(|board| {
                let facing = self
                    .pack
                    .assets
                    .registry
                    .state(self.world.block(x, y, z))
                    .and_then(|s| s.properties.get("facing"))
                    .map(String::as_str)
                    .unwrap_or("north");
                flap_display::flap_display_bounds(board, facing)
            });
            let center = at + 0.5;
            let distance_sq = if let Some((min, max)) = board_bounds {
                glam::Vec3::ZERO.clamp(at + min, at + max).length_squared() as f64
            } else if piston_reach > 0.0 {
                glam::Vec3::ZERO
                    .clamp(center - piston_axis, center + piston_axis)
                    .length_squared() as f64
            } else {
                (corner + 0.5 - camera).length_squared()
            };
            if distance_sq > range_sq {
                continue;
            }
            let state = self.world.block(x, y, z);
            let fact_name = &self.res.facts().get(state).name;
            let aero_part = aeronautics::Part::for_block(fact_name);
            let machine_state = self.world.kinetic_machines.get(&(x, y, z)).copied();
            let arm_state = self.world.kinetic_arms.get(&(x, y, z));
            let deployer_state = self.world.create_deployers.get(&(x, y, z));
            let aux_state = self.world.kinetic_aux.get(&(x, y, z));
            let log_state = self.world.kinetic_logistics.get(&(x, y, z));
            let con_state = self.world.kinetic_contraptions.get(&(x, y, z));
            if let Some(view) = &self.view_frustum {
                // A pulley's rope hangs below it (`shouldRenderOffScreen`), and a
                // mechanical arm reaches up to 3 blocks in any direction.
                let reach = machine_state.map_or(0.0, |state| state.offset.clamp(0.0, 512.0));
                let pad = if arm_state.is_some() || deployer_state.is_some() {
                    3.0
                } else if let Some(ls) = log_state {
                    Self::logistics_frustum_pad(ls)
                } else if let Some(
                    horizon_client::world::KineticContraptionState::ChainConveyor { connections },
                ) = con_state
                {
                    connections
                        .iter()
                        .flatten()
                        .map(|n| n.unsigned_abs() as f32)
                        .fold(2.0, f32::max)
                        + 2.0
                } else if contraptions::is_contraption_block(fact_name) {
                    2.0
                } else if aux_state.is_some() {
                    1.5
                } else if fact_name.as_ref() == CANNON_MOUNT {
                    0.5
                } else {
                    aero_part.map_or(0.0, aeronautics::Part::frustum_pad)
                };
                let (board_min, board_max) =
                    board_bounds.unwrap_or((glam::Vec3::ZERO, glam::Vec3::ONE));
                let below = if fact_name.as_ref() == "create:spout"
                    || fact_name.as_ref() == extra_addons::PRINTER
                    || fact_name.as_ref() == CANNON_MOUNT
                {
                    2.0
                } else {
                    reach.max(pad)
                };
                let above = if fact_name.as_ref() == CANNON_MOUNT {
                    2.0
                } else {
                    pad
                };
                if !view.intersects(
                    at + board_min.min(glam::Vec3::ZERO)
                        - glam::Vec3::new(pad, below, pad)
                        - piston_axis,
                    at + board_max.max(glam::Vec3::ONE)
                        + glam::Vec3::new(pad, above, pad)
                        + piston_axis,
                ) {
                    continue;
                }
            }

            let Some(spec) = spec_for_block(fact_name) else {
                continue;
            };
            let speed = if spec.block_name == specs::ENCASED_FAN.block_name {
                fan_speed(speed)
            } else {
                speed
            };

            // Sample block light once per block entity.
            let light_val =
                horizon_client::block_entity_render::vertex_light(self.world.light(x, y, z));

            let block_state_props = self
                .pack
                .assets
                .registry
                .state(state)
                .map(|s| &s.properties);
            if let Some(part) = aero_part {
                let parts = part.parts(
                    |name| {
                        block_state_props
                            .and_then(|props| props.get(name))
                            .map(String::as_str)
                    },
                    speed,
                    self.world.aeronautics_part_poses.get(&(x, y, z)).copied(),
                    time,
                    [x, y, z],
                );
                self.emit_create_parts(at, light_val, parts, out);
                continue;
            }
            let axis_prop = block_state_props
                .and_then(|p| p.get("axis"))
                .map(|s| s.as_str());
            let facing_prop = block_state_props
                .and_then(|p| p.get("facing").or_else(|| p.get("horizontal_facing")))
                .map(|s| s.as_str());
            let face_prop = block_state_props
                .and_then(|p| p.get("face"))
                .map(|s| s.as_str());
            let along_first = block_state_props
                .and_then(|p| p.get("axis_along_first"))
                .is_some_and(|v| v == "true");
            let ceiling = block_state_props
                .and_then(|p| p.get("ceiling"))
                .is_some_and(|v| v == "true");
            let vertical = block_state_props
                .and_then(|p| p.get("vertical"))
                .is_some_and(|v| v == "true");
            let enabled = block_state_props
                .and_then(|p| p.get("enabled"))
                .is_some_and(|v| v == "true");
            let extended = block_state_props
                .and_then(|p| p.get("extended"))
                .is_some_and(|v| v == "true");
            let shape = block_state_props
                .and_then(|p| p.get("shape"))
                .map(|s| s.as_str())
                .unwrap_or("");
            let slope = block_state_props
                .and_then(|p| p.get("slope"))
                .map(|s| s.as_str())
                .unwrap_or("horizontal");
            let belt_part = block_state_props
                .and_then(|p| p.get("part"))
                .map(|s| s.as_str())
                .unwrap_or("middle");
            let blaze = block_state_props
                .and_then(|p| p.get("blaze"))
                .map(|s| s.as_str())
                .unwrap_or("none");
            let open_prop = block_state_props
                .and_then(|p| p.get("open"))
                .is_some_and(|v| v == "true");
            let visible_prop = block_state_props
                .and_then(|p| p.get("visible"))
                .is_none_or(|v| v == "true");
            let powered_prop = block_state_props
                .and_then(|p| p.get("powered"))
                .is_some_and(|v| v == "true");
            let assembling_prop = block_state_props
                .and_then(|p| p.get("assembling"))
                .is_some_and(|v| v == "true");
            let half_prop = block_state_props
                .and_then(|p| p.get("half"))
                .map(|s| s.as_str())
                .unwrap_or("lower");
            let hinge_prop = block_state_props
                .and_then(|p| p.get("hinge"))
                .map(|s| s.as_str())
                .unwrap_or("left");
            let attachment_prop = block_state_props
                .and_then(|p| p.get("attachment"))
                .map(|s| s.as_str())
                .unwrap_or("floor");

            let seed = horizon_core::position::BlockPos::new(x, y, z);

            let resolve_axis =
                |source: specs::AxisSource, is_partial: bool| -> (glam::Quat, glam::Vec3) {
                    match source {
                        specs::AxisSource::BlockAxis => {
                            if is_partial {
                                (axis_y_rotation(axis_prop.unwrap_or("y")), glam::Vec3::Y)
                            } else {
                                let idx = axis_prop.and_then(axis_index).unwrap_or(1);
                                let mut dir = glam::Vec3::ZERO;
                                dir[idx] = 1.0;
                                (glam::Quat::IDENTITY, dir)
                            }
                        }
                        specs::AxisSource::BlockFacingY => {
                            let facing = facing_prop.unwrap_or("north");
                            (facing_y_rotation(facing), glam::Vec3::Y)
                        }
                        specs::AxisSource::BlockFacingZ => {
                            let facing = facing_prop.unwrap_or("north");
                            (facing_z_rotation(facing), glam::Vec3::Z)
                        }
                        specs::AxisSource::BlockFacingOppositeZ => {
                            let facing = facing_prop.unwrap_or("north");
                            (facing_z_rotation(opposite_facing(facing)), glam::Vec3::Z)
                        }
                        specs::AxisSource::BlockAttachFaceY => {
                            let face = face_prop.unwrap_or("floor");
                            let facing = facing_prop.unwrap_or("north");
                            (attach_face_y_rotation(face, facing), glam::Vec3::Y)
                        }
                        specs::AxisSource::BlockDirectionalAxis => {
                            let axis = instruments::directional_axis(
                                facing_prop.unwrap_or("north"),
                                along_first,
                            )
                            .unwrap_or("y");
                            (axis_y_rotation(axis), glam::Vec3::Y)
                        }
                        specs::AxisSource::BlockFacingAxis => {
                            let dir = match facing_prop.unwrap_or("north") {
                                "east" | "west" => glam::Vec3::X,
                                "up" | "down" => glam::Vec3::Y,
                                _ => glam::Vec3::Z,
                            };
                            (glam::Quat::IDENTITY, dir)
                        }
                        specs::AxisSource::Fixed(dir) => (glam::Quat::IDENTITY, dir),
                    }
                };
            let held_angle = if create_piston::is_piston_block(fact_name) {
                None
            } else {
                self.world.kinetic_held_angles.get(&(x, y, z)).copied()
            };

            for part in spec.parts {
                let is_partial = matches!(part.model, specs::PartModel::Partial(_));
                let (base_rot, spin_axis) = match part.motion {
                    specs::Motion::Spin { axis, .. }
                    | specs::Motion::SpinAndOscillate { axis, .. } => {
                        resolve_axis(axis, is_partial)
                    }
                    specs::Motion::Oscillate { direction, .. } => {
                        resolve_axis(direction, is_partial)
                    }
                };

                let (spin_rot, translation) = match part.motion {
                    specs::Motion::Spin {
                        axis,
                        speed_mult,
                        tooth_phase,
                    } => {
                        let block_axis = if axis == specs::AxisSource::BlockDirectionalAxis {
                            instruments::directional_axis(
                                facing_prop.unwrap_or("north"),
                                along_first,
                            )
                        } else {
                            axis_prop
                        };
                        let offset = match tooth_phase {
                            specs::ToothPhase::SmallCog => {
                                let idx = block_axis.and_then(axis_index).unwrap_or(1);
                                rotation_offset(idx, [x, y, z])
                            }
                            specs::ToothPhase::LargeCog => {
                                let idx = block_axis.and_then(axis_index).unwrap_or(1);
                                large_cog_rotation_offset(idx, [x, y, z])
                            }
                            specs::ToothPhase::None => 0.0,
                        };
                        let angle = if let Some(held) = held_angle {
                            held.to_radians()
                        } else if speed == 0.0 {
                            offset.to_radians() as f32
                        } else {
                            angle_radians(time, speed * speed_mult, offset)
                        };
                        (
                            glam::Quat::from_axis_angle(
                                spin_axis,
                                angle * spin_sign(base_rot, spin_axis),
                            ),
                            glam::Vec3::ZERO,
                        )
                    }
                    specs::Motion::Oscillate {
                        amplitude,
                        freq_mult,
                        ..
                    } => {
                        let stroke = if speed == 0.0 {
                            0.0f32
                        } else {
                            let progress =
                                ((time * (speed.abs() as f64) * 0.05 * (freq_mult as f64)) % 1.0)
                                    as f32;
                            if progress < 0.5 {
                                progress * 2.0
                            } else {
                                (1.0 - progress) * 2.0
                            }
                        };
                        (glam::Quat::IDENTITY, spin_axis * (stroke * amplitude))
                    }
                    specs::Motion::SpinAndOscillate {
                        speed_mult,
                        osc_direction,
                        amplitude,
                        freq_mult,
                        ..
                    } => {
                        let angle = if speed == 0.0 {
                            0.0f32
                        } else {
                            angle_radians(time, speed * speed_mult, 0.0)
                        };
                        let stroke = if speed == 0.0 {
                            0.0f32
                        } else {
                            let progress =
                                ((time * (speed.abs() as f64) * 0.05 * (freq_mult as f64)) % 1.0)
                                    as f32;
                            if progress < 0.5 {
                                progress * 2.0
                            } else {
                                (1.0 - progress) * 2.0
                            }
                        };
                        let (_osc_rot, osc_dir) = resolve_axis(osc_direction, is_partial);
                        (
                            glam::Quat::from_axis_angle(
                                spin_axis,
                                angle * spin_sign(base_rot, spin_axis),
                            ),
                            osc_dir * (stroke * amplitude),
                        )
                    }
                };

                let pivot = part.pivot;
                let transform = glam::Mat4::from_translation(at + pivot)
                    * glam::Mat4::from_quat(base_rot)
                    * glam::Mat4::from_translation(translation)
                    * glam::Mat4::from_quat(spin_rot)
                    * glam::Mat4::from_translation(-pivot);

                match part.model {
                    specs::PartModel::BlockState => {
                        self.tinted_block_model_vertices(
                            state,
                            horizon_core::random::position_seed(seed),
                            transform,
                            |_| light_val,
                            false,
                            |_| None,
                            out,
                        );
                    }
                    specs::PartModel::Partial(model_id) => {
                        if let Some(template) = get_or_bake_template(
                            self.pack.item_renderer.library(),
                            &self.gui.atlas.sprites,
                            self.gui.atlas.generation,
                            model_id,
                        ) {
                            emit_template_quads(
                                &template,
                                &self.gui.atlas.sprites,
                                transform,
                                light_val,
                                None,
                                None,
                                false,
                                out,
                            );
                        }
                    }
                }
            }

            let dial = self.world.kinetic_dials.get(&(x, y, z)).copied();
            for (model_id, local) in instruments::instrument_parts(
                fact_name,
                facing_prop.unwrap_or("north"),
                along_first,
                dial.unwrap_or(0.0),
                self.world.day_time,
                speed,
            ) {
                if let Some(template) = get_or_bake_template(
                    self.pack.item_renderer.library(),
                    &self.gui.atlas.sprites,
                    self.gui.atlas.generation,
                    model_id,
                ) {
                    let transform = glam::Mat4::from_translation(at) * local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        None,
                        None,
                        false,
                        out,
                    );
                }
            }

            if let Some(board) = board {
                let glyph = |c| self.gui.atlas.glyph_cell(c);
                for call in flap_display::flap_display_text_calls(
                    board,
                    facing_prop.unwrap_or("north"),
                    at,
                    light_val,
                    time,
                    speed,
                    &|c| glyph(c).1,
                ) {
                    horizon_client::block_entity_render::draw_text_call(&glyph, &call, true, out);
                }
            }

            if fact_name.as_ref() == extra_addons::ORE_KINETIC_INPUT {
                let shaft = block_state_props
                    .and_then(|props| props.get("shaft"))
                    .map(String::as_str);
                if let Some(part) = shaft.and_then(|shaft| {
                    extra_addons::ore_kinetic_input_part(shaft, speed, time, [x, y, z])
                }) {
                    self.emit_create_parts(at, light_val, [part], out);
                }
            } else if fact_name.as_ref() == extra_addons::CARBON_BRUSHES {
                if let Some(parts) = extra_addons::carbon_brush_parts(
                    facing_prop.unwrap_or("north"),
                    speed,
                    time,
                    [x, y, z],
                ) {
                    self.emit_create_parts(at, light_val, parts, out);
                }
            } else if let Some(parts) = extra_addons::addition_machine_parts(
                fact_name.as_ref(),
                facing_prop.unwrap_or("north"),
                speed,
                time,
                [x, y, z],
            ) {
                self.emit_create_parts(at, light_val, parts, out);
            } else if fact_name.as_ref() == "create:spout" {
                self.emit_create_parts(
                    at,
                    light_val,
                    fluids::spout_parts(self.world.create_fluids.get(&(x, y, z)), alpha),
                    out,
                );
            } else if fact_name.as_ref() == CANNON_MOUNT {
                let vertical = block_state_props
                    .and_then(|props| props.get("vertical_direction"))
                    .map(String::as_str)
                    .unwrap_or("down");
                let [mount, axle, pitch, yaw] = extra_addons::cannon_mount_parts(
                    facing_prop.unwrap_or("north"),
                    vertical,
                    self.world.create_cannon_mounts.get(&(x, y, z)),
                    time,
                    alpha,
                    [x, y, z],
                );
                let dy = if vertical == "up" { -1 } else { 1 };
                for (height, part) in [(dy, mount), (2 * dy, axle)] {
                    let light = horizon_client::block_entity_render::vertex_light(
                        self.world.light(x, y + height, z),
                    );
                    self.emit_create_parts(at, light, [part], out);
                }
                self.emit_create_parts(at, light_val, [pitch, yaw], out);
            } else if fact_name.as_ref() == extra_addons::PRINTER {
                let ticks = self.world.create_printer_ticks.get(&(x, y, z)).copied();
                self.emit_create_parts(
                    at,
                    light_val,
                    extra_addons::printer_parts(ticks, alpha),
                    out,
                );
            } else if fact_name.as_ref() == "create:mechanical_arm" {
                let resolved_item = arm_state.and_then(|a| a.held_item.as_ref()).map(|stack| {
                    let resolved = self.resolve_item(
                        stack,
                        horizon_assets::item_model::DisplayContext::Fixed,
                        None,
                        0,
                    );
                    let is_block_item = !resolved.is_empty()
                        && self
                            .res
                            .items()
                            .get(&stack.item)
                            .is_some_and(|i| i.block.is_some())
                        && resolved.uses_block_light();
                    (resolved, is_block_item)
                });
                let (arm_has_item, arm_is_block_item) = match &resolved_item {
                    Some((resolved, is_block)) if !resolved.is_empty() => (true, *is_block),
                    _ => (false, false),
                };
                let machine = machines::Machine {
                    block: fact_name,
                    facing: facing_prop.unwrap_or("north"),
                    axis: axis_prop.unwrap_or("z"),
                    along_first,
                    ceiling,
                    vertical,
                    pos: [x, y, z],
                    speed,
                    time,
                    alpha,
                    state: machine_state.unwrap_or_default(),
                    has_controller_bracket: false,
                    arm: arm_state,
                    arm_has_item,
                    arm_is_block_item,
                };
                let (parts, head_local) = machines::arm_parts_with_head(&machine);
                for part in parts {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
                if let Some((resolved, is_block_item)) = resolved_item
                    && !resolved.is_empty()
                {
                    let item_pose = glam::Mat4::from_translation(at)
                        * arm::held_item_local_matrix(head_local, is_block_item);
                    self.submit_item_world(&resolved, item_pose, light_val, out);
                }
            } else if fact_name.as_ref() == "create:deployer" {
                let held_item_is_block = deployer_state
                    .and_then(|state| state.held_item.as_ref())
                    .is_some_and(|stack| {
                        self.res
                            .items()
                            .get(&stack.item)
                            .is_some_and(|item| item.block.is_some())
                            && self
                                .resolve_item(
                                    stack,
                                    horizon_assets::item_model::DisplayContext::None,
                                    None,
                                    0,
                                )
                                .uses_block_light()
                    });
                let ctx = deployer::DeployerBlock {
                    facing: facing_prop.unwrap_or("north"),
                    along_first,
                    speed,
                    time,
                    alpha,
                    deployer: deployer_state,
                    held_item_is_block,
                };
                let (parts, items) = deployer::deployer_parts_and_items(&ctx);
                for part in parts {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
                self.submit_world_item_draws(at, [x, y, z], light_val, &items, out);
            } else if create_piston::is_piston_block(fact_name) {
                if self
                    .remote
                    .as_ref()
                    .is_some_and(|r| r.create_contraptions.has_piston_head([x, y, z]))
                {
                    continue;
                }
                let moving = block_state_props
                    .and_then(|props| props.get("state"))
                    .is_some_and(|v| v == "moving");
                let parts = create_piston::piston_parts(
                    fact_name,
                    facing_prop.unwrap_or("north"),
                    moving,
                    piston,
                    alpha,
                    -at,
                    range as f32,
                );
                self.emit_create_parts(at, light_val, parts, out);
            } else if fact_name.as_ref() == "create:steam_whistle" {
                let size = block_state_props
                    .and_then(|props| props.get("size"))
                    .map(String::as_str)
                    .unwrap_or("medium");
                let part = indicators::whistle_part(
                    facing_prop.unwrap_or("north"),
                    size,
                    self.world.create_whistles.get(&(x, y, z)),
                    time,
                    alpha,
                );
                self.emit_create_parts(at, light_val, [part], out);
            } else if indicators::is_nixie_block(fact_name) {
                let face = block_state_props
                    .and_then(|props| props.get("double_face"))
                    .map(String::as_str)
                    .or(face_prop)
                    .unwrap_or("floor");
                let facing = facing_prop.unwrap_or("north");
                let [dx, dy, dz] = indicators::nixie_attachment(face, facing);
                let ctx = indicators::NixieBlock {
                    block: fact_name,
                    face,
                    facing,
                    state: self.world.create_nixies.get(&(x, y, z)),
                    signal: self
                        .world
                        .create_track_signals
                        .get(&(x + dx, y + dy, z + dz))
                        .copied(),
                    time,
                };
                self.emit_create_parts(at, light_val, indicators::nixie_signal_parts(&ctx), out);
                let glyph = |c| self.gui.atlas.glyph_cell(c);
                for call in indicators::nixie_text_calls(&ctx, at, &|c| glyph(c).1) {
                    horizon_client::block_entity_render::draw_text_call(&glyph, &call, true, out);
                }
            } else if levers::is_aux_block(fact_name) {
                let ctx = levers::AuxBlock {
                    block: fact_name,
                    facing: facing_prop.unwrap_or("north"),
                    face: face_prop.unwrap_or("floor"),
                    shape,
                    along_first,
                    enabled,
                    extended,
                    pos: [x, y, z],
                    speed,
                    time,
                    alpha,
                    aux: aux_state,
                };
                let (parts, items) = levers::aux_parts_and_item(&ctx);
                for part in parts {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
                self.submit_world_item_draws(at, [x, y, z], light_val, &items, out);
            } else if logistics::is_logistics_block(fact_name) {
                let ctx = logistics::LogisticsBlock {
                    block: fact_name,
                    facing: facing_prop.unwrap_or("north"),
                    slope,
                    part: belt_part,
                    shape,
                    blaze,
                    pos: [x, y, z],
                    speed,
                    time,
                    alpha,
                    logistics: log_state,
                };
                let (parts, items) = logistics::logistics_parts_and_items(&ctx);
                for part in parts {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
                self.submit_world_item_draws(at, [x, y, z], light_val, &items, out);
            } else if contraptions::is_contraption_block(fact_name) {
                let ctx = contraptions::ContraptionBlock {
                    block: fact_name,
                    facing: facing_prop.unwrap_or("north"),
                    axis: axis_prop.unwrap_or("z"),
                    half: half_prop,
                    hinge: hinge_prop,
                    attachment: attachment_prop,
                    open: open_prop,
                    visible: visible_prop,
                    powered: powered_prop,
                    assembling: assembling_prop,
                    pos: [x, y, z],
                    speed,
                    time,
                    alpha,
                    contraption: con_state,
                };
                let (parts, items) = contraptions::contraption_parts_and_items(&ctx);
                let sprites = &self.gui.atlas.sprites;
                if fact_name.as_ref() == "create:chain_conveyor"
                    && let Some(sprite) = sprites
                        .get("block/iron_chain")
                        .or_else(|| sprites.get("minecraft:block/iron_chain"))
                        .or_else(|| sprites.get("block/chain"))
                        .or_else(|| sprites.get(contraptions::CHAIN_SPRITE))
                {
                    for span in contraptions::chain_spans(&ctx) {
                        let [dx, dy, dz] = span.relative;
                        let end_light =
                            horizon_client::block_entity_render::vertex_light(self.world.light(
                                x.saturating_add(dx),
                                y.saturating_add(dy),
                                z.saturating_add(dz),
                            ));
                        span.draw(at, sprite, [light_val, end_light], out);
                    }
                }
                if let Some(halves) = contraptions::sliding_door_halves(&ctx) {
                    let registry = self.pack.assets.registry;
                    if let Some(mut closed) = registry.state(state).cloned() {
                        closed.properties.insert("open".into(), "false".into());
                        closed.properties.insert("visible".into(), "true".into());
                        for (half, offset) in halves {
                            closed.properties.insert("half".into(), half.into());
                            if let Some(id) = registry.id(&closed) {
                                self.tinted_block_model_vertices(
                                    id,
                                    horizon_core::random::position_seed(seed),
                                    glam::Mat4::from_translation(at + offset),
                                    |_| light_val,
                                    false,
                                    |_| None,
                                    out,
                                );
                            }
                        }
                    }
                }
                for part in parts {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light_val,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
                self.submit_world_item_draws(at, [x, y, z], light_val, &items, out);
            } else if machines::is_machine(fact_name) {
                let has_controller_bracket =
                    fact_name.as_ref() == "create:rotation_speed_controller" && {
                        let above_state = self.world.block(x, y + 1, z);
                        self.res.facts().get(above_state).name.as_ref() == "create:large_cogwheel"
                            && self
                                .pack
                                .assets
                                .registry
                                .state(above_state)
                                .and_then(|s| s.properties.get("axis"))
                                .is_some_and(|axis| axis == "x" || axis == "z")
                    };
                let machine = machines::Machine {
                    block: fact_name,
                    facing: facing_prop.unwrap_or("north"),
                    axis: axis_prop.unwrap_or("z"),
                    along_first,
                    ceiling,
                    vertical,
                    pos: [x, y, z],
                    speed,
                    time,
                    alpha,
                    state: machine_state.unwrap_or_default(),
                    has_controller_bracket,
                    arm: None,
                    arm_has_item: false,
                    arm_is_block_item: false,
                };
                for part in machines::machine_parts(&machine) {
                    let Some(template) = get_or_bake_template(
                        self.pack.item_renderer.library(),
                        &self.gui.atlas.sprites,
                        self.gui.atlas.generation,
                        part.model,
                    ) else {
                        continue;
                    };
                    let light = if part.below == 0 {
                        light_val
                    } else {
                        horizon_client::block_entity_render::vertex_light(self.world.light(
                            x,
                            y - part.below,
                            z,
                        ))
                    };
                    let transform = glam::Mat4::from_translation(at) * part.local;
                    emit_template_quads(
                        &template,
                        &self.gui.atlas.sprites,
                        transform,
                        light,
                        part.tint,
                        part.sprite_shift,
                        part.full_bright,
                        out,
                    );
                }
            }
        }
    }

    fn emit_create_parts(
        &self,
        at: glam::Vec3,
        light: u32,
        parts: impl IntoIterator<Item = machines::MachinePart>,
        out: &mut Vec<Vertex>,
    ) {
        let world = glam::Mat4::from_translation(at);
        for part in parts {
            let Some(template) = get_or_bake_template(
                self.pack.item_renderer.library(),
                &self.gui.atlas.sprites,
                self.gui.atlas.generation,
                part.model,
            ) else {
                continue;
            };
            emit_template_quads(
                &template,
                &self.gui.atlas.sprites,
                world * part.local,
                light,
                part.tint,
                part.sprite_shift,
                part.full_bright,
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_world::nbt::{Compound, JavaString};

    fn tag_with(field: Option<(&str, Tag)>) -> Tag {
        let mut fields = Compound::new();
        if let Some((name, value)) = field {
            fields.insert(JavaString::from(name), value);
        }
        Tag::Compound(fields)
    }

    #[test]
    fn create_flap_payloads_decode_exactly_and_fold_repeated_sides() {
        use horizon_client::net::CreateFlap;
        use horizon_protocol::buf::pack_block_pos;

        let pos = [-17, -3, 24];
        let packed = pack_block_pos(pos).to_be_bytes();
        let payload = |tail: &[u8]| [packed.as_slice(), tail].concat();
        assert_eq!(
            CreateFlap::decode("create:funnel_flap", &payload(&[1])),
            Some(CreateFlap::Funnel { pos, inward: true }),
        );
        assert_eq!(
            CreateFlap::decode("create:tunnel_flap", &payload(&[3, 2, 1, 5, 1, 2, 0])),
            Some(CreateFlap::Tunnel {
                pos,
                sides: [None, None, Some(false), None, None, Some(true)],
            }),
        );
        for tail in [
            &[][..],
            &[1, 2],
            &[0, 2, 1],
            &[1, 6, 1],
            &[0xff, 0xff, 0xff, 0xff, 0x07, 2, 1],
        ] {
            assert_eq!(
                CreateFlap::decode("create:tunnel_flap", &payload(tail)),
                None
            );
        }
        assert_eq!(
            CreateFlap::decode("create:funnel_flap", &payload(&[0, 1])),
            None
        );
    }

    #[test]
    fn speed_is_read_signed_from_the_float_field() {
        let tag = tag_with(Some(("Speed", Tag::Float((-64.0f32).to_bits()))));
        assert_eq!(speed_from_nbt(&tag), Some(-64.0));
    }

    #[test]
    fn nonempty_tag_without_speed_is_stopped() {
        let tag = tag_with(Some(("Network", Tag::Int(1))));
        assert_eq!(speed_from_nbt(&tag), Some(0.0));
    }

    #[test]
    fn empty_or_foreign_tag_changes_nothing() {
        assert_eq!(speed_from_nbt(&tag_with(None)), None);
        assert_eq!(speed_from_nbt(&Tag::Int(3)), None);
    }

    #[test]
    fn angle_follows_create_formula() {
        // 256 rpm for 10 ticks: 10 * 256 * 0.3 = 768 degrees = 48 mod 360.
        let angle = angle_radians(10.0, 256.0, 0.0);
        assert!((angle.to_degrees() - 48.0).abs() < 1e-3);
        let reversed = angle_radians(10.0, -256.0, 0.0);
        assert!((reversed.to_degrees() + 48.0).abs() < 1e-3);
        let offset = angle_radians(0.0, 0.0, 22.5);
        assert!((offset.to_degrees() - 22.5).abs() < 1e-4);
    }

    #[test]
    fn parts_turn_about_the_positive_axis_like_create() {
        for facing in ["north", "west", "down"] {
            assert_eq!(
                spin_sign(facing_z_rotation(facing), glam::Vec3::Z),
                -1.0,
                "{facing}"
            );
            assert_eq!(
                spin_sign(facing_y_rotation(facing), glam::Vec3::Y),
                -1.0,
                "{facing}"
            );
        }
        for facing in ["south", "east", "up"] {
            assert_eq!(
                spin_sign(facing_z_rotation(facing), glam::Vec3::Z),
                1.0,
                "{facing}"
            );
            assert_eq!(
                spin_sign(facing_y_rotation(facing), glam::Vec3::Y),
                1.0,
                "{facing}"
            );
        }
        for axis in ["x", "y", "z"] {
            assert_eq!(
                spin_sign(axis_y_rotation(axis), glam::Vec3::Y),
                1.0,
                "{axis}"
            );
        }
        // A north-facing part spun by the flipped angle turns like one about +Z.
        let base = facing_z_rotation("north");
        let angle = 0.7;
        let turned = base
            * glam::Quat::from_axis_angle(glam::Vec3::Z, angle * spin_sign(base, glam::Vec3::Z));
        let expected = glam::Quat::from_axis_angle(glam::Vec3::Z, angle) * base;
        let probe = glam::Vec3::new(0.3, 0.8, -0.2);
        assert!(((turned * probe) - (expected * probe)).length() < 1e-5);
    }

    #[test]
    fn fan_propeller_turns_five_times_faster_but_at_least_80_rpm() {
        assert_eq!(fan_speed(0.0), 0.0);
        assert_eq!(fan_speed(8.0), 80.0);
        assert_eq!(fan_speed(32.0), 160.0);
        assert_eq!(fan_speed(-8.0), -80.0);
        assert_eq!(fan_speed(512.0), 1280.0);
    }

    #[test]
    fn bearings_hold_their_angle_until_they_turn() {
        let bearing = |running: i8, angle: f32| {
            let mut fields = Compound::new();
            fields.insert(JavaString::from("Running"), Tag::Byte(running));
            fields.insert(JavaString::from("Angle"), Tag::Float(angle.to_bits()));
            Tag::Compound(fields)
        };
        assert_eq!(held_angle_from_nbt(&bearing(0, 45.0), 32.0), Some(45.0));
        assert_eq!(held_angle_from_nbt(&bearing(1, 45.0), 0.0), Some(45.0));
        assert_eq!(held_angle_from_nbt(&bearing(1, 45.0), 32.0), None);
        let shaft = tag_with(Some(("Speed", Tag::Float(32.0f32.to_bits()))));
        assert_eq!(held_angle_from_nbt(&shaft, 32.0), None);
    }

    #[test]
    fn gauges_read_their_dial_value() {
        let gauge = tag_with(Some(("Value", Tag::Float(0.45f32.to_bits()))));
        assert_eq!(dial_from_nbt(&gauge), Some(0.45));
        assert_eq!(dial_from_nbt(&tag_with(None)), None);
    }

    #[test]
    fn offset_depends_on_the_coordinates_off_the_axis() {
        assert!(should_offset(1, [1, 7, 1]));
        assert!(!should_offset(1, [1, 7, 2]));
        assert!(should_offset(0, [5, 2, 4]));
        assert!(!should_offset(0, [4, 2, 3]));
        assert!(!should_offset(2, [-1, 0, 9]));
        assert!(should_offset(2, [-1, 1, 9]));
        assert_eq!(rotation_offset(1, [0, 0, 0]), 22.5);
        assert_eq!(rotation_offset(1, [1, 0, 0]), 0.0);
        assert_eq!(large_cog_rotation_offset(1, [0, 0, 0]), 11.25);
        assert_eq!(large_cog_rotation_offset(1, [1, 0, 0]), 0.0);
    }

    #[test]
    fn kinetic_block_entity_matches_addons() {
        assert!(is_kinetic_block_entity("create:simple_kinetic"));
        assert!(is_kinetic_block_entity("create:encased_fan"));
        assert!(is_kinetic_block_entity("createaddition:electric_motor"));
        assert!(is_kinetic_block_entity("create_new_age:basic_motor"));
        assert!(is_kinetic_block_entity(
            "create_enchantment_industry:mechanical_grindstone"
        ));
        assert!(is_kinetic_block_entity(
            "create_mechanical_extruder:mechanical_extruder"
        ));
        assert!(!is_kinetic_block_entity("minecraft:furnace"));
    }

    #[test]
    fn facing_rotations_are_normalized() {
        let cases = [
            ("up", glam::Vec3::Y),
            ("down", glam::Vec3::NEG_Y),
            ("north", glam::Vec3::NEG_Z),
            ("south", glam::Vec3::Z),
            ("east", glam::Vec3::X),
            ("west", glam::Vec3::NEG_X),
        ];
        for (facing, expected) in cases {
            let qy = facing_y_rotation(facing);
            assert!(qy.is_normalized(), "facing Y {facing} must be normalized");
            assert!(
                ((qy * glam::Vec3::Y) - expected).length() < 1e-5,
                "facing_y_rotation({facing}) mismatch"
            );

            let qz = facing_z_rotation(facing);
            assert!(qz.is_normalized(), "facing Z {facing} must be normalized");
            assert!(
                ((qz * glam::Vec3::Z) - expected).length() < 1e-5,
                "facing_z_rotation({facing}) mismatch"
            );
        }
    }

    #[test]
    fn axis_y_rotation_aligns_y_to_requested_axis() {
        let cases = [
            ("x", glam::Vec3::X),
            ("y", glam::Vec3::Y),
            ("z", glam::Vec3::Z),
        ];
        for (axis, expected) in cases {
            let q = axis_y_rotation(axis);
            assert!(
                q.is_normalized(),
                "axis_y_rotation({axis}) must be normalized"
            );
            assert!(
                ((q * glam::Vec3::Y) - expected).length() < 1e-5,
                "axis_y_rotation({axis}) mismatch"
            );
        }
    }

    #[test]
    fn attach_face_y_rotation_aligns_steam_engine_orientations() {
        let wall_cases = [
            ("north", glam::Vec3::NEG_Z),
            ("south", glam::Vec3::Z),
            ("east", glam::Vec3::X),
            ("west", glam::Vec3::NEG_X),
        ];
        for (facing, expected) in wall_cases {
            let q_floor = attach_face_y_rotation("floor", facing);
            assert!(q_floor.is_normalized());
            assert!(
                ((q_floor * glam::Vec3::Y) - glam::Vec3::Y).length() < 1e-5,
                "floor {facing} must point +Y"
            );

            let q_ceil = attach_face_y_rotation("ceiling", facing);
            assert!(q_ceil.is_normalized());
            assert!(
                ((q_ceil * glam::Vec3::Y) - glam::Vec3::NEG_Y).length() < 1e-5,
                "ceiling {facing} must point -Y"
            );

            let q_wall = attach_face_y_rotation("wall", facing);
            assert!(q_wall.is_normalized());
            assert!(
                ((q_wall * glam::Vec3::Y) - expected).length() < 1e-5,
                "wall {facing} must point {expected:?}"
            );
        }

        // Verify linkage horizontal Z orientation on floor north vs east.
        let north_z = attach_face_y_rotation("floor", "north") * glam::Vec3::Z;
        let east_z = attach_face_y_rotation("floor", "east") * glam::Vec3::Z;
        assert!((north_z - glam::Vec3::Z).length() < 1e-5);
        assert!((east_z - glam::Vec3::NEG_X).length() < 1e-5);
    }

    #[test]
    fn mixer_pole_and_head_share_oscillation_frequency() {
        let spec = spec_for_block("create:mechanical_mixer").unwrap();
        assert_eq!(spec.parts.len(), 2);
        let (pole_amp, pole_freq) = match spec.parts[0].motion {
            specs::Motion::Oscillate {
                amplitude,
                freq_mult,
                ..
            } => (amplitude, freq_mult),
            other => panic!("expected Oscillate pole, got {other:?}"),
        };
        let (head_amp, head_freq) = match spec.parts[1].motion {
            specs::Motion::SpinAndOscillate {
                amplitude,
                freq_mult,
                ..
            } => (amplitude, freq_mult),
            other => panic!("expected SpinAndOscillate head, got {other:?}"),
        };
        assert_eq!(pole_amp, head_amp);
        assert_eq!(pole_freq, head_freq);
        assert_eq!(head_freq, 0.5);
    }

    #[test]
    fn fan_propeller_blades_face_the_way_the_fan_blows() {
        // `EncasedFanRenderer`: the propeller is `partialFacing(.., direction.getOpposite())`,
        // so its native +Z goes to the opposite of the facing and the blades,
        // drawn on the model's north face, look along the facing.
        for facing in ["north", "south", "east", "west", "up", "down"] {
            let rotation = facing_z_rotation(opposite_facing(facing));
            let blades = rotation * glam::Vec3::NEG_Z;
            let along_facing = facing_z_rotation(facing) * glam::Vec3::Z;
            assert!((blades - along_facing).length() < 1e-5, "{facing}");
        }
        assert!(matches!(
            specs::ENCASED_FAN.parts[0].motion,
            specs::Motion::Spin {
                axis: specs::AxisSource::BlockFacingOppositeZ,
                ..
            }
        ));
    }

    #[test]
    fn pulleys_and_arms_read_their_offset_and_goggles() {
        let mut rope = Compound::new();
        rope.insert(JavaString::from("Offset"), Tag::Float(2.5f32.to_bits()));
        rope.insert(JavaString::from("Running"), Tag::Byte(1));
        assert_eq!(
            machine_state_from_nbt(&Tag::Compound(rope)),
            Some(MachineState {
                offset: 2.5,
                running: true,
                goggles: false,
                source: None,
            })
        );
        // A hose pulley's offset is a `LerpedFloat` compound.
        let mut lerped = Compound::new();
        lerped.insert(JavaString::from("Value"), Tag::Float(4.0f32.to_bits()));
        let mut hose = Compound::new();
        hose.insert(JavaString::from("Offset"), Tag::Compound(lerped));
        assert_eq!(
            machine_state_from_nbt(&Tag::Compound(hose)),
            Some(MachineState {
                offset: 4.0,
                running: false,
                goggles: false,
                source: None,
            })
        );
        let mut arm = Compound::new();
        arm.insert(JavaString::from("Goggles"), Tag::Byte(1));
        assert_eq!(
            machine_state_from_nbt(&Tag::Compound(arm)),
            Some(MachineState {
                offset: 0.0,
                running: false,
                goggles: true,
                source: None,
            })
        );
        let mut gearbox = Compound::new();
        gearbox.insert(JavaString::from("Source"), Tag::IntArray(vec![10, 64, -3]));
        assert_eq!(
            machine_state_from_nbt(&Tag::Compound(gearbox)),
            Some(MachineState {
                offset: 0.0,
                running: false,
                goggles: false,
                source: Some([10, 64, -3]),
            })
        );
        let mut legacy_src = Compound::new();
        legacy_src.insert(JavaString::from("X"), Tag::Int(2));
        legacy_src.insert(JavaString::from("Y"), Tag::Int(65));
        legacy_src.insert(JavaString::from("Z"), Tag::Int(7));
        let mut gearbox_legacy = Compound::new();
        gearbox_legacy.insert(JavaString::from("Source"), Tag::Compound(legacy_src));
        assert_eq!(
            machine_state_from_nbt(&Tag::Compound(gearbox_legacy)),
            Some(MachineState {
                offset: 0.0,
                running: false,
                goggles: false,
                source: Some([2, 65, 7]),
            })
        );
        assert_eq!(machine_state_from_nbt(&tag_with(None)), None);
        assert_eq!(machine_state_from_nbt(&Tag::Int(1)), None);
    }

    /// The arm log names the fields a server sends and leaves out the
    /// movement's progress from what makes a line new.
    #[test]
    fn arm_nbt_lines_show_phase_target_and_points() {
        let mut point = Compound::new();
        point.insert(
            JavaString::from("Type"),
            Tag::String(JavaString::from("create:depot")),
        );
        point.insert(JavaString::from("Pos"), Tag::IntArray(vec![1, 2, 3]));
        let mut held = Compound::new();
        held.insert(
            JavaString::from("id"),
            Tag::String(JavaString::from("minecraft:stone")),
        );
        held.insert(JavaString::from("count"), Tag::Int(1));
        let mut arm = Compound::new();
        arm.insert(
            JavaString::from("Phase"),
            Tag::String(JavaString::from("MOVE_TO_INPUT")),
        );
        arm.insert(JavaString::from("TargetPointIndex"), Tag::Int(0));
        arm.insert(
            JavaString::from("MovementProgress"),
            Tag::Float(0.25f32.to_bits()),
        );
        arm.insert(
            JavaString::from("InteractionPoints"),
            Tag::List(vec![Tag::Compound(point)]),
        );
        arm.insert(JavaString::from("HeldItem"), Tag::Compound(held));
        let arm = Tag::Compound(arm);
        assert_eq!(
            describe_arm_nbt(&arm, true).as_deref(),
            Some(
                "Phase=MOVE_TO_INPUT TargetPointIndex=0 MovementProgress=0.250 \
                 InteractionPoints=list1{Pos,Type} HeldItem={id=minecraft:stone count,id}"
            )
        );
        assert_eq!(
            describe_arm_nbt(&arm, false).as_deref(),
            Some(
                "Phase=MOVE_TO_INPUT TargetPointIndex=0 InteractionPoints=list1{Pos,Type} \
                 HeldItem={id=minecraft:stone count,id}"
            )
        );
        assert_eq!(describe_arm_nbt(&tag_with(None), true), None);
        assert_eq!(describe_arm_nbt(&Tag::Int(1), true), None);
    }

    #[test]
    fn machine_blocks_are_kinetic() {
        for block in [
            "create:mechanical_saw",
            "create:mechanical_harvester",
            "create:rope_pulley",
            "create:hose_pulley",
            "create:elevator_pulley",
        ] {
            assert!(is_kinetic_block(block), "{block}");
        }
    }

    #[test]
    fn all_core_and_addon_specs_registered() {
        assert!(is_kinetic_block("create:shaft"));
        assert!(is_kinetic_block("create:cogwheel"));
        assert!(is_kinetic_block("create:large_cogwheel"));
        assert!(is_kinetic_block("create:encased_fan"));
        assert!(is_kinetic_block("create:mechanical_drill"));
        assert!(is_kinetic_block("create:mechanical_press"));
        assert!(is_kinetic_block("create:mechanical_mixer"));
        assert!(is_kinetic_block("create:millstone"));
        assert!(is_kinetic_block("create:water_wheel"));
        assert!(is_kinetic_block("create:large_water_wheel"));
        assert!(is_kinetic_block("create:flywheel"));
        assert!(is_kinetic_block("create:powered_shaft"));
        assert!(is_kinetic_block("create:mechanical_bearing"));
        assert!(is_kinetic_block("create:windmill_bearing"));
        assert!(is_kinetic_block("create:clockwork_bearing"));
        assert!(is_kinetic_block("create:andesite_encased_shaft"));
        assert!(is_kinetic_block("create:brass_encased_shaft"));
        assert!(is_kinetic_block("create:clutch"));
        assert!(is_kinetic_block("create:gearshift"));
        assert!(is_kinetic_block("create:encased_chain_drive"));
        assert!(is_kinetic_block("create:adjustable_chain_gearshift"));
        assert!(is_kinetic_block("create:gantry_shaft"));
        assert!(is_kinetic_block("create:andesite_encased_cogwheel"));
        assert!(is_kinetic_block("create:brass_encased_cogwheel"));
        assert!(is_kinetic_block("create:andesite_encased_large_cogwheel"));
        assert!(is_kinetic_block("create:brass_encased_large_cogwheel"));
        assert!(is_kinetic_block("create:steam_engine"));
        assert!(is_kinetic_block("create:mechanical_arm"));
        assert!(is_kinetic_block("create:spout"));
        assert!(is_kinetic_block("create:mechanical_pump"));
        assert!(is_kinetic_block("create:speedometer"));
        assert!(is_kinetic_block("create:stressometer"));
        assert!(is_kinetic_block("create:cuckoo_clock"));
        assert!(is_kinetic_block("create:mysterious_cuckoo_clock"));
        assert!(is_kinetic_block("create:creative_motor"));
        assert!(is_kinetic_block("create:gearbox"));
        assert!(is_kinetic_block("create:sequenced_gearshift"));
        assert!(is_kinetic_block("create:rotation_speed_controller"));
        assert!(is_kinetic_block("create:copper_backtank"));
        assert!(is_kinetic_block("create:netherite_backtank"));
        assert!(is_kinetic_block("create:mechanical_crafter"));
        assert!(is_kinetic_block("create:display_board"));
        assert!(is_kinetic_block("create:gantry_carriage"));
        assert!(is_kinetic_block("create:fluid_valve"));
        assert!(is_kinetic_block("create:analog_lever"));
        assert!(is_kinetic_block("create:andesite_funnel"));
        assert!(is_kinetic_block("create:brass_funnel"));
        assert!(is_kinetic_block("create:andesite_belt_funnel"));
        assert!(is_kinetic_block("create:brass_belt_funnel"));
        assert!(is_kinetic_block("create:weighted_ejector"));
        assert!(is_kinetic_block("create:sticker"));
        assert!(is_kinetic_block("create:packager"));
        assert!(is_kinetic_block("create:repackager"));
        assert!(is_kinetic_block("create:portable_storage_interface"));
        assert!(is_kinetic_block("create:portable_fluid_interface"));
        assert!(is_kinetic_block("create:copper_valve_handle"));
        for color in [
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
        ] {
            assert!(is_kinetic_block(&format!("create:{color}_valve_handle")));
            assert!(is_kinetic_block(&format!("create:{color}_toolbox")));
            assert!(horizon_client::mesher::is_kinetic_part(&format!(
                "create:{color}_valve_handle"
            )));
            assert!(!horizon_client::mesher::is_kinetic_part(&format!(
                "create:{color}_toolbox"
            )));
        }
        assert!(horizon_client::mesher::is_kinetic_part(
            "create:copper_valve_handle"
        ));
        assert!(!horizon_client::mesher::is_kinetic_part(
            "create:analog_lever"
        ));
        assert!(!horizon_client::mesher::is_kinetic_part(
            "create:fluid_valve"
        ));
        assert!(!horizon_client::mesher::is_kinetic_part(
            "create:brass_funnel"
        ));
        assert!(!horizon_client::mesher::is_kinetic_part("create:packager"));
        assert!(!horizon_client::mesher::is_kinetic_part(
            "create:portable_storage_interface"
        ));
        for block in [
            "create:belt",
            "create:andesite_tunnel",
            "create:brass_tunnel",
            "create:blaze_burner",
            "create:depot",
            "create:chute",
            "create:smart_chute",
            "create:basin",
            "create:package_frogport",
            "create:schematicannon",
            "create:mechanical_roller",
            "create:chain_conveyor",
            "create:train_door",
            "create:framed_glass_door",
            "create:brass_door",
            "create:andesite_door",
            "create:copper_door",
            "create:peculiar_bell",
            "create:haunted_bell",
            "create:desk_bell",
            "create:item_drain",
            "create:track_station",
            "create:small_bogey",
            "create:large_bogey",
            "createaddition:portable_energy_interface",
            "createaddition:liquid_blaze_burner",
        ] {
            assert!(is_kinetic_block(block), "{block}");
            assert!(!horizon_client::mesher::is_kinetic_part(block), "{block}");
        }
        assert!(is_kinetic_block("createaddition:electric_motor"));
        assert!(is_kinetic_block("createaddition:alternator"));
        assert!(is_kinetic_block("createaddition:rolling_mill"));
        assert!(is_kinetic_block(
            "create_mechanical_extruder:mechanical_extruder"
        ));
        assert!(is_kinetic_block(
            "create_enchantment_industry:mechanical_grindstone"
        ));
        assert!(is_kinetic_block("create_new_age:basic_motor"));
        assert!(!is_kinetic_block("minecraft:stone"));
    }

    #[test]
    fn template_cache_re_bakes_when_atlas_generation_changes_and_shifts_uv_at_emit() {
        use horizon_assets::{model::CuboidModel, resolve::ModelLibrary};
        let _cache = super::TEMPLATE_CACHE_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use horizon_client::hud::Rect;
        use std::collections::BTreeMap;

        let model = CuboidModel::from_json(
            r##"{"textures":{"u":"create:block/belt"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"up":{"uv":[0,0,16,16],"texture":"#u"}}}]}"##,
        )
        .unwrap();
        let lib = Arc::new(
            ModelLibrary::new(BTreeMap::from([(
                "create:block/test_belt_cache".to_owned(),
                model,
            )]))
            .unwrap(),
        );
        let rect = |u0, v0, u1, v1| Rect {
            u0,
            v0,
            u1,
            v1,
            w: 16,
            h: 16,
        };
        let sprites1 = HashMap::from([
            ("create:block/belt".to_owned(), rect(0.0, 0.0, 0.25, 0.25)),
            (
                "create:block/belt_scroll".to_owned(),
                rect(0.25, 0.0, 0.5, 0.5),
            ),
        ]);
        let sprites2 =
            HashMap::from([("create:block/belt".to_owned(), rect(0.5, 0.5, 0.75, 0.75))]);

        let a =
            get_or_bake_template(&lib, &sprites1, 1001, "create:block/test_belt_cache").unwrap();
        let a_hit =
            get_or_bake_template(&lib, &sprites1, 1001, "create:block/test_belt_cache").unwrap();
        assert!(Arc::ptr_eq(&a, &a_hit));

        let mut base_verts = Vec::new();
        emit_template_quads(
            &a,
            &sprites1,
            glam::Mat4::IDENTITY,
            0,
            None,
            None,
            false,
            &mut base_verts,
        );
        let mut shifted_verts = Vec::new();
        let shift = machines::SpriteShift {
            original: "create:block/belt",
            target: "create:block/belt_scroll",
            scroll_u: 0.0,
            scroll_v: 0.25,
        };
        emit_template_quads(
            &a,
            &sprites1,
            glam::Mat4::IDENTITY,
            0,
            None,
            Some(shift),
            true,
            &mut shifted_verts,
        );
        assert_eq!(base_verts.len(), 4);
        assert_eq!(shifted_verts.len(), 4);
        // Cached template remains unmutated; shifted UVs enter target rect + 0.25 * 0.5 scroll.
        assert!((a.quads[0].uv[0][0] - 0.0).abs() < 1e-6);
        assert!((shifted_verts[0].uv[0] - 0.25).abs() < 1e-6);
        assert!((shifted_verts[0].uv[1] - 0.125).abs() < 1e-6);
        assert_ne!(shifted_verts[0].light, 0);

        let b =
            get_or_bake_template(&lib, &sprites2, 1002, "create:block/test_belt_cache").unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert!((b.quads[0].uv[0][0] - 0.5).abs() < 1e-6);
    }
}
