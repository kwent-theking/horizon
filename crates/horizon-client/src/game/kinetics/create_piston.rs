//! The head and poles of a moving Create 6.0.10 mechanical piston.
//!
//! The piston controller sends the stroke and number of poles in its block entity tag.
//! The placed base, heads and poles use their terrain models. During travel, Create moves
//! the head, poles and attached blocks as a controlled contraption. The controller tag
//! supplies a head-and-pole fallback until the moving entity's block palette
//! arrives over NeoForge's advanced spawn payload.

use super::levers::dir_normal_vec3;
use super::machines::{MachinePart, centred, part};
use glam::{Mat4, Quat, Vec3};
use horizon_client::world::CreatePistonState;
use horizon_client_core::nbt_value::float;
use horizon_world::nbt::Tag;
use std::f32::consts::{FRAC_PI_2, PI};

const PISTON: &str = "create:mechanical_piston";
const STICKY_PISTON: &str = "create:sticky_mechanical_piston";
const HEAD: &str = "create:block/mechanical_piston/normal/head";
const STICKY_HEAD: &str = "create:block/mechanical_piston/sticky/head";
const POLE: &str = "create:block/piston_extension_pole";
const SYNC_INTERVAL: u8 = 3;

pub(super) fn is_piston_block(name: &str) -> bool {
    matches!(name, PISTON | STICKY_PISTON)
}

pub(super) fn is_piston_be_tag(name: &str, tag: &Tag) -> bool {
    name == PISTON
        && matches!(tag.get("ExtensionLength"), Some(Tag::Int(_)))
        && matches!(tag.get("Running"), Some(Tag::Byte(_)))
}

pub(super) fn piston_state_from_nbt(
    previous: Option<&CreatePistonState>,
    tag: &Tag,
) -> Option<CreatePistonState> {
    // MechanicalPistonBlockEntity and LinearActuatorBlockEntity write these fields in updates.
    let (Some(Tag::Int(extension_length)), Some(Tag::Byte(running))) =
        (tag.get("ExtensionLength"), tag.get("Running"))
    else {
        return None;
    };
    let extension_length = *extension_length;
    let running = *running != 0 && extension_length > 0;
    let offset = float(tag.get("Offset")).filter(|offset| offset.is_finite())?;
    let offset = offset.clamp(0.0, f64::from(extension_length.max(0)));
    let previous = previous.filter(|previous| {
        previous.running && running && previous.extension_length == extension_length
    });
    let forced = tag.get("ForceMovement").is_some();
    let previous_offset = if forced {
        offset
    } else {
        previous.map_or(offset, |previous| previous.offset)
    };
    // A repeated authoritative sample can also mean that the contraption is stalled.
    let advancing = previous
        .is_none_or(|previous| (previous.synchronized_offset - offset).abs() > f64::EPSILON);
    let waiting = matches!(tag.get("Waiting"), Some(Tag::Byte(waiting)) if *waiting != 0);
    Some(CreatePistonState {
        extension_length: extension_length.max(0),
        synchronized_offset: offset,
        previous_offset,
        offset,
        running,
        prediction_ticks_left: if running && !waiting && advancing {
            SYNC_INTERVAL
        } else {
            0
        },
        sequenced_offset_limit: float(tag.get("SequencedOffsetLimit"))
            .filter(|limit| limit.is_finite() && *limit >= 0.0),
    })
}

pub(super) fn tick_piston_state(state: &mut CreatePistonState, facing: &str, speed: f32) {
    state.previous_offset = state.offset;
    if state.prediction_ticks_left == 0 {
        return;
    }
    state.prediction_ticks_left -= 1;
    if !speed.is_finite() {
        return;
    }
    let sign = match facing {
        "west" | "down" | "south" => 1.0,
        "east" | "up" | "north" => -1.0,
        _ => return,
    };
    // MechanicalPistonBlockEntity converts rpm to blocks per tick using its facing and stroke.
    let limit = state
        .sequenced_offset_limit
        .map_or(0.49, |limit| limit.min(0.49));
    let step = (sign * f64::from(speed) / 512.0).clamp(-limit, limit);
    let next = (state.offset + step).clamp(0.0, f64::from(state.extension_length));
    if let Some(remaining) = &mut state.sequenced_offset_limit {
        *remaining = (*remaining - (next - state.offset).abs()).max(0.0);
    }
    state.offset = next;
}

fn model_rotation(facing: &str) -> Quat {
    // Both placed part blockstates use the same negative X-then-Y quarter turns.
    let yaw = match facing {
        "up" => return Quat::IDENTITY,
        "down" => return Quat::from_rotation_x(-PI),
        "east" => -FRAC_PI_2,
        "west" => FRAC_PI_2,
        "south" => -PI,
        _ => 0.0,
    };
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-FRAC_PI_2)
}

pub(super) fn piston_parts(
    block: &str,
    facing: &str,
    moving: bool,
    state: Option<&CreatePistonState>,
    alpha: f64,
    camera_from_piston: Vec3,
    render_distance: f32,
) -> Vec<MachinePart> {
    let Some(state) = state.filter(|state| moving && state.running) else {
        return Vec::new();
    };
    let head = match block {
        PISTON => HEAD,
        STICKY_PISTON => STICKY_HEAD,
        _ => return Vec::new(),
    };
    let direction = dir_normal_vec3(facing);
    if direction == Vec3::ZERO {
        return Vec::new();
    }
    let offset =
        state.previous_offset + (state.offset - state.previous_offset) * alpha.clamp(0.0, 1.0);
    let rotation = centred(model_rotation(facing));

    // Project the camera onto the stroke. The server can configure trains longer than the view.
    let camera_offset = f64::from(direction.dot(camera_from_piston - Vec3::splat(0.5)));
    let reach = f64::from(render_distance) + 1.0;
    let first = ((offset - camera_offset - reach).ceil() as i32).max(0);
    let last = ((offset - camera_offset + reach).floor() as i32).min(state.extension_length);
    (first..=last)
        .map(|index| {
            let translation = direction * (offset - f64::from(index)) as f32;
            let model = if index == 0 { head } else { POLE };
            part(model, Mat4::from_translation(translation) * rotation)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piston_tag(length: i32, offset: f32, running: bool) -> Tag {
        Tag::compound([
            ("ExtensionLength", Tag::Int(length)),
            ("Offset", Tag::Float(offset.to_bits())),
            ("Running", Tag::Byte(i8::from(running))),
        ])
    }

    fn moving_parts(
        block: &str,
        facing: &str,
        state: &CreatePistonState,
        alpha: f64,
    ) -> Vec<MachinePart> {
        piston_parts(block, facing, true, Some(state), alpha, Vec3::ZERO, 16.0)
    }

    #[test]
    fn the_piston_controller_tag_applies_to_both_bases() {
        let tag = piston_tag(2, 0.75, true);
        assert!(is_piston_be_tag(PISTON, &tag));
        assert!(!is_piston_be_tag("create:rope_pulley", &tag));
        assert!(is_piston_block(PISTON));
        assert!(is_piston_block(STICKY_PISTON));
        assert!(!is_piston_block("create:mechanical_piston_head"));
        assert!(!is_piston_block("create:piston_extension_pole"));
        let state = piston_state_from_nbt(None, &tag).unwrap();
        assert_eq!(state.render_reach(), 1.25);
        assert!(piston_state_from_nbt(None, &piston_tag(2, f32::NAN, true)).is_none());
    }

    #[test]
    fn moving_train_uses_baked_part_orientation_and_separate_head_models() {
        let state = piston_state_from_nbt(None, &piston_tag(3, 1.25, true)).unwrap();
        let center = Vec3::splat(0.5);
        for facing in ["up", "down", "north", "south", "east", "west"] {
            let direction = dir_normal_vec3(facing);
            let parts = moving_parts(STICKY_PISTON, facing, &state, 1.0);
            assert_eq!(
                parts.iter().map(|part| part.model).collect::<Vec<_>>(),
                [STICKY_HEAD, POLE, POLE, POLE]
            );
            for (index, part) in parts.iter().enumerate() {
                let expected = center + direction * (1.25 - index as f32);
                assert!(
                    part.local
                        .transform_point3(center)
                        .abs_diff_eq(expected, 1.0e-5)
                );
                assert!(
                    part.local
                        .transform_vector3(Vec3::Y)
                        .abs_diff_eq(direction, 1.0e-5)
                );
            }
        }
        assert_eq!(moving_parts(PISTON, "up", &state, 1.0)[0].model, HEAD);
    }

    #[test]
    fn settled_or_missing_state_leaves_placed_parts_to_terrain() {
        let tag = piston_tag(3, 1.0, false);
        let settled = piston_state_from_nbt(None, &tag).unwrap();
        assert!(moving_parts(PISTON, "north", &settled, 0.0).is_empty());
        let active = piston_state_from_nbt(None, &piston_tag(3, 0.0, true)).unwrap();
        assert!(
            piston_parts(PISTON, "north", false, Some(&active), 0.0, Vec3::ZERO, 16.0).is_empty()
        );
        assert!(piston_parts(PISTON, "north", true, None, 0.0, Vec3::ZERO, 16.0).is_empty());
    }

    #[test]
    fn prediction_is_interpolated_and_stops_when_updates_stop_or_repeat() {
        let tag = piston_tag(6, 1.0, true);
        let mut state = piston_state_from_nbt(None, &tag).unwrap();
        tick_piston_state(&mut state, "north", -128.0);
        let head = &moving_parts(PISTON, "north", &state, 0.5)[0];
        let center = head.local.transform_point3(Vec3::splat(0.5));
        assert!(center.abs_diff_eq(Vec3::new(0.5, 0.5, -0.625), 1.0e-5));
        for _ in 0..5 {
            tick_piston_state(&mut state, "north", -128.0);
        }
        assert_eq!(state.offset, 1.75);
        let mut stalled = piston_state_from_nbt(Some(&state), &tag).unwrap();
        tick_piston_state(&mut stalled, "north", -128.0);
        assert_eq!(stalled.offset, 1.0);
    }

    #[test]
    fn waiting_and_sequence_limit_keep_the_train_within_server_travel() {
        let mut state = piston_state_from_nbt(
            None,
            &Tag::compound([
                ("ExtensionLength", Tag::Int(8)),
                ("Offset", Tag::Float(2.0f32.to_bits())),
                ("Running", Tag::Byte(1)),
                ("SequencedOffsetLimit", Tag::Double(0.125f64.to_bits())),
            ]),
        )
        .unwrap();
        for _ in 0..3 {
            tick_piston_state(&mut state, "up", 256.0);
        }
        assert_eq!(state.offset, 1.875);
        let mut waiting = piston_state_from_nbt(
            Some(&state),
            &Tag::compound([
                ("ExtensionLength", Tag::Int(8)),
                ("Offset", Tag::Float(4.0f32.to_bits())),
                ("Running", Tag::Byte(1)),
                ("Waiting", Tag::Byte(1)),
                ("ForceMovement", Tag::Byte(1)),
            ]),
        )
        .unwrap();
        assert_eq!(waiting.previous_offset, 4.0);
        tick_piston_state(&mut waiting, "up", 256.0);
        assert_eq!(waiting.offset, 4.0);
    }

    #[test]
    fn long_trains_only_create_parts_near_the_view() {
        let state = piston_state_from_nbt(None, &piston_tag(i32::MAX, 1000.0, true)).unwrap();
        let parts = piston_parts(PISTON, "up", true, Some(&state), 0.0, Vec3::splat(0.5), 2.0);
        assert_eq!(parts.len(), 7);
        assert!(parts.iter().all(|part| part.model == POLE));
    }
}
