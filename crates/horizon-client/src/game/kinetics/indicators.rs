//! Create 6.0.10 client indicators (`WhistleRenderer`, `NixieTubeRenderer`).
//! Pose and packet data follow the MIT-licensed `Creators-of-Create/Create`.

use super::arm::lerp_f32;
use super::levers::dir_normal_vec3;
use super::machines::{MachinePart, centred, horizontal_angle, part};
use glam::{Mat4, Quat, Vec3};
use horizon_client::block_entity_render::TextCall;
use horizon_client::world::{NixieLamp, NixieState, TrackSignalState, WhistleState};
use horizon_client_core::{
    component,
    nbt_value::{number, text},
    text::Text,
};
use horizon_world::nbt::Tag;
use serde_json::Value;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

const TRACK_PANEL: &str = "create:block/track_signal/panel";
const RED_TUBE: &str = "create:block/track_signal/red_tube";
const YELLOW_TUBE: &str = "create:block/track_signal/yellow_tube";
const WHITE_TUBE: &str = "create:block/track_signal/white_tube";
const COMPUTER_TUBE: &str = "create:block/track_signal/computer_white_tube";
const COMPUTER_BASE: &str = "create:block/track_signal/computer_white_tube_base";

/// `WhistleBlock.getAttachedDirection`: the point of contact with a boiler.
pub(super) fn whistle_attachment(facing: &str, wall: bool) -> [i32; 3] {
    if wall {
        dir_normal_vec3(facing).as_ivec3().to_array()
    } else {
        [0, -1, 0]
    }
}

pub(super) fn tick_whistle_state(state: &mut WhistleState, active: bool) {
    state.active = active;
    state.previous = state.progress;
    state.progress = if active {
        lerp_f32(0.5, state.progress, 1.0)
    } else {
        (state.progress - 0.4).max(0.0)
    };
}

pub(super) fn whistle_part(
    facing: &str,
    size: &str,
    animation: Option<&WhistleState>,
    time: f64,
    alpha: f64,
) -> MachinePart {
    let (model, frequency) = match size {
        "small" => ("create:block/steam_whistle/small_mouth", 4.0),
        "large" => ("create:block/steam_whistle/large_mouth", 2.0),
        _ => ("create:block/steam_whistle/medium_mouth", 3.0),
    };
    let progress = animation.map_or(0.0, |state| {
        let mut progress = lerp_f32(alpha as f32, state.previous, state.progress);
        if state.active && state.progress > 0.5 {
            progress -= ((time.rem_euclid(8.0) as f32 / 8.0) * TAU * frequency).sin() / 16.0;
        }
        progress
    });
    let rotation = Quat::from_rotation_y(horizontal_angle(facing).to_radians());
    part(
        model,
        Mat4::from_translation(Vec3::Y * (progress / 4.0)) * centred(rotation),
    )
}

pub(super) struct NixieBlock<'a> {
    pub block: &'a str,
    pub face: &'a str,
    pub facing: &'a str,
    pub state: Option<&'a NixieState>,
    pub signal: Option<TrackSignalState>,
    pub time: f64,
}

/// Create's `DyeHelper` bright and dark tube colors.
fn nixie_colors(block: &str) -> Option<(u32, u32)> {
    let name = block.strip_prefix("create:")?;
    if name == "nixie_tube" {
        create_dye_colors("orange")
    } else {
        let dye = name.strip_suffix("_nixie_tube")?;
        if dye == "orange" {
            return None;
        }
        create_dye_colors(dye)
    }
}

pub(super) fn create_dye_colors(dye: &str) -> Option<(u32, u32)> {
    Some(match dye {
        "black" => (0x45403B, 0x21201F),
        "red" => (0xB13937, 0x632737),
        "green" => (0x208A46, 0x1D6045),
        "brown" => (0xAC855C, 0x68533E),
        "blue" => (0x5391E1, 0x504B90),
        "gray" => (0x5D666F, 0x313538),
        "light_gray" => (0x95969B, 0x707070),
        "purple" => (0x9F54AE, 0x63366C),
        "cyan" => (0x3EABB4, 0x3C7872),
        "pink" => (0xD5A8CB, 0xB86B95),
        "lime" => (0xA3DF55, 0x4FB16F),
        "yellow" => (0xE6D756, 0xE9AC29),
        "light_blue" => (0x69CED2, 0x508AA5),
        "orange" => (0xEE9246, 0xD94927),
        "magenta" => (0xF062B0, 0xC04488),
        "white" => (0xEDEAE5, 0xBBB6B0),
        _ => return None,
    })
}

pub(super) fn is_nixie_block(block: &str) -> bool {
    nixie_colors(block).is_some()
}

fn custom_nixie_characters(tag: &Tag) -> Option<[char; 2]> {
    let raw = text(tag.get("RawCustomText"))?;
    serde_json::from_str::<Value>(&raw).ok()?;
    let resolved = text(tag.get("CustomText"))?;
    let resolved = serde_json::from_str::<Value>(&resolved).ok()?;
    if resolved.is_null() {
        return None;
    }
    let displayed = component::to_text(&resolved, &mut |_| Text::empty()).plain();
    let index = number(tag.get("CustomTextIndex")).unwrap_or_default();
    let start = usize::try_from(index).unwrap_or_default().saturating_mul(2);
    // Create indexes Java strings in UTF-16 code units, including its empty padding.
    let mut units = displayed.encode_utf16().skip(start);
    Some(std::array::from_fn(|_| {
        units.next().map_or(' ', |unit| {
            char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
    }))
}

fn computer_signal(tag: &Tag) -> Option<[NixieLamp; 2]> {
    let Tag::ByteArray(data) = tag.get("ComputerSignal")? else {
        return None;
    };
    let bytes: &[u8; 14] = data.as_slice().try_into().ok()?;
    Some([&bytes[..7], &bytes[7..]].map(|lamp| NixieLamp {
        rgb: u32::from(lamp[0]) << 16 | u32::from(lamp[1]) << 8 | u32::from(lamp[2]),
        // Create performs the blink comparisons on signed Java bytes.
        blink_period: lamp[3] as i8,
        blink_off_time: lamp[4] as i8,
    }))
}

pub(super) fn apply_nixie_tag(state: &mut NixieState, tag: &Tag) {
    state.characters = custom_nixie_characters(tag).unwrap_or_else(|| {
        let strength = number(tag.get("RedstoneStrength"))
            .unwrap_or_default()
            .clamp(0, 15) as u8;
        [
            char::from(b'0' + strength / 10),
            char::from(b'0' + strength % 10),
        ]
    });
    state.computer_signal = computer_signal(tag);
}

pub(super) fn track_signal_state(tag: &Tag) -> TrackSignalState {
    match text(tag.get("State")).as_deref() {
        Some("RED") => TrackSignalState::Red,
        Some("YELLOW") => TrackSignalState::Yellow,
        Some("GREEN") => TrackSignalState::Green,
        _ => TrackSignalState::Invalid,
    }
}

pub(super) fn nixie_attachment(face: &str, facing: &str) -> [i32; 3] {
    match face {
        "ceiling" => [0, 1, 0],
        "wall" | "wall_reversed" => (-dir_normal_vec3(facing)).as_ivec3().to_array(),
        _ => [0, -1, 0],
    }
}

fn nixie_mount(ctx: &NixieBlock<'_>) -> Quat {
    let yaw = (horizontal_angle(ctx.facing) - 90.0).to_radians();
    let (yaw, roll) = match ctx.face {
        "wall" => (yaw, -FRAC_PI_2),
        "wall_reversed" => (yaw + PI, FRAC_PI_2),
        _ => (yaw, 0.0),
    };
    Quat::from_rotation_y(yaw) * Quat::from_rotation_z(roll)
}

fn mix_rgb(from: u32, to: u32, amount: f32) -> u32 {
    [0, 8, 16]
        .into_iter()
        .map(|shift| {
            let a = ((from >> shift) & 255) as f32;
            let b = ((to >> shift) & 255) as f32;
            (lerp_f32(amount, a, b) as u32) << shift
        })
        .sum()
}

pub(super) fn nixie_text_calls(
    ctx: &NixieBlock<'_>,
    at: Vec3,
    glyph_advance: &dyn Fn(char) -> f32,
) -> Vec<TextCall> {
    if ctx.signal.is_some()
        || ctx
            .state
            .is_some_and(|state| state.computer_signal.is_some())
    {
        return Vec::new();
    }
    let Some((bright, dark)) = nixie_colors(ctx.block) else {
        return Vec::new();
    };
    let characters = ctx.state.map_or(['0'; 2], |state| state.characters);
    let mount = Mat4::from_translation(at + Vec3::splat(0.5)) * Mat4::from_quat(nixie_mount(ctx));
    let height = if ctx.face == "ceiling" { 5.0 } else { 3.0 };
    let mut calls = Vec::with_capacity(8);
    for (index, (ch, tube_x)) in characters.into_iter().zip([-0.25, 0.25]).enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let tube = mount
            * Mat4::from_translation(Vec3::X * tube_x)
            * Mat4::from_scale(Vec3::new(0.05, -0.05, 0.05));
        let x = (0.5 - glyph_advance(ch).ceil()) / 2.0;
        let phase = ((ctx.time * 5.3 + index as f64 * 1.7).sin() as f32 + 1.0) / 8.0;
        let front = mix_rgb(bright, dark, phase);
        let rear = tube * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        for (matrix, x, y, color) in [
            (tube, x, -height, front),
            (
                tube * Mat4::from_translation(-Vec3::Z / 16.0),
                x + 0.5,
                -height + 0.5,
                dark,
            ),
            (rear, x, -height, dark),
            (
                rear * Mat4::from_translation(-Vec3::Z / 16.0),
                x - 0.5,
                -height + 0.5,
                mix_rgb(dark, 0, 0.35),
            ),
        ] {
            calls.push(TextCall {
                matrix,
                x,
                y,
                text: ch.to_string(),
                color: 0xFF00_0000 | color,
                outline: 0,
                light: 0xF0F0,
            });
        }
    }
    calls
}

fn lamp_part(model: &'static str, local: Mat4, scale: f32, tint: Option<u32>) -> MachinePart {
    let mut lamp = part(model, local * Mat4::from_scale(Vec3::splat(scale)));
    lamp.full_bright = true;
    lamp.tint = tint;
    lamp
}

pub(super) fn nixie_signal_parts(ctx: &NixieBlock<'_>) -> Vec<MachinePart> {
    let computer = ctx.state.and_then(|state| state.computer_signal);
    if computer.is_none() && ctx.signal.is_none() {
        return Vec::new();
    }
    let ceiling = ctx.face == "ceiling";
    let inverted = ceiling || ctx.face == "wall_reversed";
    let mut mount = nixie_mount(ctx);
    if ceiling {
        mount *= Quat::from_rotation_z(PI);
    }
    let local = centred(mount);
    let mut parts = vec![part(TRACK_PANEL, local)];
    for (index, left) in [!inverted, inverted].into_iter().enumerate() {
        let place = local
            * Mat4::from_translation(Vec3::new(if left { 0.25 } else { 0.75 }, 7.5 / 16.0, 0.5));
        if let Some(lamps) = computer {
            let lamp = lamps[index];
            if lamp.blink_period == 0
                || lamp.blink_period > 1
                    && ctx.time.rem_euclid(f64::from(lamp.blink_period))
                        < f64::from(lamp.blink_off_time)
            {
                continue;
            }
            parts.push(lamp_part(
                COMPUTER_BASE,
                place,
                1.0 + 1.25 / 16.0,
                Some(0x0C0C0C),
            ));
            parts.push(lamp_part(
                COMPUTER_TUBE,
                place,
                1.0 + 1.0 / 16.0,
                Some(lamp.rgb),
            ));
        } else {
            let tube = match (ctx.signal, index) {
                (Some(TrackSignalState::Red), 0) => RED_TUBE,
                (Some(TrackSignalState::Invalid), 0) if ctx.time.rem_euclid(40.0) < 3.0 => RED_TUBE,
                (Some(TrackSignalState::Yellow), 1) => YELLOW_TUBE,
                (Some(TrackSignalState::Green), 1) => WHITE_TUBE,
                _ => continue,
            };
            parts.push(lamp_part(tube, place, 1.0 + 1.0 / 16.0, None));
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::block_entity_render::draw_text_call;
    use horizon_client::hud::Rect;
    use horizon_client::mesher::CULL_BACK;
    use horizon_client_core::nbt_value::string;

    fn nixie(state: Option<&NixieState>) -> NixieBlock<'_> {
        NixieBlock {
            block: "create:nixie_tube",
            face: "floor",
            facing: "south",
            state,
            signal: None,
            time: 0.0,
        }
    }

    #[test]
    fn whistle_mouth_chases_heat_with_partial_tick_interpolation() {
        let mut animation = WhistleState::default();
        let idle = whistle_part("south", "large", None, 0.0, 0.0);
        assert_eq!(idle.model, "create:block/steam_whistle/large_mouth");
        assert_eq!(idle.local, Mat4::IDENTITY);
        assert_eq!(whistle_attachment("east", true), [1, 0, 0]);
        assert_eq!(whistle_attachment("east", false), [0, -1, 0]);

        tick_whistle_state(&mut animation, true);
        let mid = whistle_part("west", "small", Some(&animation), 0.5, 0.5);
        assert_eq!(mid.model, "create:block/steam_whistle/small_mouth");
        let front = mid.local.transform_point3(Vec3::new(0.5, 0.5, 1.0));
        assert!(front.abs_diff_eq(Vec3::new(0.0, 0.5625, 0.5), 1e-5));

        tick_whistle_state(&mut animation, true);
        let still = whistle_part("south", "medium", Some(&animation), 0.0, 1.0);
        let wiggling = whistle_part("south", "medium", Some(&animation), 2.0 / 3.0, 1.0);
        assert!((still.local.w_axis.y - 0.1875).abs() < 1e-6);
        assert!((still.local.w_axis.y - wiggling.local.w_axis.y - 1.0 / 64.0).abs() < 1e-6);
        tick_whistle_state(&mut animation, false);
        assert!((animation.progress - 0.35).abs() < 1e-6);
        tick_whistle_state(&mut animation, false);
        assert_eq!(animation.progress, 0.0);
    }

    #[test]
    fn nixie_decodes_redstone_and_the_indexed_display_link_text() {
        assert!(is_nixie_block("create:nixie_tube"));
        assert!(is_nixie_block("create:light_blue_nixie_tube"));
        assert!(!is_nixie_block("create:orange_nixie_tube"));
        assert!(!is_nixie_block("other:red_nixie_tube"));
        let mut state = NixieState::default();
        apply_nixie_tag(
            &mut state,
            &Tag::compound([("RedstoneStrength", Tag::Int(9))]),
        );
        assert_eq!(state.characters, ['0', '9']);
        apply_nixie_tag(
            &mut state,
            &Tag::compound([
                ("RawCustomText", string(r#""Switch""#)),
                (
                    "CustomText",
                    string(r#"{"text":"Ab","extra":[{"text":"é"}]}"#),
                ),
                ("CustomTextIndex", Tag::Int(1)),
            ]),
        );
        assert_eq!(state.characters, ['é', ' ']);
        apply_nixie_tag(
            &mut state,
            &Tag::compound([("RedstoneStrength", Tag::Int(15))]),
        );
        assert_eq!(state.characters, ['1', '5']);
    }

    #[test]
    fn nixie_glyphs_occupy_two_tubes_and_follow_the_mount() {
        let floor = nixie(None);
        let calls = nixie_text_calls(&floor, Vec3::ZERO, &|_| 6.0);
        assert_eq!(calls.len(), 8);
        assert!(
            calls
                .iter()
                .all(|call| call.text == "0" && call.light == 0xF0F0)
        );
        let left = calls[0].matrix.transform_point3(Vec3::ZERO);
        let right = calls[4].matrix.transform_point3(Vec3::ZERO);
        assert!(left.abs_diff_eq(Vec3::new(0.5, 0.5, 0.25), 1e-5));
        assert!(right.abs_diff_eq(Vec3::new(0.5, 0.5, 0.75), 1e-5));
        let wall = NixieBlock {
            face: "wall",
            ..floor
        };
        let mounted = nixie_text_calls(&wall, Vec3::ZERO, &|_| 6.0);
        assert!(
            mounted[0]
                .matrix
                .transform_point3(Vec3::ZERO)
                .abs_diff_eq(Vec3::new(0.5, 0.75, 0.5), 1e-5)
        );
        assert_eq!(nixie_attachment("wall_reversed", "east"), [-1, 0, 0]);
        assert_eq!(nixie_attachment("floor", "north"), [0, -1, 0]);
        assert_eq!(nixie_attachment("ceiling", "north"), [0, 1, 0]);
    }

    #[test]
    fn nixie_glyph_faces_are_visible_from_opposite_sides() {
        let calls = nixie_text_calls(&nixie(None), Vec3::ZERO, &|_| 6.0);
        let glyph = |_| {
            (
                Some(Rect {
                    u0: 0.0,
                    v0: 0.0,
                    u1: 1.0,
                    v1: 1.0,
                    w: 8,
                    h: 8,
                }),
                6.0,
            )
        };
        let mut vertices = Vec::new();
        draw_text_call(&glyph, &calls[0], true, &mut vertices);
        draw_text_call(&glyph, &calls[2], true, &mut vertices);
        assert_eq!(vertices.len(), 8);
        assert!(vertices.iter().all(|vertex| vertex.normal == CULL_BACK));
        let normal = |base: usize| {
            let pos = |i: usize| Vec3::from_array(vertices[base + i].pos);
            (pos(1) - pos(0)).cross(pos(2) - pos(0)).normalize()
        };
        assert!((normal(0).dot(normal(4)) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn nixie_switches_to_valid_rail_and_computer_signals() {
        let track = track_signal_state(&Tag::compound([("State", string("YELLOW"))]));
        assert_eq!(track, TrackSignalState::Yellow);
        let rail = NixieBlock {
            signal: Some(track),
            ..nixie(None)
        };
        assert!(nixie_text_calls(&rail, Vec3::ZERO, &|_| 6.0).is_empty());
        let parts = nixie_signal_parts(&rail);
        assert_eq!(parts[0].model, "create:block/track_signal/panel");
        assert_eq!(parts[1].model, "create:block/track_signal/yellow_tube");
        assert!(parts[1].full_bright);

        let mut state = NixieState::default();
        apply_nixie_tag(
            &mut state,
            &Tag::compound([(
                "ComputerSignal",
                Tag::ByteArray(vec![255, 34, 19, 4, 2, 1, 1, 0, 0, 0, 0, 0, 1, 1]),
            )]),
        );
        let off = NixieBlock {
            state: Some(&state),
            ..rail
        };
        assert_eq!(nixie_signal_parts(&off).len(), 1);
        let on = NixieBlock { time: 2.0, ..off };
        let parts = nixie_signal_parts(&on);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].tint, Some(0xFF2213));
        assert!(nixie_text_calls(&on, Vec3::ZERO, &|_| 6.0).is_empty());
        apply_nixie_tag(
            &mut state,
            &Tag::compound([("ComputerSignal", Tag::ByteArray(vec![0; 13]))]),
        );
        assert!(state.computer_signal.is_none());
    }
}
