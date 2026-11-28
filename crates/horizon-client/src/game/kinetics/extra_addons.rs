//! Additional client-only parts for Create add-ons.

use super::machines::{MachinePart, centred, part, spin};
use super::{
    angle_radians, axis_index, axis_y_rotation, facing_z_rotation, opposite_facing, rotation_offset,
};
use glam::{Mat4, Quat, Vec3};
use horizon_client::world::{CreateCannonMountState, LerpedValue};
use horizon_world::nbt::{JavaString, Tag};
use std::f32::consts::FRAC_PI_2;

pub(super) const ORE_KINETIC_INPUT: &str = "createoreexcavation:kinetic_input";
pub(super) const PRINTER: &str = "create_enchantment_industry:printer";
pub(super) const CARBON_BRUSHES: &str = "create_new_age:carbon_brushes";
pub(in crate::game) const CANNON_MOUNT: &str = "createbigcannons:cannon_mount";

const PRINTER_TICKS: i32 = 50;
const PRINTER_PISTON_TICKS: f32 = 5.0;
const SHAFT: &str = "create:block/shaft";
const SHAFT_HALF: &str = "create:block/shaft_half";

fn facing_axis(facing: &str) -> Option<&'static str> {
    Some(match facing {
        "west" | "east" => "x",
        "down" | "up" => "y",
        "north" | "south" => "z",
        _ => return None,
    })
}

fn shaft_spin(axis: &str, speed: f32, time: f64, pos: [i32; 3]) -> Option<Mat4> {
    let offset = rotation_offset(axis_index(axis)?, pos);
    Some(spin(axis, angle_radians(time, speed, offset)))
}

pub(super) fn cannon_mount_from_nbt(
    previous: Option<&CreateCannonMountState>,
    tag: &Tag,
) -> Option<CreateCannonMountState> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    let running = matches!(
        fields.get(&JavaString::from("Running")),
        Some(Tag::Byte(running)) if *running != 0
    );
    let angle = |key: &str| match fields.get(&JavaString::from(key)) {
        Some(Tag::Float(bits)) => {
            let value = f32::from_bits(*bits);
            value.is_finite().then_some(value % 360.0)
        }
        _ => None,
    };
    let rpm = |key: &str| {
        fields
            .get(&JavaString::from(key))
            .and_then(super::speed_from_nbt)
            .filter(|value| value.is_finite())
            .unwrap_or_default()
    };
    let yaw = angle("CannonYaw")?;
    let pitch = angle("CannonPitch")?;
    let mut state = CreateCannonMountState {
        running,
        yaw: LerpedValue::start_with(yaw),
        pitch: LerpedValue::start_with(pitch),
        yaw_rpm: rpm("YawInterface"),
        pitch_rpm: rpm("PitchInterface"),
    };
    if let Some(previous) = previous.filter(|previous| previous.running && running) {
        state.yaw = previous.yaw;
        state.pitch = previous.pitch;
        let turn = (yaw - state.yaw.value + 180.0).rem_euclid(360.0) - 180.0;
        state.yaw.chase_exp(state.yaw.value + turn, 0.5);
        state.pitch.chase_exp(pitch, 0.5);
    }
    Some(state)
}

pub(super) fn cannon_mount_parts(
    facing: &str,
    vertical: &str,
    state: Option<&CreateCannonMountState>,
    time: f64,
    alpha: f64,
    pos: [i32; 3],
) -> [MachinePart; 4] {
    let (idle_yaw, pitch_axis) = match facing {
        "east" => (-90.0, "z"),
        "west" => (90.0, "z"),
        "north" => (180.0, "x"),
        _ => (0.0, "x"),
    };
    let flipped = vertical == "up";
    let offset = if flipped { -Vec3::Y } else { Vec3::Y };
    let sign = if flipped { 1.0 } else { -1.0 };
    let (yaw, pitch) = state
        .filter(|state| state.running)
        .map_or((idle_yaw, 0.0), |state| {
            (state.yaw.get(alpha), state.pitch.get(alpha))
        });
    let mut rotation = Quat::from_rotation_y(sign * yaw.to_radians());
    if flipped {
        rotation = Quat::from_rotation_z(std::f32::consts::PI) * rotation;
    }
    let elevation = Quat::from_rotation_x(sign * pitch.to_radians());
    let (yaw_rpm, pitch_rpm) = state.map_or((0.0, 0.0), |state| (state.yaw_rpm, state.pitch_rpm));
    let yaw_shaft = shaft_spin("y", yaw_rpm, time, pos).unwrap_or(Mat4::IDENTITY);
    let pitch_shaft = shaft_spin(pitch_axis, pitch_rpm, time, pos).unwrap_or(Mat4::IDENTITY);
    [
        part(
            "createbigcannons:block/cannon_mount/rotating_mount",
            Mat4::from_translation(offset) * centred(rotation),
        ),
        part(
            "createbigcannons:block/cannon_carriage/carriage_axle",
            Mat4::from_translation(offset * 2.0) * centred(rotation * elevation),
        ),
        part(SHAFT, pitch_shaft * centred(axis_y_rotation(pitch_axis))),
        part(SHAFT_HALF, yaw_shaft * centred(facing_z_rotation(vertical))),
    ]
}

/// Create Ore Excavation's `KineticInputBlockEntityRenderer` uses the shaft
/// face for both the partial model orientation and the rotation axis.
pub(super) fn ore_kinetic_input_part(
    shaft: &str,
    speed: f32,
    time: f64,
    pos: [i32; 3],
) -> Option<MachinePart> {
    let rotation = shaft_spin(facing_axis(shaft)?, speed, time, pos)?;
    Some(part(
        SHAFT_HALF,
        rotation * centred(facing_z_rotation(shaft)),
    ))
}

/// New Age's `CarbonBrushesRenderer` draws a full Create shaft and its own coil.
pub(super) fn carbon_brush_parts(
    facing: &str,
    speed: f32,
    time: f64,
    pos: [i32; 3],
) -> Option<[MachinePart; 2]> {
    let axis = facing_axis(facing)?;
    let rotation = shaft_spin(axis, speed, time, pos)?;
    let shaft_orientation = axis_y_rotation(axis);
    let coil_orientation = if axis == "x" {
        Quat::from_rotation_z(FRAC_PI_2)
    } else {
        shaft_orientation
    };
    Some([
        part(SHAFT, rotation * centred(shaft_orientation)),
        part(
            "create_new_age:block/carbon_brushes/coil",
            rotation * centred(coil_orientation),
        ),
    ])
}

pub(super) fn addition_machine_parts(
    block: &str,
    facing: &str,
    speed: f32,
    time: f64,
    pos: [i32; 3],
) -> Option<[MachinePart; 2]> {
    match block {
        "createaddition:alternator" => {
            // AlternatorVisual orients the two halves toward opposite faces.
            let rotation = shaft_spin(facing_axis(facing)?, speed, time, pos)?;
            Some(
                [facing, opposite_facing(facing)]
                    .map(|face| part(SHAFT_HALF, rotation * centred(facing_z_rotation(face)))),
            )
        }
        "createaddition:rolling_mill" => {
            // RollingMillVisual stacks the same shaft model at two heights.
            let axis = match facing {
                "north" | "south" => "z",
                "east" | "west" => "x",
                _ => return None,
            };
            let orientation = centred(axis_y_rotation(axis));
            let lower = shaft_spin(axis, speed, time, pos)? * orientation;
            let upper = Mat4::from_translation(Vec3::Y * (4.0 / 16.0))
                * shaft_spin(axis, -speed, time, pos)?
                * orientation;
            Some([part(SHAFT, lower), part(SHAFT, upper)])
        }
        _ => None,
    }
}

/// CEI's `PrinterBlockEntity` writes this countdown in both save and update tags.
pub(super) fn printer_ticks_from_nbt(tag: &Tag) -> Option<i32> {
    let Tag::Int(ticks) = tag.get("ProcessingTicks")? else {
        return None;
    };
    (-1..=PRINTER_TICKS).contains(ticks).then_some(*ticks)
}

/// Both printer nozzles rise, with the second translation accumulating, while
/// the piston descends. All three parts are centred on the vertical axis.
pub(super) fn printer_parts(ticks: Option<i32>, alpha: f64) -> [MachinePart; 3] {
    let ticks = ticks.unwrap_or(-1) as f32 - alpha as f32;
    let progress = (ticks.min(PRINTER_TICKS as f32 - ticks) / PRINTER_PISTON_TICKS).clamp(0.0, 1.0);
    [
        (
            "create_enchantment_industry:block/printer/nozzle_top",
            3.0 / 32.0,
        ),
        (
            "create_enchantment_industry:block/printer/nozzle_bottom",
            6.0 / 32.0,
        ),
        ("create_enchantment_industry:block/printer/piston", -0.5),
    ]
    .map(|(model, travel)| part(model, Mat4::from_translation(Vec3::Y * (travel * progress))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cannon_mount_reads_separate_rpm_and_chases_the_shortest_heading() {
        let tag = |yaw: f32| {
            horizon_world::nbt::parse_snbt(&format!(
            "{{Running:1b,CannonYaw:{yaw}f,CannonPitch:25f,YawInterface:{{Speed:32f}},PitchInterface:{{Speed:-8f}}}}"
        )).unwrap().0
        };
        let first = cannon_mount_from_nbt(None, &tag(359.0)).unwrap();
        assert_eq!((first.yaw_rpm, first.pitch_rpm), (32.0, -8.0));
        assert_eq!(first.pitch.value, 25.0);
        let mut second = cannon_mount_from_nbt(Some(&first), &tag(1.0)).unwrap();
        assert_eq!(second.yaw.target, 361.0);
        second.yaw.tick();
        assert_eq!(second.yaw.value, 360.0);
    }

    #[test]
    fn cannon_mount_offsets_follow_mount_direction() {
        let center = Vec3::splat(0.5);
        let tip = center + Vec3::Z * 0.5;
        for (vertical, dir) in [("down", Vec3::Y), ("up", Vec3::NEG_Y)] {
            let [mount, axle, _, shaft] =
                cannon_mount_parts("east", vertical, None, 0.0, 1.0, [0, 0, 0]);
            assert!(
                mount
                    .local
                    .transform_point3(center)
                    .abs_diff_eq(center + dir, 1e-6)
            );
            assert!(
                axle.local
                    .transform_point3(center)
                    .abs_diff_eq(center + 2.0 * dir, 1e-6)
            );
            assert!(
                shaft
                    .local
                    .transform_point3(tip)
                    .abs_diff_eq(center - dir * 0.5, 1e-6)
            );
        }
    }

    #[test]
    fn ore_kinetic_input_shaft_reaches_the_selected_face() {
        let center = Vec3::splat(0.5);
        for (shaft, normal) in [
            ("down", Vec3::NEG_Y),
            ("up", Vec3::Y),
            ("north", Vec3::NEG_Z),
            ("south", Vec3::Z),
            ("west", Vec3::NEG_X),
            ("east", Vec3::X),
        ] {
            let part = ore_kinetic_input_part(shaft, 8.0, 10.0, [0, 0, 0]).unwrap();
            assert_eq!(part.model, "create:block/shaft_half");
            let end = part.local.transform_point3(center + Vec3::Z * 0.5);
            assert!(end.abs_diff_eq(center + normal * 0.5, 1e-6), "{shaft}");
        }
        assert!(ore_kinetic_input_part("sideways", 0.0, 0.0, [0, 0, 0]).is_none());
    }

    #[test]
    fn opposite_input_faces_spin_around_the_same_positive_axis() {
        let model_radial = Vec3::new(0.625, 0.5, 0.5);
        let south = ore_kinetic_input_part("south", 8.0, 10.0, [0, 0, 0]).unwrap();
        let north = ore_kinetic_input_part("north", 8.0, 10.0, [0, 0, 0]).unwrap();
        let south_radial = south.local.transform_point3(model_radial);
        let north_radial = north.local.transform_point3(model_radial);
        assert!(south_radial.y > 0.5);
        assert!(north_radial.y < 0.5);
    }

    #[test]
    fn carbon_brushes_emit_a_full_shaft_and_coil_on_the_facing_axis() {
        let center = Vec3::splat(0.5);
        let model_endpoint = center + Vec3::Y * 0.5;
        for (facing, axis) in [("north", Vec3::Z), ("down", Vec3::Y), ("east", Vec3::X)] {
            let [shaft, coil] = carbon_brush_parts(facing, 32.0, 8.0, [1, 1, 1]).unwrap();
            assert_eq!(shaft.model, "create:block/shaft");
            assert_eq!(coil.model, "create_new_age:block/carbon_brushes/coil");
            for part in [shaft, coil] {
                let tip = part.local.transform_point3(model_endpoint) - center;
                assert!(tip.abs().abs_diff_eq(axis * 0.5, 1e-6), "{facing}");
            }
        }
    }

    #[test]
    fn alternator_has_a_half_shaft_toward_both_ends_of_its_axis() {
        let center = Vec3::splat(0.5);
        let tip = center + Vec3::Z * 0.5;
        let [front, rear] =
            addition_machine_parts("createaddition:alternator", "up", 8.0, 10.0, [0, 0, 0])
                .unwrap();
        assert_eq!(front.model, "create:block/shaft_half");
        assert_eq!(rear.model, "create:block/shaft_half");
        assert!(
            front
                .local
                .transform_point3(tip)
                .abs_diff_eq(center + Vec3::Y * 0.5, 1e-6)
        );
        assert!(
            rear.local
                .transform_point3(tip)
                .abs_diff_eq(center - Vec3::Y * 0.5, 1e-6)
        );
    }

    #[test]
    fn rolling_mill_has_stacked_counterrotating_full_shafts() {
        let [lower, upper] =
            addition_machine_parts("createaddition:rolling_mill", "north", 8.0, 10.0, [1, 0, 0])
                .unwrap();
        let center = Vec3::splat(0.5);
        let radial = center + Vec3::X * 0.125;
        for part in [&lower, &upper] {
            assert_eq!(part.model, "create:block/shaft");
            let direction = part.local.transform_vector3(Vec3::Y);
            assert!(direction.abs_diff_eq(Vec3::Z, 1e-6));
        }
        assert!(
            lower
                .local
                .transform_point3(center)
                .abs_diff_eq(center, 1e-6)
        );
        let upper_center = center + Vec3::Y * 0.25;
        assert!(
            upper
                .local
                .transform_point3(center)
                .abs_diff_eq(upper_center, 1e-6)
        );
        assert!(lower.local.transform_point3(radial).y > center.y);
        assert!(upper.local.transform_point3(radial).y < upper_center.y);
    }

    #[test]
    fn printer_reads_its_signed_countdown_from_a_regular_block_entity_tag() {
        let update = |ticks| Tag::compound([("ProcessingTicks", ticks)]);
        assert_eq!(printer_ticks_from_nbt(&update(Tag::Int(50))), Some(50));
        assert_eq!(printer_ticks_from_nbt(&update(Tag::Int(-1))), Some(-1));
        assert_eq!(printer_ticks_from_nbt(&update(Tag::Int(51))), None);
        assert_eq!(
            printer_ticks_from_nbt(&update(Tag::Float(5.0_f32.to_bits()))),
            None
        );
        assert_eq!(printer_ticks_from_nbt(&Tag::Int(5)), None);
    }

    #[test]
    fn printer_extends_then_retracts_three_independent_models() {
        let idle = printer_parts(None, 0.0);
        let midstroke = printer_parts(Some(50), 0.5);
        let pressed = printer_parts(Some(25), 0.0);
        let retracting = printer_parts(Some(3), 0.5);
        let finished = printer_parts(Some(-1), 0.0);

        for part in idle.iter().chain(&finished) {
            assert_eq!(part.local, Mat4::IDENTITY);
        }
        assert_eq!(
            pressed[0].model,
            "create_enchantment_industry:block/printer/nozzle_top"
        );
        assert_eq!(
            pressed[1].model,
            "create_enchantment_industry:block/printer/nozzle_bottom"
        );
        assert_eq!(
            pressed[2].model,
            "create_enchantment_industry:block/printer/piston"
        );
        for (part, offset) in pressed.iter().zip([3.0 / 32.0, 6.0 / 32.0, -0.5]) {
            assert_eq!(part.local.transform_point3(Vec3::ZERO), Vec3::Y * offset);
        }
        assert_eq!(midstroke[2].local.transform_point3(Vec3::ZERO).y, -0.05);
        assert_eq!(retracting[2].local.transform_point3(Vec3::ZERO).y, -0.25);
    }
}
