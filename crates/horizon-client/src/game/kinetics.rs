//! Create and addon kinetic animation engine.
//!
//! Block entities report their `Speed` (rpm) in network update tags. Dynamic parts
//! are drawn turned, translated or oscillated by the game, while static casings
//! stay in the chunk terrain mesh.
use super::Game;
use horizon_client::mesher::Vertex;
use horizon_world::nbt::{JavaString, Tag};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub mod addons;
pub mod specs;

pub use addons::*;
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

/// Whether the tag contains a readable kinetic `Speed` field.
pub(super) fn has_kinetic_speed(tag: &Tag) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    fields.contains_key(&JavaString::from("Speed"))
}

/// Checks whether a block entity identifier belongs to Create or one of its kinetic addons.
pub(super) fn is_kinetic_block_entity(name: &str) -> bool {
    name == KINETIC_BLOCK_ENTITY
        || name.starts_with("create:")
        || name.starts_with("createaddition:")
        || name.starts_with("create_enchantment_industry:")
        || name.starts_with("create_mechanical_extruder:")
        || name.starts_with("create_new_age:")
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
    specs::core_spec(name).or_else(|| addons::addon_spec(name))
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
    pub shade: u32,
    pub emission: u8,
}

#[derive(Clone, Debug)]
pub struct BakedTemplate {
    pub quads: Arc<[TemplateQuad]>,
}

struct TemplateCache {
    library_id: usize,
    templates: HashMap<&'static str, Arc<BakedTemplate>>,
}

fn get_or_bake_template(
    library: &Arc<horizon_assets::resolve::ModelLibrary>,
    sprites: &HashMap<String, horizon_client::hud::Rect>,
    model_id: &'static str,
) -> Option<Arc<BakedTemplate>> {
    static CACHE: Mutex<Option<TemplateCache>> = Mutex::new(None);
    let library_id = Arc::as_ptr(library) as usize;

    let mut cache_guard = CACHE.lock().ok()?;
    let cache = cache_guard.get_or_insert_with(|| TemplateCache {
        library_id,
        templates: HashMap::new(),
    });
    if cache.library_id != library_id {
        cache.library_id = library_id;
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
    for quad in &quads {
        let sprite_name = quad.material.sprite.as_str();
        let key = sprite_name
            .strip_prefix("minecraft:")
            .unwrap_or(sprite_name);
        let Some(r) = sprites.get(key).or_else(|| sprites.get(sprite_name)) else {
            continue;
        };
        let (u0, v0, u1, v1) = (r.u0, r.v0, r.u1, r.v1);

        let mut uv = [[0.0f32; 2]; 4];
        for k in 0..4 {
            uv[k] = [
                u0 + quad.uv[k][0] * (u1 - u0),
                v0 + quad.uv[k][1] * (v1 - v0),
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
            shade,
            emission: quad.light_emission,
        });
    }

    let baked = Arc::new(BakedTemplate {
        quads: template_quads.into(),
    });
    cache.templates.insert(model_id, baked.clone());
    Some(baked)
}

fn emit_template_quads(
    template: &BakedTemplate,
    matrix: glam::Mat4,
    light_val: u32,
    out: &mut Vec<Vertex>,
) {
    for q in template.quads.iter() {
        let color = 0xFF00_0000 | (q.shade << 16) | (q.shade << 8) | q.shade;
        let light = if q.emission > 0 {
            let em = u32::from(q.emission) * 16;
            (light_val & 0xFF) | ((light_val >> 8 & 0xFF).max(em) << 8)
        } else {
            light_val
        };
        for k in 0..4 {
            let p = glam::Vec3::from_array(q.positions[k]);
            let world_pos = matrix.transform_point3(p);
            out.push(Vertex {
                pos: world_pos.to_array(),
                uv: q.uv[k],
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
    /// Stores the speed of a kinetic block entity at `pos`.
    pub(super) fn store_kinetic_speed(&mut self, pos: (i32, i32, i32), tag: &Tag) {
        let Some(speed) = speed_from_nbt(tag) else {
            return;
        };
        self.world.kinetic_speeds.insert(pos, speed);
    }

    /// Keeps `kinetic_speeds` in step with kinetic blocks placed in the world.
    pub(super) fn sync_kinetic_part(&mut self, pos: (i32, i32, i32), name: &str) {
        if is_kinetic_block(name) {
            self.world.kinetic_speeds.entry(pos).or_insert(0.0);
        } else {
            self.world.kinetic_speeds.remove(&pos);
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
            if (corner + 0.5 - camera).length_squared() > range_sq {
                continue;
            }
            if let Some(view) = &self.view_frustum {
                let min = (corner - camera).as_vec3();
                if !view.intersects(min, min + glam::Vec3::ONE) {
                    continue;
                }
            }

            let state = self.world.block(x, y, z);
            let fact_name = &self.res.facts().get(state).name;
            let Some(spec) = spec_for_block(fact_name) else {
                continue;
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
            let axis_prop = block_state_props
                .and_then(|p| p.get("axis"))
                .map(|s| s.as_str());
            let facing_prop = block_state_props
                .and_then(|p| p.get("facing").or_else(|| p.get("horizontal_facing")))
                .map(|s| s.as_str());
            let face_prop = block_state_props
                .and_then(|p| p.get("face"))
                .map(|s| s.as_str());

            let at = (corner - camera).as_vec3();
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
                        specs::AxisSource::Fixed(dir) => (glam::Quat::IDENTITY, dir),
                    }
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
                        speed_mult,
                        tooth_phase,
                        ..
                    } => {
                        let offset = match tooth_phase {
                            specs::ToothPhase::SmallCog => {
                                let idx = axis_prop.and_then(axis_index).unwrap_or(1);
                                rotation_offset(idx, [x, y, z])
                            }
                            specs::ToothPhase::LargeCog => {
                                let idx = axis_prop.and_then(axis_index).unwrap_or(1);
                                large_cog_rotation_offset(idx, [x, y, z])
                            }
                            specs::ToothPhase::None => 0.0,
                        };
                        let angle = if speed == 0.0 {
                            offset.to_radians() as f32
                        } else {
                            angle_radians(time, speed * speed_mult, offset)
                        };
                        (
                            glam::Quat::from_axis_angle(spin_axis, angle),
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
                            glam::Quat::from_axis_angle(spin_axis, angle),
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
                            model_id,
                        ) {
                            emit_template_quads(&template, transform, light_val, out);
                        }
                    }
                }
            }
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
            assert!(q.is_normalized(), "axis_y_rotation({axis}) must be normalized");
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
}
