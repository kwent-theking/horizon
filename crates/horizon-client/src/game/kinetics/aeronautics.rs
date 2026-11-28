//! Ordinary block-entity parts from Create Aeronautics 1.3.2 and its bundled mods.

use super::instruments::directional_axis;
use super::machines::{MachinePart, centred, part};
use super::{
    DEGREES_PER_TICK_PER_RPM, angle_radians, axis_index, axis_y_rotation, facing_z_rotation,
    has_kinetic_speed, opposite_facing, rotation_offset, speed_from_nbt, spin_sign,
};
use glam::{Mat4, Quat, Vec3};
use horizon_client::world::AeronauticsPartPose;
use horizon_world::nbt::Tag;
use std::f32::consts::{FRAC_PI_2, PI};

pub(super) const WOODEN: &str = "aeronautics:wooden_propeller";
pub(super) const ANDESITE: &str = "aeronautics:andesite_propeller";
pub(super) const SMART: &str = "aeronautics:smart_propeller";
pub(super) const BEARING: &str = "aeronautics:propeller_bearing";
pub(super) const ENVELOPE_BE: &str = "aeronautics:envelope_encased_shaft";
pub(super) const ANALOG: &str = "simulated:analog_transmission";
pub(super) const AUGER_COG: &str = "simulated:auger_cog";
pub(super) const AUGER_BE: &str = "simulated:auger_shaft";
pub(super) const WINCH: &str = "simulated:rope_winch";
pub(super) const SPRING: &str = "simulated:torsion_spring";
pub(super) const SWIVEL: &str = "simulated:swivel_bearing";
pub(super) const BOREHEAD: &str = "offroad:borehead_bearing";

const SHAFT: &str = "create:block/shaft";
const SHAFT_HALF: &str = "create:block/shaft_half";

#[derive(Clone, Copy)]
pub(super) enum Part {
    Wooden,
    Andesite,
    Smart,
    Bearing,
    EnvelopeShaft,
    Analog,
    AugerCog,
    Winch,
    Spring,
    Swivel,
    Borehead,
}

pub(super) fn is_kinetic_be(name: &str) -> bool {
    matches!(
        name,
        WOODEN
            | ANDESITE
            | SMART
            | BEARING
            | ENVELOPE_BE
            | ANALOG
            | AUGER_BE
            | WINCH
            | SPRING
            | SWIVEL
            | BOREHEAD
            | "simulated:simple"
    )
}

pub(super) fn has_pose_tag(name: &str, tag: &Tag) -> bool {
    let kind = if name == "simulated:simple" {
        Some(Part::Analog)
    } else {
        Part::for_block(name)
    };
    kind.is_some_and(|kind| kind.read_pose(None, tag, 0, 0.0).is_some())
}

impl Part {
    pub(super) fn for_block(name: &str) -> Option<Self> {
        Some(match name {
            WOODEN => Self::Wooden,
            ANDESITE => Self::Andesite,
            SMART => Self::Smart,
            BEARING => Self::Bearing,
            ANALOG => Self::Analog,
            AUGER_COG => Self::AugerCog,
            WINCH => Self::Winch,
            SPRING => Self::Spring,
            SWIVEL => Self::Swivel,
            BOREHEAD => Self::Borehead,
            _ if name.starts_with("aeronautics:") && name.ends_with("_envelope_encased_shaft") => {
                Self::EnvelopeShaft
            }
            _ => return None,
        })
    }

    pub(super) fn frustum_pad(self) -> f32 {
        match self {
            // The wooden blades reach x=-13..29. The smart rotor sits above
            // the block, so allow for its extension beyond any face.
            Self::Wooden | Self::Andesite | Self::Smart => 1.0,
            Self::Analog | Self::AugerCog | Self::Swivel => 1.0 / 16.0,
            _ => 0.0,
        }
    }

    pub(super) fn keeps_pose(self, pose: AeronauticsPartPose) -> bool {
        use AeronauticsPartPose::*;
        matches!(
            (self, pose),
            (Self::Wooden | Self::Andesite | Self::Smart, Rotor { .. })
                | (Self::Bearing | Self::Borehead, Bearing { .. })
                | (Self::Analog, CogSpeed(_))
                | (Self::Swivel, SwivelCogSpeed(_))
                | (Self::Spring, SpringAngle(_))
        )
    }

    pub(super) fn read_pose(
        self,
        previous: Option<AeronauticsPartPose>,
        tag: &Tag,
        tick: i64,
        rpm: f32,
    ) -> Option<AeronauticsPartPose> {
        use AeronauticsPartPose::*;
        let previous = previous.filter(|&pose| self.keeps_pose(pose));
        match self {
            Self::Wooden | Self::Andesite | Self::Smart => {
                // airpressure/miniprop.nbt pairs Speed:64f with RotationSpeed:19.199995f.
                let rate = degrees(tag, "RotationSpeed").or_else(|| {
                    has_kinetic_speed(tag).then_some(rpm * DEGREES_PER_TICK_PER_RPM as f32)
                });
                let Some(degrees_per_tick) = rate.filter(|rate| rate.is_finite()) else {
                    return previous;
                };
                Some(Rotor {
                    angle: angle_at(previous, tick as f64),
                    degrees_per_tick,
                    tick,
                })
            }
            Self::Bearing | Self::Borehead => {
                let Some(angle) = degrees(tag, "Angle") else {
                    return previous;
                };
                let running = flag(tag, "Running") && !flag(tag, "Stalled");
                Some(Bearing {
                    angle,
                    degrees_per_tick: if running {
                        degrees(tag, "RotationSpeed").unwrap_or_default()
                    } else {
                        0.0
                    },
                    tick,
                })
            }
            Self::Analog => tag
                .get("ExtraCogwheel")
                .and_then(speed_from_nbt)
                .filter(|rpm| rpm.is_finite())
                .map(CogSpeed)
                .or(previous),
            Self::Swivel => tag
                .get("SwivelCog")
                .and_then(speed_from_nbt)
                .filter(|rpm| rpm.is_finite())
                .map(SwivelCogSpeed)
                .or(previous),
            Self::Spring => tag
                .get("TorsionSpringOutput")
                .or_else(|| tag.get("Internal")) // older Ponder structure
                .and_then(|output| degrees(output, "Angle"))
                .map(SpringAngle)
                .or(previous),
            _ => None,
        }
    }

    pub(super) fn parts<'a>(
        self,
        prop: impl Fn(&str) -> Option<&'a str>,
        rpm: f32,
        pose: Option<AeronauticsPartPose>,
        time: f64,
        pos: [i32; 3],
    ) -> impl IntoIterator<Item = MachinePart> {
        let facing = prop("facing").unwrap_or("up");
        let axis = prop("axis").unwrap_or("y");
        let along_first = prop("axis_along_first") == Some("true");
        let rpm = if rpm.is_finite() { rpm } else { 0.0 };
        let spin = |mount, local_axis, rpm, phase| {
            spun(mount, local_axis, angle_radians(time, rpm, phase))
        };
        let shaft = |axis| part(SHAFT, spin(axis_y_rotation(axis), Vec3::Y, rpm, 0.0));
        let shaft_half = || {
            part(
                SHAFT_HALF,
                spin(
                    facing_z_rotation(opposite_facing(facing)),
                    Vec3::Z,
                    rpm,
                    0.0,
                ),
            )
        };
        let mut shaft_input = None;
        let (main, extra) = match self {
            Self::Wooden | Self::Andesite | Self::Smart => {
                let reversed = prop("reversed") == Some("true");
                let model = match (self, reversed) {
                    (Self::Wooden, false) => "aeronautics:block/wooden_propeller/propeller",
                    (Self::Wooden, true) => "aeronautics:block/wooden_propeller/propeller_reversed",
                    (Self::Andesite, false) => "aeronautics:block/andesite_propeller/propeller",
                    (Self::Andesite, true) => {
                        "aeronautics:block/andesite_propeller/propeller_reversed"
                    }
                    (_, false) => "aeronautics:block/smart_propeller/propeller",
                    (_, true) => "aeronautics:block/smart_propeller/propeller_reversed",
                };
                let angle = pose.map_or_else(
                    || angle_radians(time, rpm, 0.0),
                    |pose| angle_at(Some(pose), time).to_radians(),
                );
                if matches!(self, Self::Smart) {
                    // The item model's hinge pivot is [8,8,6] in its
                    // north-facing view. Unpitching that model places the
                    // hub 11 pixels higher than the standalone rotor model.
                    let x = if prop("ceiling") == Some("true") {
                        PI
                    } else {
                        0.0
                    };
                    let y = if axis == "x" { FRAC_PI_2 } else { 0.0 };
                    let mount = block_rotation(x, y);
                    shaft_input = Some(part(
                        SHAFT_HALF,
                        spin(mount * Quat::from_rotation_x(FRAC_PI_2), Vec3::Z, rpm, 0.0),
                    ));
                    let rotor = spun(mount, Vec3::Y, angle)
                        * Mat4::from_translation(Vec3::Y * (11.0 / 16.0));
                    (
                        part(model, rotor),
                        Some(part(
                            "aeronautics:block/smart_propeller/hinge",
                            centred(mount),
                        )),
                    )
                } else {
                    (part(model, spun(up_mount(facing), Vec3::Y, angle)), None)
                }
            }
            Self::Bearing => (
                part(
                    "aeronautics:block/propeller_bearing/bearing_plate",
                    spun(up_mount(facing), Vec3::Y, angle_at(pose, time).to_radians()),
                ),
                Some(shaft_half()),
            ),
            Self::EnvelopeShaft => (shaft(axis), None),
            Self::Analog | Self::AugerCog => {
                let base = axis_y_rotation(axis);
                let phase = axis_index(axis).map_or(0.0, |axis| rotation_offset(axis, pos));
                if matches!(self, Self::Analog) {
                    let cog_rpm = match pose {
                        Some(AeronauticsPartPose::CogSpeed(cog_rpm)) => cog_rpm,
                        _ if prop("powered") != Some("true") => rpm,
                        _ => 0.0,
                    };
                    (
                        shaft(axis),
                        Some(part(
                            "simulated:block/analog_transmission/gear",
                            spin(base, Vec3::Y, cog_rpm, phase),
                        )),
                    )
                } else {
                    (
                        part(
                            "simulated:block/auger_shaft/cog",
                            spin(base, Vec3::Y, rpm, phase),
                        ),
                        None,
                    )
                }
            }
            Self::Winch => {
                // The spool and shaft in the horizontal casing run along X.
                // The vertical casing rolls that axle a quarter turn around Z.
                let mut mount = facing_z_rotation(facing);
                let axis = directional_axis(facing, along_first).unwrap_or("x");
                if axis == "y" {
                    mount *= Quat::from_rotation_z(FRAC_PI_2);
                } else if matches!(facing, "up" | "down") && !along_first {
                    mount = Quat::from_rotation_y(-FRAC_PI_2) * mount;
                }
                let transform = spin(mount, Vec3::X, rpm, 0.0);
                (
                    part("simulated:block/rope_winch/shaft", transform),
                    Some(part("simulated:block/rope_winch/rope_coil", transform)),
                )
            }
            Self::Spring => (
                part(
                    "simulated:block/torsion_spring/spring",
                    spun(up_mount(facing), Vec3::Y, angle_at(pose, time).to_radians()),
                ),
                Some(shaft_half()),
            ),
            Self::Swivel => {
                let mount = up_mount(facing);
                let axis = match facing {
                    "east" | "west" => 0,
                    "north" | "south" => 2,
                    _ => 1,
                };
                let phase = rotation_offset(axis, pos);
                let cog_rpm = match pose {
                    Some(AeronauticsPartPose::SwivelCogSpeed(cog_rpm)) => cog_rpm,
                    _ => 0.0,
                };
                (
                    part(SHAFT, spin(mount, Vec3::Y, rpm, 0.0)),
                    Some(part(
                        "simulated:block/swivel_bearing/ironcog",
                        spin(mount, Vec3::Y, cog_rpm, phase),
                    )),
                )
            }
            Self::Borehead => {
                let mount = facing_z_rotation(facing) * Quat::from_rotation_x(FRAC_PI_2);
                (
                    part(
                        "create:block/bearing/top",
                        spun(mount, Vec3::Y, angle_at(pose, time).to_radians()),
                    ),
                    Some(shaft(directional_axis(facing, along_first).unwrap_or("x"))),
                )
            }
        };
        [Some(main), extra, shaft_input].into_iter().flatten()
    }
}

fn flag(tag: &Tag, key: &str) -> bool {
    matches!(tag.get(key), Some(Tag::Byte(value)) if *value != 0)
}

fn degrees(tag: &Tag, key: &str) -> Option<f32> {
    let value = match tag.get(key)? {
        Tag::Float(bits) => f32::from_bits(*bits),
        Tag::Double(bits) => f64::from_bits(*bits) as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn angle_at(pose: Option<AeronauticsPartPose>, time: f64) -> f32 {
    use AeronauticsPartPose::*;
    match pose {
        Some(
            Rotor {
                angle,
                degrees_per_tick,
                tick,
            }
            | Bearing {
                angle,
                degrees_per_tick,
                tick,
            },
        ) => (f64::from(angle) + (time - tick as f64).max(0.0) * f64::from(degrees_per_tick))
            .rem_euclid(360.0) as f32,
        Some(SpringAngle(angle)) => angle,
        _ => 0.0,
    }
}

/// Minecraft applies blockstate rotations about the block centre in -X, -Y order.
fn block_rotation(x: f32, y: f32) -> Quat {
    Quat::from_rotation_y(-y) * Quat::from_rotation_x(-x)
}

fn up_mount(facing: &str) -> Quat {
    match facing {
        "up" => Quat::IDENTITY,
        "down" => block_rotation(PI, 0.0),
        "north" => block_rotation(FRAC_PI_2, 0.0),
        "east" => block_rotation(FRAC_PI_2, FRAC_PI_2),
        "south" => block_rotation(FRAC_PI_2, PI),
        "west" => block_rotation(FRAC_PI_2, 3.0 * FRAC_PI_2),
        _ => Quat::IDENTITY,
    }
}

fn spun(mount: Quat, local_axis: Vec3, angle: f32) -> Mat4 {
    centred(mount * Quat::from_axis_angle(local_axis, angle * spin_sign(mount, local_axis)))
}

#[cfg(test)]
mod tests;
