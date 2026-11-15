//! Create's `KineticBlockEntityRenderer` for shafts and small cogwheels: the
//! block entity's `Speed` (rpm) arrives in its update tag, and the block's own
//! model is drawn turning about its `axis`; the chunk mesh leaves it out.
use super::Game;
use horizon_client::mesher::{Vertex, is_kinetic_part};
use horizon_world::nbt::{JavaString, Tag};

pub(super) const KINETIC_BLOCK_ENTITY: &str = "create:simple_kinetic";

/// Degrees per tick per rpm (`getAngleForBe`: `time * speed * 3 / 10`).
const DEGREES_PER_TICK_PER_RPM: f64 = 3.0 / 10.0;
/// `KineticBlockEntityVisual.rotationOffset` when the position says so.
const HALF_TOOTH_OFFSET: f64 = 22.5;

/// `Speed` of a kinetic block entity's tag: a missing field is a stopped one.
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

/// `KineticBlockEntityVisual.shouldOffset`: the coordinates off the axis sum
/// to an even number (Java's `% 2 == 0`, so negative odd sums are not).
pub(super) fn should_offset(axis: usize, pos: [i32; 3]) -> bool {
    let sum: i64 = (0..3)
        .filter(|&i| i != axis)
        .map(|i| i64::from(pos[i]))
        .sum();
    sum % 2 == 0
}

/// `getRotationOffsetForPosition` for the small parts (no large cog bonus).
pub(super) fn rotation_offset(axis: usize, pos: [i32; 3]) -> f64 {
    if should_offset(axis, pos) {
        HALF_TOOTH_OFFSET
    } else {
        0.0
    }
}

/// `getAngleForBe`, in radians: `((time * speed * 3 / 10 + offset) % 360)`.
pub(super) fn angle_radians(time: f64, speed: f32, offset: f64) -> f32 {
    let degrees = (time * f64::from(speed) * DEGREES_PER_TICK_PER_RPM + offset) % 360.0;
    degrees.to_radians() as f32
}

fn axis_index(name: &str) -> Option<usize> {
    match name {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    }
}

impl Game<'_> {
    /// The tag of a `create:simple_kinetic` block entity: its speed is kept
    /// for the shaft or cogwheel standing there (`sync_kinetic_part` made
    /// the entry); other kinetic blocks are not drawn turning yet.
    pub(super) fn store_kinetic_speed(&mut self, pos: (i32, i32, i32), tag: &Tag) {
        let Some(speed) = speed_from_nbt(tag) else {
            return;
        };
        if let Some(stored) = self.world.kinetic_speeds.get_mut(&pos) {
            *stored = speed;
        }
    }

    /// Keeps `kinetic_speeds` to the shafts and small cogwheels there are.
    pub(super) fn sync_kinetic_part(&mut self, pos: (i32, i32, i32), name: &str) {
        if is_kinetic_part(name) {
            self.world.kinetic_speeds.entry(pos).or_insert(0.0);
        } else {
            self.world.kinetic_speeds.remove(&pos);
        }
    }

    /// Each shaft and small cogwheel in range, turned about its axis.
    pub(super) fn kinetic_geometry(&self, camera: glam::DVec3, alpha: f64, out: &mut Vec<Vertex>) {
        if self.world.kinetic_speeds.is_empty() {
            return;
        }
        let time = self.world.game_time as f64 + alpha;
        let range = super::entity_culling::default_range(glam::DVec3::ONE, self.view_scale());
        for (&(x, y, z), &speed) in &self.world.kinetic_speeds {
            let corner = glam::DVec3::new(f64::from(x), f64::from(y), f64::from(z));
            if (corner + 0.5 - camera).length_squared() > range * range {
                continue;
            }
            let state = self.world.block(x, y, z);
            if !is_kinetic_part(&self.res.facts().get(state).name) {
                continue;
            }
            let Some(axis) = self
                .pack
                .assets
                .registry
                .state(state)
                .and_then(|s| s.properties.get("axis"))
                .and_then(|a| axis_index(a))
            else {
                continue;
            };
            let angle = angle_radians(time, speed, rotation_offset(axis, [x, y, z]));
            let mut direction = glam::Vec3::ZERO;
            direction[axis] = 1.0;
            let spin = glam::Quat::from_axis_angle(direction, angle);
            let seed = horizon_core::position::BlockPos::new(x, y, z);
            self.spun_block_vertices(state, seed, corner - camera, spin, out);
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
        // Axis y ignores y: x + z decides.
        assert!(should_offset(1, [1, 7, 1]));
        assert!(!should_offset(1, [1, 7, 2]));
        // Axis x ignores x.
        assert!(should_offset(0, [5, 2, 4]));
        assert!(!should_offset(0, [4, 2, 3]));
        // Java's remainder keeps the sign: -1 % 2 is -1, not 0.
        assert!(!should_offset(2, [-1, 0, 9]));
        assert!(should_offset(2, [-1, 1, 9]));
        assert_eq!(rotation_offset(1, [0, 0, 0]), 22.5);
        assert_eq!(rotation_offset(1, [1, 0, 0]), 0.0);
    }
}
