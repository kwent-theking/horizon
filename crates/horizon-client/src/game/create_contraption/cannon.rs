//! Create Big Cannons 5.11.7 pitch-contraption spawn metadata and visual pose.

use glam::{DVec3, Mat4, Vec3};
use horizon_client_core::nbt_value::text;
use horizon_world::nbt::Tag;

pub(in crate::game) const PITCH_CONTRAPTION: &str = "createbigcannons:pitch_contraption";

#[derive(Clone, Copy, Debug)]
enum Orientation {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Orientation {
    fn initial_yaw(self) -> f32 {
        match self {
            Self::North => 180.0,
            Self::West => 90.0,
            Self::East => 270.0,
            Self::South | Self::Down | Self::Up => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::game) struct PitchContraptionPose {
    orientation: Orientation,
    pub controller: Option<[i32; 3]>,
    pub yaw: f32,
    pub pitch: f32,
}

impl PitchContraptionPose {
    pub(in crate::game) fn from_spawn(tag: &Tag, position: DVec3) -> Option<Self> {
        let orientation = match text(tag.get("InitialOrientation"))?
            .to_ascii_lowercase()
            .as_str()
        {
            "down" => Orientation::Down,
            "up" => Orientation::Up,
            "north" => Orientation::North,
            "south" => Orientation::South,
            "west" => Orientation::West,
            "east" => Orientation::East,
            _ => return None,
        };
        let controller = match tag.get("ControllerRelative") {
            None => None,
            Some(Tag::IntArray(relative)) => {
                let &[dx, dy, dz] = relative.as_slice() else {
                    return None;
                };
                let origin = position.floor().to_array();
                let range = f64::from(i32::MIN)..=f64::from(i32::MAX);
                if !origin.iter().all(|value| range.contains(value)) {
                    return None;
                }
                let [x, y, z] = origin.map(|v| v as i32);
                Some([x.checked_add(dx)?, y.checked_add(dy)?, z.checked_add(dz)?])
            }
            Some(_) => return None,
        };
        let yaw = spawn_angle(tag.get("ForceYaw").or_else(|| tag.get("Yaw")))?;
        let pitch = spawn_angle(tag.get("Pitch"))?;
        Some(Self {
            orientation,
            controller,
            yaw,
            pitch,
        })
    }

    pub(super) fn local_forward(self) -> Vec3 {
        match self.orientation {
            Orientation::Down => Vec3::NEG_Y,
            Orientation::Up => Vec3::Y,
            Orientation::North => Vec3::NEG_Z,
            Orientation::South => Vec3::Z,
            Orientation::West => Vec3::NEG_X,
            Orientation::East => Vec3::X,
        }
    }

    /// Convert the block entity's sampled `CannonYaw` and `CannonPitch` to
    /// the entity's raw angles (`CannonMountBlockEntity.applyRotation`).
    pub(in crate::game) fn controller_angles(self, yaw: f32, pitch: f32) -> (f32, f32) {
        let pitch = match self.orientation {
            Orientation::East | Orientation::North | Orientation::Down => pitch,
            Orientation::West | Orientation::South | Orientation::Up => -pitch,
        };
        (yaw, pitch)
    }

    /// Transform contraption-local blocks relative to the entity's network
    /// position, using `PitchOrientedContraptionEntityMixin.applyLocalTransforms`.
    /// The entity is at the first block's bottom center.
    pub(in crate::game) fn transform(self, yaw: f32, pitch: f32) -> Mat4 {
        let yaw = (self.orientation.initial_yaw() - yaw.rem_euclid(360.0)).to_radians();
        let pitch = pitch.rem_euclid(360.0).to_radians();
        let tilt = match self.orientation {
            Orientation::East | Orientation::West => Mat4::from_rotation_z(pitch),
            _ => Mat4::from_rotation_x(pitch),
        };
        Mat4::from_translation(Vec3::Y * 0.5)
            * Mat4::from_rotation_y(yaw)
            * tilt
            * Mat4::from_translation(Vec3::splat(-0.5))
    }
}

fn spawn_angle(tag: Option<&Tag>) -> Option<f32> {
    match tag {
        Some(Tag::Float(bits)) => {
            let value = f32::from_bits(*bits);
            value.is_finite().then_some(value)
        }
        None => Some(0.0),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(snbt: &str, position: DVec3) -> Option<PitchContraptionPose> {
        let (tag, _) = horizon_world::nbt::parse_snbt(snbt).unwrap();
        PitchContraptionPose::from_spawn(&tag, position)
    }

    #[test]
    fn kinetic_pitch_contraption_applies_forced_yaw_and_floors_controller_origin() {
        let pose = spawn(
            r#"{InitialOrientation:"DOWN",Yaw:160f,ForceYaw:180f,Pitch:-20f,ControllerRelative:[I;1,2,-2]}"#,
            DVec3::new(-2.25, 64.0, -0.01),
        )
        .unwrap();
        assert_eq!(pose.controller, Some([-2, 66, -3]));
        assert_eq!((pose.yaw, pose.pitch), (180.0, -20.0));
        assert_eq!(pose.controller_angles(45.0, 15.0), (45.0, 15.0));
    }

    #[test]
    fn kinetic_pitch_contraption_keeps_a_packet_without_controller_self_contained() {
        let pose = spawn(
            r#"{InitialOrientation:"west",Yaw:90f,Pitch:0f}"#,
            DVec3::ZERO,
        )
        .unwrap();
        assert_eq!(pose.controller, None);
        let corner = pose
            .transform(pose.yaw, pose.pitch)
            .transform_point3(Vec3::ZERO);
        assert!(corner.abs_diff_eq(Vec3::new(-0.5, 0.0, -0.5), 1e-6));
        assert_eq!(pose.controller_angles(45.0, 15.0), (45.0, -15.0));
        assert!(
            spawn(
                r#"{InitialOrientation:"invalid",Yaw:0f,Pitch:0f}"#,
                DVec3::ZERO
            )
            .is_none()
        );
        assert!(
            spawn(
                r#"{InitialOrientation:"north",ControllerRelative:[I;1,2]}"#,
                DVec3::ZERO
            )
            .is_none()
        );
    }

    #[test]
    fn kinetic_pitch_contraption_rotates_about_its_first_block_center() {
        let east = spawn(
            r#"{InitialOrientation:"east",Yaw:270f,Pitch:90f}"#,
            DVec3::ZERO,
        )
        .unwrap();
        let north = spawn(
            r#"{InitialOrientation:"north",Yaw:180f,Pitch:90f}"#,
            DVec3::ZERO,
        )
        .unwrap();
        let center = Vec3::splat(0.5);
        let raised = Vec3::Y * 1.5;
        for (pose, forward) in [(east, Vec3::X), (north, Vec3::NEG_Z)] {
            let transform = pose.transform(pose.yaw, pose.pitch);
            assert!(
                transform
                    .transform_point3(center)
                    .abs_diff_eq(Vec3::Y * 0.5, 1e-6)
            );
            assert!(
                transform
                    .transform_point3(center + forward)
                    .abs_diff_eq(raised, 1e-6)
            );
        }
        assert!(
            east.transform(180.0, 90.0)
                .transform_vector3(Vec3::Y)
                .abs_diff_eq(Vec3::Z, 1e-6)
        );
    }
}
