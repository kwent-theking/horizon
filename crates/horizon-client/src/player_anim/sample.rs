//! Sampling clips the way `KeyframeAnimationPlayer` does: integer ticks plus a
//! partial tick, source and destination keys, boundary keys that blend toward
//! the underlying pose, loop wrap-around between `returnTick` and `endTick`.

use super::format::{AXIS_COUNT, AnimationClip, AxisTrack, Bone, Keyframe, default_pose};
use super::layer::{ChannelMask, PlayerPose};
use std::sync::Arc;

/// The timing fields of a clip the sampler reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub begin: i32,
    pub end: i32,
    pub stop: i32,
    pub return_tick: i32,
    pub infinite: bool,
    pub ease_before: bool,
}

impl Timing {
    pub fn of(clip: &AnimationClip) -> Self {
        Self {
            begin: clip.begin_tick,
            end: clip.end_tick,
            stop: clip.stop_tick,
            return_tick: clip.return_tick,
            infinite: clip.looped,
            ease_before: clip.ease_before,
        }
    }

    /// Ticks one loop lap covers (`endTick - returnTick + 1`).
    fn lap(&self) -> i32 {
        self.end - self.return_tick + 1
    }
}

/// Index of the last key at or before `tick`, `-1` when none is.
fn find_at_tick(keys: &[Keyframe], tick: i32) -> isize {
    keys.iter()
        .rposition(|k| k.tick <= tick)
        .map_or(-1, |i| i as isize)
}

fn boundary(tick: i32, value: f32) -> Keyframe {
    Keyframe::plain(tick, value)
}

fn find_before(
    keys: &[Keyframe],
    index: isize,
    t: &Timing,
    cur: i32,
    default: f32,
    underlying: f32,
) -> Keyframe {
    if index < 0 {
        return if cur < t.begin {
            boundary(0, underlying)
        } else if cur < t.end {
            boundary(t.begin, default)
        } else {
            boundary(t.end, default)
        };
    }
    let key = &keys[index as usize];
    if !t.infinite && cur >= t.end && index as usize == keys.len() - 1 && key.tick < t.end {
        return Keyframe {
            tick: t.end,
            ..key.clone()
        };
    }
    key.clone()
}

fn find_after(
    keys: &[Keyframe],
    index: isize,
    t: &Timing,
    cur: i32,
    default: f32,
    underlying: f32,
) -> Keyframe {
    let next = (index + 1) as usize;
    if keys.len() > next {
        return keys[next].clone();
    }
    if t.infinite {
        return boundary(t.end + 1, default);
    }
    if cur < t.end
        && let Some(last) = keys.last()
    {
        return Keyframe {
            tick: t.end,
            ..last.clone()
        };
    }
    if cur >= t.end {
        boundary(t.stop, underlying)
    } else if cur >= t.begin {
        boundary(t.end, default)
    } else {
        boundary(t.begin, default)
    }
}

fn value_between(
    source: &Keyframe,
    destination: &Keyframe,
    t: &Timing,
    cur: i32,
    delta: f32,
) -> f32 {
    let (mut from, mut to) = (source.tick, destination.tick);
    if from >= to {
        // The pair straddles the loop point.
        if cur < from {
            from -= t.lap();
        } else {
            to += t.lap();
        }
    }
    if from == to {
        return source.value;
    }
    let progress = (cur as f32 + delta - from as f32) / (to - from) as f32;
    let key = if t.ease_before { destination } else { source };
    let eased = key.ease.apply(progress, key.arg);
    source.value + (destination.value - source.value) * eased
}

/// One channel's value at tick `cur` plus `delta`. `default` is the channel's
/// neutral value and `underlying` the value of the pose below the layer.
pub fn sample_axis(
    track: &AxisTrack,
    default: f32,
    t: &Timing,
    cur: i32,
    delta: f32,
    loop_started: bool,
    underlying: f32,
) -> f32 {
    let keys = &track.keyframes;
    if keys.is_empty() {
        return underlying;
    }
    let index = find_at_tick(keys, cur);
    let mut source = find_before(keys, index, t, cur, default, underlying);
    if loop_started && source.tick < t.return_tick {
        let at_end = find_at_tick(keys, t.end);
        source = find_before(keys, at_end, t, cur, default, underlying);
    }
    let mut destination = find_after(keys, index, t, cur, default, underlying);
    if t.infinite && destination.tick > t.end {
        let before_return = find_at_tick(keys, t.return_tick - 1);
        destination = find_after(keys, before_return, t, cur, default, underlying);
    }
    value_between(&source, &destination, t, cur, delta)
}

/// Playback speed over the wall-clock ticks since the clip started: the last
/// step whose start is not after the elapsed time applies; empty is 1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeedSchedule {
    pub steps: Vec<(f32, f32)>,
}

impl SpeedSchedule {
    pub fn at(&self, elapsed: f32) -> f32 {
        self.steps
            .iter()
            .rev()
            .find(|(from, _)| *from <= elapsed)
            .map_or(1.0, |(_, speed)| *speed)
    }
}

/// A clip being played for one player layer.
#[derive(Clone, Debug)]
pub struct Playback {
    pub clip: Arc<AnimationClip>,
    time: f32,
    elapsed: f32,
    speed: f32,
    schedule: SpeedSchedule,
    loop_started: bool,
    finished: bool,
}

impl Playback {
    pub fn new(clip: Arc<AnimationClip>) -> Self {
        Self::with_schedule(clip, SpeedSchedule::default())
    }

    pub fn with_schedule(clip: Arc<AnimationClip>, schedule: SpeedSchedule) -> Self {
        let speed = schedule.at(0.0);
        Self {
            clip,
            time: 0.0,
            elapsed: 0.0,
            speed,
            schedule,
            loop_started: false,
            finished: false,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The whole ticks played so far (`getTick`).
    pub fn tick_index(&self) -> i32 {
        self.time.floor() as i32
    }

    /// Advances one client tick. Render passes never call this.
    pub fn tick(&mut self) {
        if self.finished {
            return;
        }
        let timing = Timing::of(&self.clip);
        self.time += self.speed;
        self.elapsed += 1.0;
        self.speed = self.schedule.at(self.elapsed);
        if timing.infinite && self.tick_index() > timing.end {
            self.time -= timing.lap() as f32;
            self.loop_started = true;
        }
        if self.tick_index() >= timing.stop {
            self.finished = true;
        }
    }

    /// Writes this clip's enabled, unmasked channels over `pose` at the
    /// fraction `alpha` of the current tick.
    pub fn evaluate(&self, alpha: f32, mask: &ChannelMask, pose: &mut PlayerPose) {
        let timing = Timing::of(&self.clip);
        let at = self.time + alpha * self.speed;
        let mut cur = at.floor() as i32;
        let delta = at - cur as f32;
        if timing.infinite && cur > timing.end {
            cur -= timing.lap();
        }
        for bone in Bone::ALL {
            let default = default_pose(bone);
            for axis in 0..AXIS_COUNT {
                let track = self.clip.track(bone, axis);
                if !track.is_enabled() || mask.is_disabled(bone, axis) {
                    continue;
                }
                let slot = &mut pose.bones[bone.index()][axis];
                *slot = sample_axis(
                    track,
                    default[axis],
                    &timing,
                    cur,
                    delta,
                    self.loop_started,
                    *slot,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_anim::ease::Ease;
    use crate::player_anim::format::{AXIS_YAW, parse_emotecraft, parse_gecko};

    fn track(keys: &[(i32, f32)]) -> AxisTrack {
        AxisTrack {
            keyframes: keys.iter().map(|(t, v)| Keyframe::plain(*t, *v)).collect(),
        }
    }

    fn timing(begin: i32, end: i32, stop: i32, ret: i32, infinite: bool) -> Timing {
        Timing {
            begin,
            end,
            stop,
            return_tick: ret,
            infinite,
            ease_before: false,
        }
    }

    #[test]
    fn interpolates_between_keys_with_the_partial_tick() {
        let t = timing(0, 10, 13, 0, false);
        let track = track(&[(0, 0.0), (10, 1.0)]);
        let v = sample_axis(&track, 0.0, &t, 4, 0.5, false, 0.0);
        assert!((v - 0.45).abs() < 1e-6);
    }

    #[test]
    fn blends_in_from_the_underlying_pose_before_the_first_key() {
        let t = timing(8, 20, 30, 0, false);
        let track = track(&[(8, 1.0), (20, 1.0)]);
        // Tick 0..8 runs from the underlying value to the first key.
        let start = sample_axis(&track, 0.0, &t, 0, 0.0, false, 0.5);
        assert!((start - 0.5).abs() < 1e-6);
        let mid = sample_axis(&track, 0.0, &t, 4, 0.0, false, 0.5);
        assert!((mid - 0.75).abs() < 1e-6);
        let at_key = sample_axis(&track, 0.0, &t, 8, 0.0, false, 0.5);
        assert!((at_key - 1.0).abs() < 1e-6);
    }

    #[test]
    fn returns_to_the_underlying_pose_between_end_and_stop() {
        let t = timing(0, 10, 20, 0, false);
        let track = track(&[(0, 1.0), (10, 1.0)]);
        let held = sample_axis(&track, 0.0, &t, 10, 0.0, false, 0.0);
        assert!((held - 1.0).abs() < 1e-6);
        let halfway = sample_axis(&track, 0.0, &t, 15, 0.0, false, 0.0);
        assert!((halfway - 0.5).abs() < 1e-6);
    }

    #[test]
    fn loops_wrap_between_return_and_end() {
        // Keys at 0 and 10 loop back to tick 0: the lap is 11 ticks and the
        // wrap segment (10 -> 0) interpolates over one tick.
        let t = timing(0, 10, 13, 0, true);
        let track = track(&[(0, 0.0), (5, 1.0), (10, 0.0)]);
        let inside = sample_axis(&track, 0.0, &t, 5, 0.0, false, 0.0);
        assert!((inside - 1.0).abs() < 1e-6);
        let after_wrap = sample_axis(&track, 0.0, &t, 0, 0.0, true, 0.0);
        assert!(after_wrap.abs() < 1e-6);
    }

    #[test]
    fn playback_loops_and_finishes() {
        let clips = parse_gecko(
            r#"{"animations":{"spin":{"loop":true,"animation_length":0.5,
                "bones":{"head":{"rotation":{"0":[0,0,0],"0.4":[0,90,0]}}}}}}"#,
        )
        .unwrap();
        let mut play = Playback::new(Arc::new(clips[0].clone()));
        // end_tick is 10 - 1 = 9, so a lap is 10 ticks.
        for _ in 0..25 {
            play.tick();
        }
        assert!(!play.is_finished());
        assert!(play.tick_index() <= 9);

        let one_shot = parse_emotecraft(
            r#"{"emote":{"endTick":4,"stopTick":6,"moves":[{"tick":0,"head":{"yaw":1}}]}}"#,
        )
        .unwrap();
        let mut play = Playback::new(Arc::new(one_shot));
        for _ in 0..5 {
            play.tick();
        }
        assert!(!play.is_finished());
        play.tick();
        assert!(play.is_finished());
    }

    #[test]
    fn evaluate_writes_only_enabled_channels() {
        let clip = parse_emotecraft(
            r#"{"emote":{"endTick":10,"degrees":false,"moves":[
                {"tick":0,"head":{"yaw":2}},{"tick":10,"head":{"yaw":2}}]}}"#,
        )
        .unwrap();
        let play = Playback::new(Arc::new(clip));
        let mut pose = PlayerPose::neutral();
        pose.bones[Bone::Head.index()][3] = 0.7;
        play.evaluate(0.0, &ChannelMask::default(), &mut pose);
        assert!((pose.bones[Bone::Head.index()][AXIS_YAW] - 2.0).abs() < 1e-6);
        assert_eq!(pose.bones[Bone::Head.index()][3], 0.7);
    }

    #[test]
    fn schedule_changes_the_playback_rate() {
        let clip = parse_emotecraft(r#"{"emote":{"endTick":40,"moves":[]}}"#).unwrap();
        let schedule = SpeedSchedule {
            steps: vec![(0.0, 2.0), (5.0, 1.0)],
        };
        let mut play = Playback::with_schedule(Arc::new(clip), schedule);
        for _ in 0..5 {
            play.tick();
        }
        assert_eq!(play.tick_index(), 10);
        play.tick();
        assert_eq!(play.tick_index(), 11);
    }

    #[test]
    fn easing_source_or_destination_key() {
        let mut slow = Keyframe::plain(0, 0.0);
        slow.ease = Ease::parse("inquad").unwrap();
        let fast = Keyframe::plain(10, 1.0);
        let track = AxisTrack {
            keyframes: vec![slow, fast],
        };
        let mut t = timing(0, 10, 13, 0, false);
        let source_eased = sample_axis(&track, 0.0, &t, 5, 0.0, false, 0.0);
        assert!((source_eased - 0.25).abs() < 1e-6);
        t.ease_before = true;
        let destination_linear = sample_axis(&track, 0.0, &t, 5, 0.0, false, 0.0);
        assert!((destination_linear - 0.5).abs() < 1e-6);
    }
}
