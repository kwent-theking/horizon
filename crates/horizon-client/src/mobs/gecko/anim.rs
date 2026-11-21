//! GeckoLib 4 `.animation.json` parser and sampler.
//!
//! Sampled values are raw Bedrock values: rotation in degrees, position in
//! 1/16 block units, scale as a multiplier. Bedrock axis and sign conventions
//! are not converted here; the renderer applies its own convention.
//!
//! Keyframe easing and `lerp_mode` on keyframe K govern the segment from K to
//! the next keyframe. Molang values are evaluated at the clip-local time.

use std::collections::BTreeMap;

use serde_json::Value;

use super::molang::{Expr, MolangContext, compile};

const ANIM_TIME_QUERY: &str = "query.anim_time";
const BACK_OVERSHOOT: f32 = 1.70158;
const ELASTIC_AMPLITUDE_PERIOD: f32 = 2.0 * std::f32::consts::PI / 3.0;
const ELASTIC_INOUT_PERIOD: f32 = 2.0 * std::f32::consts::PI / 4.5;

pub struct AnimationFile {
    pub clips: BTreeMap<String, Clip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    Once,
    Loop,
    HoldOnLastFrame,
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub loop_mode: LoopMode,
    length: f32,
    bones: BTreeMap<String, BoneChannels>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneSample {
    pub rotation_deg: [f32; 3],
    pub position: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for BoneSample {
    fn default() -> Self {
        Self {
            rotation_deg: [0.0; 3],
            position: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BoneChannels {
    rotation: Vec<Keyframe>,
    position: Vec<Keyframe>,
    scale: Vec<Keyframe>,
}

#[derive(Debug, Clone)]
enum Component {
    Constant(f32),
    Molang(Expr),
}

type Vector = [Component; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Curve {
    Sine,
    Quad,
    Cubic,
    Quart,
    Quint,
    Expo,
    Circ,
    Back,
    Elastic,
    Bounce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    In,
    Out,
    InOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Easing {
    Linear,
    Step,
    Curve(Curve, Phase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LerpMode {
    Linear,
    CatmullRom,
    Step,
}

#[derive(Debug, Clone)]
struct Keyframe {
    time: f32,
    value: Vector,
    pre: Option<Vector>,
    post: Option<Vector>,
    easing: Easing,
    lerp: LerpMode,
}

impl Keyframe {
    fn incoming(&self) -> &Vector {
        self.pre.as_ref().unwrap_or(&self.value)
    }

    fn outgoing(&self) -> &Vector {
        self.post.as_ref().unwrap_or(&self.value)
    }

    fn holds_outgoing(&self) -> bool {
        self.lerp == LerpMode::Step || self.easing == Easing::Step
    }
}

impl Clip {
    pub fn length(&self) -> f32 {
        self.length
    }

    pub fn is_finished(&self, time_s: f32) -> bool {
        self.loop_mode == LoopMode::Once && time_s >= self.length
    }

    pub fn sample(&self, time_s: f32, ctx: &dyn MolangContext) -> Vec<(String, BoneSample)> {
        let local = self.local_time(time_s);
        let timed = AnimTimeContext {
            inner: ctx,
            anim_time: local,
        };
        self.bones
            .iter()
            .map(|(name, channels)| (name.clone(), channels.sample(local, &timed)))
            .collect()
    }

    fn local_time(&self, time_s: f32) -> f32 {
        if self.loop_mode == LoopMode::Loop {
            if self.length > 0.0 {
                time_s.rem_euclid(self.length)
            } else {
                0.0
            }
        } else {
            time_s.max(0.0).min(self.length)
        }
    }
}

struct AnimTimeContext<'a> {
    inner: &'a dyn MolangContext,
    anim_time: f32,
}

impl MolangContext for AnimTimeContext<'_> {
    fn query(&self, name: &str) -> f32 {
        if name == ANIM_TIME_QUERY {
            self.anim_time
        } else {
            self.inner.query(name)
        }
    }
}

impl BoneChannels {
    fn sample(&self, time: f32, ctx: &dyn MolangContext) -> BoneSample {
        BoneSample {
            rotation_deg: sample_track(&self.rotation, time, ctx, [0.0; 3]),
            position: sample_track(&self.position, time, ctx, [0.0; 3]),
            scale: sample_track(&self.scale, time, ctx, [1.0; 3]),
        }
    }

    fn last_time(&self) -> f32 {
        [&self.rotation, &self.position, &self.scale]
            .iter()
            .filter_map(|track| track.last().map(|key| key.time))
            .fold(0.0, f32::max)
    }
}

fn sample_track(
    track: &[Keyframe],
    time: f32,
    ctx: &dyn MolangContext,
    default: [f32; 3],
) -> [f32; 3] {
    let (Some(first), Some(last)) = (track.first(), track.last()) else {
        return default;
    };
    if time <= first.time {
        return eval_vector(first.incoming(), ctx);
    }
    if time >= last.time {
        return eval_vector(last.outgoing(), ctx);
    }
    let next = track.partition_point(|key| key.time <= time);
    let from_index = next - 1;
    let from = &track[from_index];
    let to = &track[next];
    if from.holds_outgoing() {
        return eval_vector(from.outgoing(), ctx);
    }
    let progress = (time - from.time) / (to.time - from.time);
    match from.lerp {
        LerpMode::CatmullRom => {
            let p1 = eval_vector(from.outgoing(), ctx);
            let p2 = eval_vector(to.incoming(), ctx);
            let p0 = match from_index.checked_sub(1) {
                Some(prev) => eval_vector(track[prev].outgoing(), ctx),
                None => p1,
            };
            let p3 = match track.get(next + 1) {
                Some(after) => eval_vector(after.incoming(), ctx),
                None => p2,
            };
            catmull_rom(p0, p1, p2, p3, progress)
        }
        LerpMode::Linear => {
            let start = eval_vector(from.outgoing(), ctx);
            let end = eval_vector(to.incoming(), ctx);
            lerp_vector(start, end, apply_easing(from.easing, progress))
        }
        LerpMode::Step => eval_vector(from.outgoing(), ctx),
    }
}

fn eval_vector(vector: &Vector, ctx: &dyn MolangContext) -> [f32; 3] {
    [
        eval_component(&vector[0], ctx),
        eval_component(&vector[1], ctx),
        eval_component(&vector[2], ctx),
    ]
}

fn eval_component(component: &Component, ctx: &dyn MolangContext) -> f32 {
    let value = match component {
        Component::Constant(value) => *value,
        Component::Molang(expr) => expr.eval(ctx),
    };
    if value.is_finite() { value } else { 0.0 }
}

fn lerp_vector(start: [f32; 3], end: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| start[i] + (end[i] - start[i]) * t)
}

fn catmull_rom(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], u: f32) -> [f32; 3] {
    let u2 = u * u;
    let u3 = u2 * u;
    std::array::from_fn(|i| {
        0.5 * (2.0 * p1[i]
            + (p2[i] - p0[i]) * u
            + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * u2
            + (3.0 * p1[i] - p0[i] - 3.0 * p2[i] + p3[i]) * u3)
    })
}

fn apply_easing(easing: Easing, t: f32) -> f32 {
    match easing {
        Easing::Linear | Easing::Step => t,
        Easing::Curve(curve, phase) => ease(curve, phase, t),
    }
}

fn ease(curve: Curve, phase: Phase, t: f32) -> f32 {
    match phase {
        Phase::In => ease_in(curve, t),
        Phase::Out => ease_out(curve, t),
        Phase::InOut => match curve {
            Curve::Back => back_in_out(t),
            Curve::Elastic => elastic_in_out(t),
            _ if t < 0.5 => ease_in(curve, 2.0 * t) / 2.0,
            _ => 1.0 - ease_in(curve, 2.0 - 2.0 * t) / 2.0,
        },
    }
}

fn ease_in(curve: Curve, t: f32) -> f32 {
    match curve {
        Curve::Sine => 1.0 - (t * std::f32::consts::FRAC_PI_2).cos(),
        Curve::Quad => t * t,
        Curve::Cubic => t * t * t,
        Curve::Quart => t.powi(4),
        Curve::Quint => t.powi(5),
        Curve::Expo => {
            if t == 0.0 {
                0.0
            } else {
                2f32.powf(10.0 * t - 10.0)
            }
        }
        Curve::Circ => 1.0 - (1.0 - t * t).sqrt(),
        Curve::Back => back_in(t),
        Curve::Elastic => elastic_in(t),
        Curve::Bounce => 1.0 - bounce_out(1.0 - t),
    }
}

fn ease_out(curve: Curve, t: f32) -> f32 {
    match curve {
        Curve::Elastic => elastic_out(t),
        Curve::Bounce => bounce_out(t),
        _ => 1.0 - ease_in(curve, 1.0 - t),
    }
}

fn back_in(t: f32) -> f32 {
    let c3 = BACK_OVERSHOOT + 1.0;
    c3 * t * t * t - BACK_OVERSHOOT * t * t
}

fn back_in_out(t: f32) -> f32 {
    let c2 = BACK_OVERSHOOT * 1.525;
    if t < 0.5 {
        ((2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2)) / 2.0
    } else {
        ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (2.0 * t - 2.0) + c2) + 2.0) / 2.0
    }
}

fn elastic_in(t: f32) -> f32 {
    if t == 0.0 || t == 1.0 {
        return t;
    }
    -2f32.powf(10.0 * t - 10.0) * ((10.0 * t - 10.75) * ELASTIC_AMPLITUDE_PERIOD).sin()
}

fn elastic_out(t: f32) -> f32 {
    if t == 0.0 || t == 1.0 {
        return t;
    }
    2f32.powf(-10.0 * t) * ((10.0 * t - 0.75) * ELASTIC_AMPLITUDE_PERIOD).sin() + 1.0
}

fn elastic_in_out(t: f32) -> f32 {
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let wave = ((20.0 * t - 11.125) * ELASTIC_INOUT_PERIOD).sin();
    if t < 0.5 {
        -(2f32.powf(20.0 * t - 10.0) * wave) / 2.0
    } else {
        (2f32.powf(-20.0 * t + 10.0) * wave) / 2.0 + 1.0
    }
}

fn bounce_out(t: f32) -> f32 {
    const N1: f32 = 7.5625;
    const D1: f32 = 2.75;
    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let x = t - 1.5 / D1;
        N1 * x * x + 0.75
    } else if t < 2.5 / D1 {
        let x = t - 2.25 / D1;
        N1 * x * x + 0.9375
    } else {
        let x = t - 2.625 / D1;
        N1 * x * x + 0.984375
    }
}

pub fn parse_animations(json: &str) -> Result<AnimationFile, String> {
    let root: Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|err| format!("invalid JSON: {err}"))?;
    let animations = root
        .get("animations")
        .and_then(Value::as_object)
        .ok_or("missing \"animations\" object")?;
    let mut clips = BTreeMap::new();
    for (name, clip) in animations {
        let parsed = parse_clip(clip).map_err(|err| format!("animation '{name}': {err}"))?;
        clips.insert(name.clone(), parsed);
    }
    Ok(AnimationFile { clips })
}

fn parse_clip(value: &Value) -> Result<Clip, String> {
    let object = value.as_object().ok_or("animation must be an object")?;
    let loop_mode = parse_loop_mode(object.get("loop"))?;
    let declared_length = match object.get("animation_length") {
        None | Some(Value::Null) => None,
        Some(length) => Some(length.as_f64().ok_or("animation_length must be a number")? as f32),
    };
    let mut bones = BTreeMap::new();
    if let Some(bone_map) = object.get("bones").filter(|value| !value.is_null()) {
        let bone_map = bone_map.as_object().ok_or("bones must be an object")?;
        for (bone, channels) in bone_map {
            bones.insert(
                bone.clone(),
                parse_bone(channels).map_err(|err| format!("bone '{bone}': {err}"))?,
            );
        }
    }
    let last_key_time = bones
        .values()
        .map(BoneChannels::last_time)
        .fold(0.0, f32::max);
    Ok(Clip {
        loop_mode,
        length: declared_length.unwrap_or(last_key_time),
        bones,
    })
}

fn parse_loop_mode(value: Option<&Value>) -> Result<LoopMode, String> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(LoopMode::Once),
        Some(Value::Bool(true)) => Ok(LoopMode::Loop),
        Some(Value::String(mode)) => match mode.as_str() {
            "true" => Ok(LoopMode::Loop),
            "false" => Ok(LoopMode::Once),
            "hold_on_last_frame" => Ok(LoopMode::HoldOnLastFrame),
            other => Err(format!("unknown loop mode '{other}'")),
        },
        Some(other) => Err(format!("unsupported loop value {other}")),
    }
}

fn parse_bone(value: &Value) -> Result<BoneChannels, String> {
    let object = value.as_object().ok_or("bone must be an object")?;
    Ok(BoneChannels {
        rotation: parse_track(object.get("rotation"))?,
        position: parse_track(object.get("position"))?,
        scale: parse_track(object.get("scale"))?,
    })
}

fn parse_track(value: Option<&Value>) -> Result<Vec<Keyframe>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(timeline)) if !timeline.keys().any(|key| is_keyframe_field(key)) => {
            let mut keys = timeline
                .iter()
                .map(|(time, key)| parse_keyframe(time, key))
                .collect::<Result<Vec<_>, _>>()?;
            keys.sort_by(|a, b| a.time.total_cmp(&b.time));
            Ok(keys)
        }
        Some(single @ Value::Object(_)) => Ok(vec![parse_keyframe("0.0", single)?]),
        Some(constant) => Ok(vec![Keyframe {
            time: 0.0,
            value: parse_vector(constant)?,
            pre: None,
            post: None,
            easing: Easing::Linear,
            lerp: LerpMode::Linear,
        }]),
    }
}

fn is_keyframe_field(key: &str) -> bool {
    matches!(key, "vector" | "pre" | "post")
}

fn parse_keyframe(time_key: &str, value: &Value) -> Result<Keyframe, String> {
    let time: f32 = time_key
        .trim()
        .parse()
        .map_err(|_| format!("bad keyframe time '{time_key}'"))?;
    let Value::Object(fields) = value else {
        return Ok(Keyframe {
            time,
            value: parse_vector(value)?,
            pre: None,
            post: None,
            easing: Easing::Linear,
            lerp: LerpMode::Linear,
        });
    };
    let vector = fields.get("vector").map(parse_vector).transpose()?;
    let pre = fields.get("pre").map(parse_vector).transpose()?;
    let post = fields.get("post").map(parse_vector).transpose()?;
    let value = vector
        .or_else(|| post.clone())
        .or_else(|| pre.clone())
        .ok_or_else(|| format!("keyframe '{time_key}' has no vector, pre or post"))?;
    Ok(Keyframe {
        time,
        value,
        pre,
        post,
        easing: parse_easing(fields.get("easing")),
        lerp: parse_lerp_mode(fields.get("lerp_mode")),
    })
}

fn parse_easing(value: Option<&Value>) -> Easing {
    let name = value.and_then(Value::as_str).unwrap_or("linear");
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "step" => return Easing::Step,
        "linear" => return Easing::Linear,
        _ => {}
    }
    let Some(rest) = lower.strip_prefix("ease") else {
        return Easing::Linear;
    };
    let (phase, curve_name) = if let Some(curve) = rest.strip_prefix("inout") {
        (Phase::InOut, curve)
    } else if let Some(curve) = rest.strip_prefix("in") {
        (Phase::In, curve)
    } else if let Some(curve) = rest.strip_prefix("out") {
        (Phase::Out, curve)
    } else {
        return Easing::Linear;
    };
    let curve = match curve_name {
        "sine" => Curve::Sine,
        "quad" => Curve::Quad,
        "cubic" => Curve::Cubic,
        "quart" => Curve::Quart,
        "quint" => Curve::Quint,
        "expo" => Curve::Expo,
        "circ" => Curve::Circ,
        "back" => Curve::Back,
        "elastic" => Curve::Elastic,
        "bounce" => Curve::Bounce,
        _ => return Easing::Linear,
    };
    Easing::Curve(curve, phase)
}

fn parse_lerp_mode(value: Option<&Value>) -> LerpMode {
    match value.and_then(Value::as_str) {
        Some("catmullrom") => LerpMode::CatmullRom,
        Some("step") => LerpMode::Step,
        _ => LerpMode::Linear,
    }
}

fn parse_vector(value: &Value) -> Result<Vector, String> {
    match value {
        Value::Array(items) if items.len() == 3 => Ok([
            parse_component(&items[0])?,
            parse_component(&items[1])?,
            parse_component(&items[2])?,
        ]),
        Value::Array(items) => Err(format!("vector needs 3 components, got {}", items.len())),
        Value::Object(fields) => match fields.get("vector") {
            Some(vector) => parse_vector(vector),
            None => Err("keyframe value object has no \"vector\" field".to_string()),
        },
        scalar => {
            let component = parse_component(scalar)?;
            Ok([component.clone(), component.clone(), component])
        }
    }
}

fn parse_component(value: &Value) -> Result<Component, String> {
    match value {
        Value::Number(number) => Ok(Component::Constant(number.as_f64().unwrap_or(0.0) as f32)),
        // A lone sign has no operand; the corpus uses it as a placeholder for zero.
        Value::String(source) if matches!(source.trim(), "-" | "+") => Ok(Component::Constant(0.0)),
        Value::String(source) => compile(source)
            .map(Component::Molang)
            .map_err(|err| format!("molang '{source}': {err}")),
        other => Err(format!("unsupported channel value {other}")),
    }
}

pub fn pick_clip(file: &AnimationFile, moving: bool) -> Option<&str> {
    let preferred: &[&str] = if moving {
        &["walk", "run", "move"]
    } else {
        &["idle"]
    };
    preferred
        .iter()
        .find_map(|word| {
            file.clips
                .keys()
                .find(|name| name.to_ascii_lowercase().contains(*word))
        })
        .or_else(|| file.clips.keys().next())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoValues;

    impl MolangContext for NoValues {
        fn query(&self, _name: &str) -> f32 {
            0.0
        }
    }

    struct CallerAnimTime;

    impl MolangContext for CallerAnimTime {
        fn query(&self, name: &str) -> f32 {
            if name == "query.anim_time" {
                999.0
            } else {
                0.0
            }
        }
    }

    fn parse(json: &str) -> AnimationFile {
        parse_animations(json).expect("animation JSON should parse")
    }

    fn single_clip(json: &str) -> Clip {
        parse(json).clips.into_values().next().expect("one clip")
    }

    fn bone_rotation(sample: &[(String, BoneSample)], bone: &str) -> [f32; 3] {
        sample
            .iter()
            .find(|(name, _)| name == bone)
            .expect("bone present")
            .1
            .rotation_deg
    }

    fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < 1e-4,
                "axis {axis}: expected {}, got {}",
                expected[axis],
                actual[axis]
            );
        }
    }

    fn clip_with_head_rotation(loop_field: &str, keys: &str) -> String {
        format!(
            r#"{{"format_version":"1.8.0","animations":{{"a":{{{loop_field}"bones":{{"head":{{"rotation":{keys}}}}}}}}}}}"#
        )
    }

    #[test]
    fn constant_channel_returns_its_vector_and_defaults_other_channels() {
        let clip = single_clip(&clip_with_head_rotation("", "[10,20,30]"));
        let sample = clip.sample(0.5, &NoValues);
        assert_close(bone_rotation(&sample, "head"), [10.0, 20.0, 30.0]);
        assert_eq!(sample[0].1.position, [0.0; 3]);
        assert_eq!(sample[0].1.scale, [1.0; 3]);
    }

    #[test]
    fn linear_interpolation_returns_midpoint() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":{"vector":[0,0,0]},"1.0":{"vector":[10,0,0]}}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.5, &NoValues), "head"),
            [5.0, 0.0, 0.0],
        );
    }

    #[test]
    fn step_lerp_mode_holds_previous_value() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":{"vector":[0,0,0],"lerp_mode":"step"},"1.0":{"vector":[10,0,0]}}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.5, &NoValues), "head"),
            [0.0; 3],
        );
    }

    #[test]
    fn catmullrom_hits_keyframes_at_segment_endpoints() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":{"vector":[0,0,0],"lerp_mode":"catmullrom"},"1.0":{"vector":[10,0,0],"lerp_mode":"catmullrom"},"2.0":{"vector":[0,0,0]}}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.0, &NoValues), "head"),
            [0.0; 3],
        );
        assert_close(
            bone_rotation(&clip.sample(1.0, &NoValues), "head"),
            [10.0, 0.0, 0.0],
        );
    }

    #[test]
    fn easing_shapes_the_segment_leaving_the_keyframe() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":{"vector":[0,0,0],"easing":"easeInQuad"},"1.0":{"vector":[1,0,0]}}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.5, &NoValues), "head"),
            [0.25, 0.0, 0.0],
        );
    }

    #[test]
    fn unknown_easing_falls_back_to_linear() {
        assert_eq!(
            parse_easing(Some(&Value::String("wobbly".into()))),
            Easing::Linear
        );
    }

    #[test]
    fn loop_wraps_time_by_animation_length() {
        let clip = single_clip(
            r#"{"animations":{"a":{"loop":true,"animation_length":1.0,"bones":{"head":{"rotation":{"0.0":[0,0,0],"1.0":[10,0,0]}}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(1.5, &NoValues), "head"),
            [5.0, 0.0, 0.0],
        );
        assert_close(
            bone_rotation(&clip.sample(3.5, &NoValues), "head"),
            [5.0, 0.0, 0.0],
        );
        assert!(!clip.is_finished(100.0));
    }

    #[test]
    fn hold_on_last_frame_clamps_and_never_finishes() {
        let clip = single_clip(
            r#"{"animations":{"a":{"loop":"hold_on_last_frame","animation_length":1.0,"bones":{"head":{"rotation":{"0.0":[0,0,0],"1.0":[10,0,0]}}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(5.0, &NoValues), "head"),
            [10.0, 0.0, 0.0],
        );
        assert!(!clip.is_finished(5.0));
    }

    #[test]
    fn non_looping_clip_clamps_and_reports_finished() {
        let clip = single_clip(
            r#"{"animations":{"a":{"loop":false,"animation_length":1.0,"bones":{"head":{"rotation":{"0.0":[0,0,0],"1.0":[10,0,0]}}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(5.0, &NoValues), "head"),
            [10.0, 0.0, 0.0],
        );
        assert!(!clip.is_finished(0.5));
        assert!(clip.is_finished(1.0));
    }

    #[test]
    fn length_falls_back_to_last_keyframe_time() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":[0,0,0],"2.5":[1,0,0]}"#,
        ));
        assert_eq!(clip.length(), 2.5);
    }

    #[test]
    fn scalar_channel_applies_to_all_three_axes() {
        let clip = single_clip(&clip_with_head_rotation("", "15"));
        assert_close(
            bone_rotation(&clip.sample(0.0, &NoValues), "head"),
            [15.0; 3],
        );
    }

    #[test]
    fn pre_and_post_values_create_discontinuity_at_keyframe() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"0.0":[0,0,0],"1.0":{"pre":[2,0,0],"post":[8,0,0]},"2.0":[0,0,0]}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.5, &NoValues), "head"),
            [1.0, 0.0, 0.0],
        );
        assert_close(
            bone_rotation(&clip.sample(1.0, &NoValues), "head"),
            [8.0, 0.0, 0.0],
        );
    }

    #[test]
    fn molang_channel_uses_clip_local_anim_time_over_caller_value() {
        let clip = single_clip(
            r#"{"animations":{"a":{"animation_length":1.0,"bones":{"head":{"rotation":["query.anim_time * 10", 0, 0]}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(0.3, &CallerAnimTime), "head"),
            [3.0, 0.0, 0.0],
        );
    }

    #[test]
    fn molang_channel_needs_declared_length_to_use_time() {
        let clip = single_clip(
            r#"{"animations":{"a":{"animation_length":1.0,"bones":{"head":{"rotation":["query.anim_time * 10", 0, 0]}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(0.3, &NoValues), "head"),
            [3.0, 0.0, 0.0],
        );
    }

    #[test]
    fn timeline_and_effects_are_ignored() {
        let clip = single_clip(
            r#"{"animations":{"a":{"loop":true,"timeline":{"0.2":"closeHandR;"},"sound_effects":{},"bones":{"head":{"relative_to":{},"rotation":[1,2,3]}}}}}"#,
        );
        assert_close(
            bone_rotation(&clip.sample(0.0, &NoValues), "head"),
            [1.0, 2.0, 3.0],
        );
    }

    #[test]
    fn channel_given_as_single_keyframe_object_is_taken_at_time_zero() {
        let clip = single_clip(&clip_with_head_rotation(
            "",
            r#"{"vector":[4,5,6],"easing":"easeInOutSine"}"#,
        ));
        assert_close(
            bone_rotation(&clip.sample(0.0, &NoValues), "head"),
            [4.0, 5.0, 6.0],
        );
    }

    #[test]
    fn lone_sign_component_reads_as_zero() {
        let clip = single_clip(&clip_with_head_rotation("", r#"[0, "-", 2]"#));
        assert_close(
            bone_rotation(&clip.sample(0.0, &NoValues), "head"),
            [0.0, 0.0, 2.0],
        );
    }

    #[test]
    fn invalid_json_and_molang_fail_to_parse() {
        assert!(parse_animations("{").is_err());
        assert!(parse_animations(&clip_with_head_rotation("", r#"["1 +", 0, 0]"#)).is_err());
        assert!(parse_animations(&clip_with_head_rotation("", r#"{"x":[0,0,0]}"#)).is_err());
    }

    #[test]
    fn pick_clip_prefers_idle_when_still_and_walk_when_moving() {
        let file = parse(r#"{"animations":{"attack":{},"idle":{},"run":{},"walk":{}}}"#);
        assert_eq!(pick_clip(&file, false), Some("idle"));
        assert_eq!(pick_clip(&file, true), Some("walk"));
    }

    #[test]
    fn pick_clip_falls_back_to_run_then_first_clip() {
        let running = parse(r#"{"animations":{"attack":{},"sprint_run":{}}}"#);
        assert_eq!(pick_clip(&running, true), Some("sprint_run"));
        let stationary = parse(r#"{"animations":{"attack":{},"sprint_run":{}}}"#);
        assert_eq!(pick_clip(&stationary, false), Some("attack"));
        let empty = parse(r#"{"animations":{}}"#);
        assert_eq!(pick_clip(&empty, true), None);
    }
}
