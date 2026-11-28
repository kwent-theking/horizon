//! Create's speed and stress gauges and cuckoo clocks: the parts their block
//! entity renderers draw besides the turning shaft (`GaugeRenderer`,
//! `CuckooClockRenderer`).
use glam::{Mat4, Quat, Vec3};
use std::f32::consts::FRAC_PI_2;

const GAUGE_DIAL: &str = "create:block/gauge/dial";
const SPEED_HEAD: &str = "create:block/gauge/speedometer/head";
const STRESS_HEAD: &str = "create:block/gauge/stressometer/head";
const HOUR_HAND: &str = "create:block/cuckoo_clock/hour_hand";
const MINUTE_HAND: &str = "create:block/cuckoo_clock/minute_hand";
/// The dial's hinge, on the model's east side (`dialPivot`).
const DIAL_PIVOT: f32 = 5.75 / 16.0;
/// The hands' hub, on the model's west side.
const HAND_PIVOT: Vec3 = Vec3::new(2.0 / 16.0, 6.0 / 16.0, 8.0 / 16.0);
const HORIZONTAL: [&str; 4] = ["north", "south", "west", "east"];

/// `DirectionalAxisKineticBlock.getRotationAxis`: a gauge's shaft runs across
/// its facing, along the first remaining axis or the other one.
pub(super) fn directional_axis(facing: &str, along_first: bool) -> Option<&'static str> {
    Some(match (axis_of(facing)?, along_first) {
        ("x", true) => "y",
        ("x", false) => "z",
        ("y", true) => "x",
        ("y", false) => "z",
        ("z", true) => "x",
        ("z", false) => "y",
        _ => return None,
    })
}

fn axis_of(direction: &str) -> Option<&'static str> {
    match direction {
        "east" | "west" => Some("x"),
        "up" | "down" => Some("y"),
        "north" | "south" => Some("z"),
        _ => None,
    }
}

/// `Direction.toYRot` of a horizontal direction, in degrees.
fn y_rot(face: &str) -> Option<f32> {
    match face {
        "south" => Some(0.0),
        "west" => Some(90.0),
        "north" => Some(180.0),
        "east" => Some(270.0),
        _ => None,
    }
}

/// A turn of `degrees` about the vertical through the block's centre.
fn centred_y(degrees: f32) -> Mat4 {
    Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_quat(Quat::from_rotation_y(degrees.to_radians()))
        * Mat4::from_translation(Vec3::splat(-0.5))
}

/// `GaugeBlock.shouldRenderHeadOnFace` without its neighbour test: the
/// horizontal faces across the shaft, never the back, and only the front
/// for a gauge whose shaft stands upright.
pub(super) fn gauge_faces(facing: &str, axis: &str) -> Vec<&'static str> {
    let back = super::opposite_facing(facing);
    HORIZONTAL
        .into_iter()
        .filter(|&face| {
            face != back && axis_of(face) != Some(axis) && (axis != "y" || face == facing)
        })
        .collect()
}

/// `rotateBufferTowards`: the models' east side turned to `face`.
fn towards(face: &str) -> Option<Mat4> {
    Some(centred_y(-y_rot(face)? - 90.0))
}

/// The dial on `face`, swung up by `progress` (0 at rest, 1 a quarter turn).
pub(super) fn dial_transform(face: &str, progress: f32) -> Option<Mat4> {
    let hinge = Vec3::new(0.0, DIAL_PIVOT, DIAL_PIVOT);
    Some(
        towards(face)?
            * Mat4::from_translation(hinge)
            * Mat4::from_quat(Quat::from_rotation_x(-FRAC_PI_2 * progress))
            * Mat4::from_translation(-hinge),
    )
}

/// `CuckooClockBlockEntity.tick`'s targets in an overworld-like dimension:
/// the hour hand in whole hours, 30° each, the minute hand 6° a minute.
pub(super) fn clock_hand_angles(day_time: i64) -> (f32, f32) {
    let day = day_time.rem_euclid(24000) as i32;
    let hours = (day / 1000 + 6) % 24;
    let minutes = (day % 1000) * 60 / 1000;
    ((30 * (hours % 12)) as f32, (6 * minutes) as f32)
}

/// A hand at `degrees` on a clock facing `facing`. The model draws the hands
/// on its west side and the clock's face looks along `facing`, so they are
/// turned by `horizontalAngle(facing.getCounterClockWise())` first.
pub(super) fn hand_transform(facing: &str, degrees: f32) -> Option<Mat4> {
    let turn = match facing {
        "north" => -90.0,
        "east" => 180.0,
        "south" => 90.0,
        "west" => 0.0,
        _ => return None,
    };
    Some(
        centred_y(turn)
            * Mat4::from_translation(HAND_PIVOT)
            * Mat4::from_quat(Quat::from_rotation_x(degrees.to_radians()))
            * Mat4::from_translation(-HAND_PIVOT),
    )
}

/// The block-local parts of `block`'s gauge heads and dial or clock hands.
/// `dial` is the gauge's `Value`; a clock without speed shows noon, where a
/// clock that was never powered starts.
pub(super) fn instrument_parts(
    block: &str,
    facing: &str,
    along_first: bool,
    dial: f32,
    day_time: i64,
    speed: f32,
) -> Vec<(&'static str, Mat4)> {
    let head = match block {
        "create:speedometer" => SPEED_HEAD,
        "create:stressometer" => STRESS_HEAD,
        "create:cuckoo_clock" | "create:mysterious_cuckoo_clock" => {
            let (hour, minute) = if speed == 0.0 {
                (0.0, 0.0)
            } else {
                clock_hand_angles(day_time)
            };
            return [(HOUR_HAND, hour), (MINUTE_HAND, minute)]
                .into_iter()
                .filter_map(|(model, angle)| Some((model, hand_transform(facing, angle)?)))
                .collect();
        }
        _ => return Vec::new(),
    };
    let Some(axis) = directional_axis(facing, along_first) else {
        return Vec::new();
    };
    gauge_faces(facing, axis)
        .into_iter()
        .filter_map(|face| {
            Some([
                (head, towards(face)?),
                (GAUGE_DIAL, dial_transform(face, dial)?),
            ])
        })
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn gauge_shaft_axis_follows_create() {
        assert_eq!(directional_axis("east", true), Some("y"));
        assert_eq!(directional_axis("west", false), Some("z"));
        assert_eq!(directional_axis("up", true), Some("x"));
        assert_eq!(directional_axis("down", false), Some("z"));
        assert_eq!(directional_axis("north", true), Some("x"));
        assert_eq!(directional_axis("south", false), Some("y"));
        assert_eq!(directional_axis("sideways", false), None);
    }

    #[test]
    fn gauge_heads_sit_across_the_shaft() {
        // On the floor with the shaft along Z: both sides along X.
        assert_eq!(gauge_faces("up", "z"), ["west", "east"]);
        // On a wall with an upright shaft: only the front.
        assert_eq!(gauge_faces("north", "y"), ["north"]);
        // Facing east with the shaft along Z: never the back (west).
        assert_eq!(gauge_faces("east", "z"), ["east"]);
    }

    #[test]
    fn heads_turn_from_east_to_their_face() {
        let east = towards("east").unwrap();
        assert!(close(
            east.transform_point3(Vec3::new(1.0, 0.5, 0.5)),
            Vec3::new(1.0, 0.5, 0.5)
        ));
        let south = towards("south").unwrap();
        assert!(close(
            south.transform_point3(Vec3::new(1.0, 0.5, 0.5)),
            Vec3::new(0.5, 0.5, 1.0)
        ));
        let north = towards("north").unwrap();
        assert!(close(
            north.transform_point3(Vec3::new(1.0, 0.5, 0.5)),
            Vec3::new(0.5, 0.5, 0.0)
        ));
    }

    #[test]
    fn the_dial_swings_up_from_its_hinge() {
        let hinge = Vec3::new(1.0, DIAL_PIVOT, DIAL_PIVOT);
        let tip = hinge + Vec3::Z * 0.25;
        let rest = dial_transform("east", 0.0).unwrap();
        assert!(close(rest.transform_point3(tip), tip));
        let full = dial_transform("east", 1.0).unwrap();
        assert!(close(full.transform_point3(hinge), hinge));
        assert!(close(full.transform_point3(tip), hinge + Vec3::Y * 0.25));
    }

    #[test]
    fn clock_hands_read_the_day_time() {
        // Day time 0 is six in the morning.
        assert_eq!(clock_hand_angles(0), (180.0, 0.0));
        assert_eq!(clock_hand_angles(6000), (0.0, 0.0));
        assert_eq!(clock_hand_angles(1500), (210.0, 180.0));
        assert_eq!(clock_hand_angles(24000 + 1500), (210.0, 180.0));
    }

    #[test]
    fn clock_hands_sit_on_the_clock_face() {
        // The model's front (its dial at z = 1.75/16) looks north; the hands
        // drawn on its west side must land there.
        let hub = hand_transform("north", 0.0)
            .unwrap()
            .transform_point3(HAND_PIVOT);
        assert!(close(hub, Vec3::new(0.5, 6.0 / 16.0, 2.0 / 16.0)));
        let west = hand_transform("west", 0.0)
            .unwrap()
            .transform_point3(HAND_PIVOT);
        assert!(close(west, HAND_PIVOT));
        let east = hand_transform("east", 0.0)
            .unwrap()
            .transform_point3(HAND_PIVOT);
        assert!(close(east, Vec3::new(14.0 / 16.0, 6.0 / 16.0, 0.5)));
        // A quarter turn points the hand from twelve to three as seen from the front.
        let up = HAND_PIVOT + Vec3::Y * 0.1;
        let three = hand_transform("west", 90.0).unwrap().transform_point3(up);
        assert!(close(three, HAND_PIVOT + Vec3::Z * 0.1));
    }

    #[test]
    fn instruments_list_their_parts() {
        let speedometer = instrument_parts("create:speedometer", "up", false, 0.5, 0, 16.0);
        let models: Vec<_> = speedometer.iter().map(|(model, _)| *model).collect();
        assert_eq!(models, [SPEED_HEAD, GAUGE_DIAL, SPEED_HEAD, GAUGE_DIAL]);
        let clock = instrument_parts("create:cuckoo_clock", "north", false, 0.0, 0, 16.0);
        assert_eq!(clock.len(), 2);
        assert!(instrument_parts("create:shaft", "north", false, 0.0, 0, 16.0).is_empty());
    }
}
