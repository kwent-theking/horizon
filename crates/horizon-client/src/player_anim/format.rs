//! Animation clips and their two JSON schemas.
//!
//! PlayerAnimator reads an Emotecraft-style file (`emote` with integer-tick
//! `moves`, used by Better Combat and Iron's `waving`) and a Gecko-style file
//! (`animations` with timestamped `bones`, used by Iron's casting clips). Both
//! become the same [`AnimationClip`]: per bone, nine scalar tracks in
//! `Poses` layout (x, y, z, xRot, yRot, zRot, xScale, yScale, zScale), where
//! an empty track is a disabled channel that passes the underlying pose
//! through.

use super::ease::Ease;
use serde_json::{Map, Value};
use std::f32::consts::TAU;

pub const BONE_COUNT: usize = 9;
pub const AXIS_COUNT: usize = 9;

pub const AXIS_X: usize = 0;
pub const AXIS_Y: usize = 1;
pub const AXIS_Z: usize = 2;
pub const AXIS_PITCH: usize = 3;
pub const AXIS_YAW: usize = 4;
pub const AXIS_ROLL: usize = 5;
pub const AXIS_SCALE_X: usize = 6;

/// A channel group of the player. `Torso` is the chest (Horizon's `body`
/// part); `Body` is the whole-player transform, drawn by the renderer's root
/// matrix; the items are item-local transforms at the hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bone {
    Head,
    Torso,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
    Body,
    RightItem,
    LeftItem,
}

impl Bone {
    pub const ALL: [Bone; BONE_COUNT] = [
        Bone::Head,
        Bone::Torso,
        Bone::RightArm,
        Bone::LeftArm,
        Bone::RightLeg,
        Bone::LeftLeg,
        Bone::Body,
        Bone::RightItem,
        Bone::LeftItem,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Accepts `right_arm` and `rightArm` alike (PAL's `snake2Camel`).
    pub fn from_name(name: &str) -> Option<Bone> {
        let key: String = name
            .chars()
            .filter(|c| *c != '_')
            .collect::<String>()
            .to_ascii_lowercase();
        Some(match key.as_str() {
            "head" => Bone::Head,
            "torso" => Bone::Torso,
            "body" => Bone::Body,
            "rightarm" => Bone::RightArm,
            "leftarm" => Bone::LeftArm,
            "rightleg" => Bone::RightLeg,
            "leftleg" => Bone::LeftLeg,
            "rightitem" => Bone::RightItem,
            "leftitem" => Bone::LeftItem,
            _ => return None,
        })
    }

    /// The `Poses` part this bone drives, if it is a model part.
    pub fn part_name(self) -> Option<&'static str> {
        Some(match self {
            Bone::Head => "head",
            Bone::Torso => "body",
            Bone::RightArm => "right_arm",
            Bone::LeftArm => "left_arm",
            Bone::RightLeg => "right_leg",
            Bone::LeftLeg => "left_leg",
            Bone::Body | Bone::RightItem | Bone::LeftItem => return None,
        })
    }

    pub fn mirrored(self) -> Bone {
        match self {
            Bone::RightArm => Bone::LeftArm,
            Bone::LeftArm => Bone::RightArm,
            Bone::RightLeg => Bone::LeftLeg,
            Bone::LeftLeg => Bone::RightLeg,
            Bone::RightItem => Bone::LeftItem,
            Bone::LeftItem => Bone::RightItem,
            other => other,
        }
    }

    /// PAL's default pivot as x, y, z in model pixels.
    fn pivot(self) -> [f32; 3] {
        match self {
            Bone::RightArm => [-5.0, 2.0, 0.0],
            Bone::LeftArm => [5.0, 2.0, 0.0],
            Bone::RightLeg => [-1.9, 12.0, 0.1],
            Bone::LeftLeg => [1.9, 12.0, 0.1],
            _ => [0.0; 3],
        }
    }
}

/// The neutral value of every axis of `bone`: its default pivot, no
/// rotation, unit scale. Boundary keys without a neighbour blend toward it.
pub fn default_pose(bone: Bone) -> [f32; AXIS_COUNT] {
    let [x, y, z] = bone.pivot();
    [x, y, z, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    pub tick: i32,
    pub value: f32,
    pub ease: Ease,
    pub arg: Option<f32>,
}

impl Keyframe {
    pub fn plain(tick: i32, value: f32) -> Self {
        Self {
            tick,
            value,
            ease: Ease::Linear,
            arg: None,
        }
    }
}

/// One scalar channel; empty means disabled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AxisTrack {
    pub keyframes: Vec<Keyframe>,
}

impl AxisTrack {
    pub fn is_enabled(&self) -> bool {
        !self.keyframes.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoneTracks {
    pub axes: [AxisTrack; AXIS_COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipFormat {
    /// `version` of an Emotecraft file (1 when absent).
    Emotecraft(i32),
    Gecko,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub format: ClipFormat,
    pub begin_tick: i32,
    pub end_tick: i32,
    pub stop_tick: i32,
    pub looped: bool,
    pub return_tick: i32,
    pub ease_before: bool,
    pub bones: [BoneTracks; BONE_COUNT],
    /// Channels and fields this runtime does not apply (`bend`, unknown bones).
    pub unsupported: Vec<String>,
}

pub type ParseResult<T> = Result<T, String>;

impl AnimationClip {
    pub(super) fn new(name: String, format: ClipFormat) -> Self {
        Self {
            name,
            format,
            begin_tick: 0,
            end_tick: 1,
            stop_tick: 0,
            looped: false,
            return_tick: 0,
            ease_before: false,
            bones: Default::default(),
            unsupported: Vec::new(),
        }
    }

    pub fn track(&self, bone: Bone, axis: usize) -> &AxisTrack {
        &self.bones[bone.index()].axes[axis]
    }

    fn push(&mut self, bone: Bone, axis: usize, key: Keyframe) {
        self.bones[bone.index()].axes[axis].keyframes.push(key);
    }

    fn note_unsupported(&mut self, what: &str) {
        if !self.unsupported.iter().any(|u| u == what) {
            self.unsupported.push(what.to_owned());
        }
    }

    /// Sorts every track by tick (stable: same-tick keys keep file order) and
    /// normalises an insufficient `stopTick` to `endTick + 3`.
    pub(super) fn finish(&mut self) {
        for bone in &mut self.bones {
            for axis in &mut bone.axes {
                axis.keyframes.sort_by_key(|k| k.tick);
            }
        }
        if self.stop_tick <= self.end_tick {
            self.stop_tick = self.end_tick + 3;
        }
    }
}

fn json_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_f64().map(|n| n != 0.0),
        _ => None,
    }
}

fn json_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|n| n as f32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn json_i32(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(|n| n as i32)
            .or_else(|| n.as_f64().map(|f| f as i32)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn easing_of(map: &Map<String, Value>) -> Ease {
    map.get("easing")
        .and_then(Value::as_str)
        .and_then(Ease::parse)
        .unwrap_or(Ease::Linear)
}

fn emote_axis(name: &str) -> Option<usize> {
    Some(match name {
        "x" => AXIS_X,
        "y" => AXIS_Y,
        "z" => AXIS_Z,
        "pitch" => AXIS_PITCH,
        "yaw" => AXIS_YAW,
        "roll" => AXIS_ROLL,
        "scaleX" => AXIS_SCALE_X,
        "scaleY" => AXIS_SCALE_X + 1,
        "scaleZ" => AXIS_SCALE_X + 2,
        _ => return None,
    })
}

/// Parses an Emotecraft-style file (`emote` with `moves`).
pub fn parse_emotecraft(text: &str) -> ParseResult<AnimationClip> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let root = root.as_object().ok_or("not a JSON object")?;
    let emote = root
        .get("emote")
        .and_then(Value::as_object)
        .ok_or("no `emote` object")?;
    let version = root.get("version").and_then(json_i32).unwrap_or(1);
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut clip = AnimationClip::new(name, ClipFormat::Emotecraft(version));
    clip.begin_tick = emote.get("beginTick").and_then(json_i32).unwrap_or(0);
    clip.end_tick = emote
        .get("endTick")
        .and_then(json_i32)
        .ok_or("no endTick")?;
    if clip.end_tick <= 0 {
        return Err("endTick must be bigger than 0".into());
    }
    // PAL reads the loop only when both keys are present.
    if let (Some(looped), Some(return_tick)) = (emote.get("isLoop"), emote.get("returnTick")) {
        clip.looped = json_bool(looped).unwrap_or(false);
        clip.return_tick = json_i32(return_tick).unwrap_or(0);
        if clip.looped && (clip.return_tick > clip.end_tick || clip.return_tick < 0) {
            return Err("return tick has to be within 0..=endTick".into());
        }
    }
    clip.stop_tick = emote
        .get("stopTick")
        .and_then(json_i32)
        .unwrap_or(clip.end_tick);
    let degrees = emote.get("degrees").and_then(json_bool).unwrap_or(true);
    clip.ease_before = emote
        .get("easeBeforeKeyframe")
        .and_then(json_bool)
        .unwrap_or(false);
    let moves = emote
        .get("moves")
        .and_then(Value::as_array)
        .ok_or("no `moves` array")?;
    for entry in moves {
        if let Some(entry) = entry.as_object() {
            read_move(&mut clip, entry, version, degrees)?;
        }
    }
    clip.finish();
    Ok(clip)
}

fn read_move(
    clip: &mut AnimationClip,
    entry: &Map<String, Value>,
    version: i32,
    degrees: bool,
) -> ParseResult<()> {
    let tick = entry
        .get("tick")
        .and_then(json_i32)
        .ok_or("move without a tick")?;
    let ease = easing_of(entry);
    let arg = entry.get("easingArg").and_then(json_f32);
    let turn = entry.get("turn").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    for (part_name, part) in entry {
        let Some(part) = part.as_object() else {
            continue;
        };
        let Some(mut bone) = Bone::from_name(part_name) else {
            clip.note_unsupported(part_name);
            continue;
        };
        // Emotecraft's old `torso` is the whole player (PAL's `body`).
        if version < 3 && bone == Bone::Torso {
            bone = Bone::Body;
        }
        for (axis_name, value) in part {
            if matches!(axis_name.as_str(), "bend" | "axis") {
                clip.note_unsupported("bend");
                continue;
            }
            let (Some(axis), Some(mut value)) = (emote_axis(axis_name), json_f32(value)) else {
                continue;
            };
            if (AXIS_PITCH..=AXIS_ROLL).contains(&axis) {
                if degrees {
                    value = value.to_radians();
                }
                value += turn * TAU;
            }
            clip.push(
                bone,
                axis,
                Keyframe {
                    tick,
                    value,
                    ease,
                    arg,
                },
            );
        }
    }
    Ok(())
}

/// Parses a Gecko-style file: every entry of `animations` becomes a clip.
pub fn parse_gecko(text: &str) -> ParseResult<Vec<AnimationClip>> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let animations = root
        .get("animations")
        .and_then(Value::as_object)
        .ok_or("no `animations` object")?;
    let mut clips = Vec::with_capacity(animations.len());
    for (name, animation) in animations {
        let Some(animation) = animation.as_object() else {
            continue;
        };
        clips.push(read_gecko_clip(name, animation));
    }
    Ok(clips)
}

fn read_gecko_clip(name: &str, animation: &Map<String, Value>) -> AnimationClip {
    let mut clip = AnimationClip::new(name.to_owned(), ClipFormat::Gecko);
    let length = animation
        .get("animation_length")
        .and_then(json_f32)
        .unwrap_or(0.0);
    clip.end_tick = ((length * 20.0).ceil() as i32).max(1);
    match animation.get("loop") {
        Some(Value::String(s)) if s == "hold_on_last_frame" => {
            clip.looped = true;
            clip.return_tick = clip.end_tick;
        }
        // An explicit `loop`, true or false, takes the loader's end-tick
        // decrement; an absent one keeps the computed end.
        Some(other) => {
            clip.looped = json_bool(other).unwrap_or(false);
            clip.end_tick = (clip.end_tick - 1).max(1);
        }
        None => {}
    }
    if let Some(bones) = animation.get("bones").and_then(Value::as_object) {
        for (bone_name, channels) in bones {
            let Some(bone) = Bone::from_name(bone_name) else {
                clip.note_unsupported(bone_name);
                continue;
            };
            let Some(channels) = channels.as_object() else {
                continue;
            };
            for (channel, value) in channels {
                read_gecko_channel(&mut clip, bone, channel, value);
            }
        }
    }
    clip.finish();
    clip
}

struct GeckoKey {
    tick: i32,
    vector: [f32; 3],
    ease: Ease,
    arg: Option<f32>,
}

fn vec3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() < 3 {
        return None;
    }
    let component = |i: usize| json_f32(&a[i]).unwrap_or(0.0);
    Some([component(0), component(1), component(2)])
}

fn read_gecko_channel(clip: &mut AnimationClip, bone: Bone, channel: &str, value: &Value) {
    if !matches!(channel, "rotation" | "position" | "scale") {
        clip.note_unsupported(channel);
        return;
    }
    let mut keys = Vec::new();
    match value {
        Value::Array(_) => {
            if let Some(vector) = vec3(value) {
                keys.push(GeckoKey {
                    tick: 0,
                    vector,
                    ease: Ease::Linear,
                    arg: None,
                });
            }
        }
        Value::Object(frames) => {
            for (time, frame) in frames {
                let Ok(seconds) = time.parse::<f32>() else {
                    continue;
                };
                // Java float arithmetic: truncate seconds * 20.
                let tick = (seconds * 20.0) as i32;
                match frame {
                    Value::Array(_) => {
                        if let Some(vector) = vec3(frame) {
                            keys.push(GeckoKey {
                                tick,
                                vector,
                                ease: Ease::Linear,
                                arg: None,
                            });
                        }
                    }
                    Value::Object(frame) => {
                        let vector = ["vector", "post", "pre"]
                            .iter()
                            .find_map(|k| frame.get(*k))
                            .and_then(vec3);
                        let Some(vector) = vector else {
                            continue;
                        };
                        keys.push(GeckoKey {
                            tick,
                            vector,
                            ease: easing_of(frame),
                            arg: frame
                                .get("easingArgs")
                                .and_then(Value::as_array)
                                .and_then(|a| a.first())
                                .and_then(json_f32),
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    for key in keys {
        for (axis, value) in gecko_values(bone, channel, key.vector) {
            clip.push(
                bone,
                axis,
                Keyframe {
                    tick: key.tick,
                    value,
                    ease: key.ease,
                    arg: key.arg,
                },
            );
        }
    }
}

/// Gecko vector to `(axis, value)` pairs in `Poses` units: radians, model
/// pixels from the default pivot (y negated), and blocks for the whole player.
fn gecko_values(bone: Bone, channel: &str, [x, y, z]: [f32; 3]) -> [(usize, f32); 3] {
    match channel {
        "rotation" if bone == Bone::Body => [
            (AXIS_PITCH, -x.to_radians()),
            (AXIS_YAW, -y.to_radians()),
            (AXIS_ROLL, z.to_radians()),
        ],
        "rotation" => [
            (AXIS_PITCH, x.to_radians()),
            (AXIS_YAW, y.to_radians()),
            (AXIS_ROLL, z.to_radians()),
        ],
        "position" if bone == Bone::Body => {
            [(AXIS_X, -x / 16.0), (AXIS_Y, y / 16.0), (AXIS_Z, z / 16.0)]
        }
        "position" => {
            let [px, py, pz] = bone.pivot();
            [(AXIS_X, px + x), (AXIS_Y, py - y), (AXIS_Z, pz + z)]
        }
        _ => [
            (AXIS_SCALE_X, x),
            (AXIS_SCALE_X + 1, y),
            (AXIS_SCALE_X + 2, z),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BC_SLASH: &str = r#"{
      "name": "one_handed_slash_horizontal_right",
      "author": "Daedelus",
      "emote": {
        "isLoop": "false", "returnTick": 2, "beginTick": 8, "endTick": 20,
        "stopTick": 30, "degrees": false,
        "moves": [
          {"tick": 8, "easing": "EASEINOUTQUAD", "turn": 0, "head": {"yaw": -0.4427400529384613}},
          {"tick": 9, "easing": "EASEINOUTQUAD", "turn": 0, "head": {"yaw": 0.2642005681991577}},
          {"tick": 9, "easing": "EASEINQUAD", "turn": 0, "head": {"yaw": 0.5}},
          {"tick": 8, "easing": "EASEINOUTQUAD", "turn": 0, "torso": {"pitch": 1.0}},
          {"tick": 8, "easing": "EASEINOUTQUAD", "turn": 0, "rightItem": {"x": 3.0}}
        ]
      }
    }"#;

    const WAVING: &str = r#"{
      "version": 1, "uuid": "33b912f8-0aa0-45e6-a2d4-9b5677e6f35c", "name": "waving",
      "emote": {"beginTick": 12, "endTick": 45, "stopTick": 50, "isLoop": false,
        "returnTick": 0, "nsfw": false, "degrees": false,
        "moves": [{"tick": 0, "easing": "INQUAD", "rightLeg": {"x": -1.9}}]}
    }"#;

    const GECKO: &str = r#"{
      "format_version": "1.8.0",
      "animations": {
        "charged_throw": {
          "loop": "hold_on_last_frame", "animation_length": 25,
          "bones": {"right_arm": {"rotation": {
            "0.0": {"vector": [0, 0, 0]},
            "0.5": {"vector": [-210.90686, -10.58927, -17.06488], "easing": "easeInOutSine"},
            "0.875": {"vector": [-163.41, -10.59, -17.06], "easing": "easeInOutSine"}}}}
        },
        "sway": {
          "loop": true, "animation_length": 1,
          "bones": {
            "body": {"rotation": [10, 20, 30], "position": [16, 16, 16]},
            "left_arm": {"position": {
              "0.1875": [1, 2, 3],
              "0.0833": {"vector": [0, 0, 0], "easing": "easeInElastic", "easingArgs": [2]}}},
            "bipedHandLeft": {"rotation": [0, 0, 0]}
          }
        },
        "plain": {"animation_length": 0.5, "bones": {"torso": {"rotation": [90, 0, 0]}}}
      }
    }"#;

    #[test]
    fn emotecraft_better_combat_clip() {
        let clip = parse_emotecraft(BC_SLASH).unwrap();
        assert_eq!(clip.name, "one_handed_slash_horizontal_right");
        assert_eq!(clip.format, ClipFormat::Emotecraft(1));
        assert_eq!((clip.begin_tick, clip.end_tick, clip.stop_tick), (8, 20, 30));
        // "isLoop": "false" is a string Boolean.
        assert!(!clip.looped);
        // Radians were given: no conversion.
        let yaw = clip.track(Bone::Head, AXIS_YAW);
        assert_eq!(yaw.keyframes.len(), 3);
        assert!((yaw.keyframes[0].value + 0.442_740_05).abs() < 1e-6);
        // Same-tick keys keep their file order.
        assert_eq!(yaw.keyframes[1].tick, 9);
        assert!((yaw.keyframes[1].value - 0.264_200_57).abs() < 1e-6);
        assert_eq!(yaw.keyframes[2].value, 0.5);
        // Sparse axes: only the written ones are enabled.
        assert!(!clip.track(Bone::Head, AXIS_PITCH).is_enabled());
        assert!(clip.track(Bone::RightItem, AXIS_X).is_enabled());
    }

    #[test]
    fn emotecraft_old_torso_is_the_whole_player() {
        let old = parse_emotecraft(BC_SLASH).unwrap();
        assert!(old.track(Bone::Body, AXIS_PITCH).is_enabled());
        assert!(!old.track(Bone::Torso, AXIS_PITCH).is_enabled());
        let versioned = BC_SLASH.replace("\"name\"", "\"version\": 3, \"name\"");
        let new = parse_emotecraft(&versioned).unwrap();
        assert!(new.track(Bone::Torso, AXIS_PITCH).is_enabled());
        assert!(!new.track(Bone::Body, AXIS_PITCH).is_enabled());
    }

    #[test]
    fn emotecraft_defaults_and_degrees() {
        let clip = parse_emotecraft(WAVING).unwrap();
        assert_eq!(clip.name, "waving");
        let key = &clip.track(Bone::RightLeg, AXIS_X).keyframes[0];
        assert_eq!(key.value, -1.9);
        assert_eq!(key.ease, Ease::parse("inquad").unwrap());
        // degrees defaults to true.
        let text = r#"{"emote":{"endTick":10,"moves":[{"tick":0,"head":{"pitch":90}}]}}"#;
        let clip = parse_emotecraft(text).unwrap();
        let value = clip.track(Bone::Head, AXIS_PITCH).keyframes[0].value;
        assert!((value - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        // stopTick falls back to endTick, then to endTick + 3.
        assert_eq!(clip.stop_tick, 13);
    }

    #[test]
    fn emotecraft_turn_and_loop_validation() {
        let text = r#"{"emote":{"endTick":10,"degrees":false,"moves":[{"tick":0,"turn":2,"head":{"yaw":0.5}}]}}"#;
        let clip = parse_emotecraft(text).unwrap();
        let value = clip.track(Bone::Head, AXIS_YAW).keyframes[0].value;
        assert!((value - (0.5 + 2.0 * TAU)).abs() < 1e-4);
        let bad = r#"{"emote":{"endTick":10,"isLoop":true,"returnTick":11,"moves":[]}}"#;
        assert!(parse_emotecraft(bad).is_err());
        assert!(parse_emotecraft(r#"{"emote":{"endTick":0,"moves":[]}}"#).is_err());
    }

    #[test]
    fn gecko_hold_on_last_frame() {
        let clips = parse_gecko(GECKO).unwrap();
        let clip = clips
            .iter()
            .find(|clip| clip.name == "charged_throw")
            .unwrap();
        assert_eq!(clip.end_tick, 500);
        assert!(clip.looped);
        assert_eq!(clip.return_tick, clip.end_tick);
        assert_eq!(clip.stop_tick, 503);
        let pitch = clip.track(Bone::RightArm, AXIS_PITCH);
        let ticks: Vec<i32> = pitch.keyframes.iter().map(|k| k.tick).collect();
        assert_eq!(ticks, vec![0, 10, 17]);
        assert!((pitch.keyframes[1].value - (-210.90686_f32).to_radians()).abs() < 1e-6);
        assert_eq!(pitch.keyframes[1].ease, Ease::IN_OUT_SINE);
    }

    #[test]
    fn gecko_units_and_timestamps() {
        let clips = parse_gecko(GECKO).unwrap();
        let sway = clips.iter().find(|clip| clip.name == "sway").unwrap();
        // loop: true decrements the computed end tick.
        assert_eq!(sway.end_tick, 19);
        assert!(sway.looped);
        assert_eq!(sway.return_tick, 0);
        // Whole-player position is (-x, y, z) / 16 blocks.
        assert_eq!(sway.track(Bone::Body, AXIS_X).keyframes[0].value, -1.0);
        assert_eq!(sway.track(Bone::Body, AXIS_Y).keyframes[0].value, 1.0);
        // Whole-player rotation is (-x, -y, z) in radians.
        let pitch = sway.track(Bone::Body, AXIS_PITCH).keyframes[0].value;
        assert!((pitch + 10.0_f32.to_radians()).abs() < 1e-6);
        let roll = sway.track(Bone::Body, AXIS_ROLL).keyframes[0].value;
        assert!((roll - 30.0_f32.to_radians()).abs() < 1e-6);
        // Ordinary position: default pivot + (x, -y, z); timestamps truncate
        // (0.0833 s is tick 1, 0.1875 s is tick 3).
        let y = sway.track(Bone::LeftArm, AXIS_Y);
        let ticks: Vec<i32> = y.keyframes.iter().map(|k| k.tick).collect();
        assert_eq!(ticks, vec![1, 3]);
        assert_eq!(y.keyframes[0].value, 2.0);
        assert_eq!(y.keyframes[1].value, 0.0);
        assert_eq!(sway.track(Bone::LeftArm, AXIS_X).keyframes[1].value, 6.0);
        // easingArgs ride on the key.
        assert_eq!(y.keyframes[0].arg, Some(2.0));
        // Unknown bones are kept as a note, not applied.
        assert!(sway.unsupported.iter().any(|u| u == "bipedHandLeft"));
    }

    #[test]
    fn gecko_torso_is_chest_and_absent_loop_keeps_length() {
        let clips = parse_gecko(GECKO).unwrap();
        let plain = clips.iter().find(|clip| clip.name == "plain").unwrap();
        assert_eq!(plain.end_tick, 10);
        assert!(!plain.looped);
        assert!(plain.track(Bone::Torso, AXIS_PITCH).is_enabled());
        assert!(!plain.track(Bone::Body, AXIS_PITCH).is_enabled());
    }
}
