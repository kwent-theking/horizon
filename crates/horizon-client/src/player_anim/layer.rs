//! The pose a stack of animation layers works on, and the layers themselves.
//!
//! A layer holds the clip it plays and, while a fade runs, the clip it
//! replaces. Layers apply in ascending priority; each one turns the pose below
//! it into a new pose. Ticking advances state once per client tick, evaluating
//! never does, so every render consumer sees the same pose at the same partial
//! tick.

use super::ease::Ease;
use super::format::{
    AXIS_COUNT, AXIS_PITCH, AXIS_ROLL, AXIS_X, AXIS_YAW, BONE_COUNT, Bone, default_pose,
};
use super::sample::Playback;

/// Every bone's nine channels in `Poses` layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPose {
    pub bones: [[f32; AXIS_COUNT]; BONE_COUNT],
}

impl PlayerPose {
    pub fn neutral() -> Self {
        Self {
            bones: Bone::ALL.map(default_pose),
        }
    }

    pub fn bone(&self, bone: Bone) -> &[f32; AXIS_COUNT] {
        &self.bones[bone.index()]
    }

    /// Left and right swapped, x and the y/z rotations negated
    /// (`MirrorModifier`). Mirroring twice is the identity.
    pub fn mirrored(&self) -> Self {
        let mut out = *self;
        for bone in Bone::ALL {
            let mut channels = self.bones[bone.index()];
            channels[AXIS_X] = -channels[AXIS_X];
            channels[AXIS_YAW] = -channels[AXIS_YAW];
            channels[AXIS_ROLL] = -channels[AXIS_ROLL];
            out.bones[bone.mirrored().index()] = channels;
        }
        out
    }

    pub fn lerp(&self, other: &Self, weight: f32) -> Self {
        let mut out = *self;
        for (bone, channels) in out.bones.iter_mut().enumerate() {
            for (axis, value) in channels.iter_mut().enumerate() {
                *value += (other.bones[bone][axis] - *value) * weight;
            }
        }
        out
    }
}

/// Channels a layer must leave to the pose below it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelMask {
    disabled: [[bool; AXIS_COUNT]; BONE_COUNT],
}

impl ChannelMask {
    pub fn disable(&mut self, bone: Bone, axis: usize) {
        self.disabled[bone.index()][axis] = true;
    }

    pub fn is_disabled(&self, bone: Bone, axis: usize) -> bool {
        self.disabled[bone.index()][axis]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstPersonMode {
    None,
    Vanilla,
    ThirdPersonModel,
    Disabled,
}

/// What first person shows while an animation runs (`FirstPersonConfiguration`).
/// The default hides both arms and shows both items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstPersonConfig {
    pub mode: FirstPersonMode,
    pub show_right_arm: bool,
    pub show_left_arm: bool,
    pub show_right_item: bool,
    pub show_left_item: bool,
}

impl Default for FirstPersonConfig {
    fn default() -> Self {
        Self {
            mode: FirstPersonMode::None,
            show_right_arm: false,
            show_left_arm: false,
            show_right_item: true,
            show_left_item: true,
        }
    }
}

impl FirstPersonConfig {
    pub fn third_person_model() -> Self {
        Self {
            mode: FirstPersonMode::ThirdPersonModel,
            ..Self::default()
        }
    }

    pub fn mirrored(self) -> Self {
        Self {
            mode: self.mode,
            show_right_arm: self.show_left_arm,
            show_left_arm: self.show_right_arm,
            show_right_item: self.show_left_item,
            show_left_item: self.show_right_item,
        }
    }
}

/// Where the player looks, for the adjustments that follow the aim.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimContext {
    /// Interpolated player pitch in radians (negative looks up).
    pub pitch: f32,
}

/// Share of the player's pitch Iron's Spells gives the casting arms.
pub const AIM_ARM_FACTOR: f32 = 0.65;

/// A clip and how it is applied.
#[derive(Clone, Debug)]
pub struct ActiveClip {
    pub playback: Playback,
    pub mirror: bool,
    pub mask: ChannelMask,
    pub first_person: FirstPersonConfig,
    /// The arms follow the player's pitch (Iron's Spells casting).
    pub aim_arms: bool,
}

impl ActiveClip {
    pub fn new(playback: Playback) -> Self {
        Self {
            playback,
            mirror: false,
            mask: ChannelMask::default(),
            first_person: FirstPersonConfig::default(),
            aim_arms: false,
        }
    }

    pub fn apply(&self, below: &PlayerPose, alpha: f32, aim: AimContext) -> PlayerPose {
        let mut pose = if self.mirror {
            below.mirrored()
        } else {
            *below
        };
        self.playback.evaluate(alpha, &self.mask, &mut pose);
        if self.aim_arms {
            for arm in [Bone::RightArm, Bone::LeftArm] {
                pose.bones[arm.index()][AXIS_PITCH] += aim.pitch * AIM_ARM_FACTOR;
            }
        }
        if self.mirror { pose.mirrored() } else { pose }
    }
}

#[derive(Clone, Copy, Debug)]
struct Fade {
    total: i32,
    elapsed: i32,
    ease: Ease,
}

impl Fade {
    fn weight(&self, alpha: f32) -> f32 {
        if self.total <= 0 {
            return 1.0;
        }
        let progress = ((self.elapsed as f32 + alpha) / self.total as f32).clamp(0.0, 1.0);
        self.ease.apply(progress, None)
    }

    fn is_done(&self) -> bool {
        self.elapsed >= self.total
    }
}

/// One priority level: the clip it plays, and the one it is fading out.
#[derive(Clone, Debug)]
pub struct AnimationLayer {
    pub priority: i32,
    current: Option<ActiveClip>,
    outgoing: Option<ActiveClip>,
    fade: Option<Fade>,
}

impl AnimationLayer {
    pub fn new(priority: i32) -> Self {
        Self {
            priority,
            current: None,
            outgoing: None,
            fade: None,
        }
    }

    pub fn current(&self) -> Option<&ActiveClip> {
        self.current.as_ref()
    }

    pub fn is_idle(&self) -> bool {
        self.current.is_none() && self.outgoing.is_none()
    }

    /// Replaces the clip, fading from the old one over `fade_ticks`.
    pub fn play(&mut self, clip: ActiveClip, fade_ticks: i32, ease: Ease) {
        if fade_ticks > 0 {
            self.outgoing = self.current.take();
            self.fade = Some(Fade {
                total: fade_ticks,
                elapsed: 0,
                ease,
            });
        } else {
            self.outgoing = None;
            self.fade = None;
        }
        self.current = Some(clip);
    }

    /// Fades the clip out to the pose below (or drops it at once).
    pub fn cancel(&mut self, fade_ticks: i32, ease: Ease) {
        if fade_ticks > 0 && self.current.is_some() {
            self.outgoing = self.current.take();
            self.fade = Some(Fade {
                total: fade_ticks,
                elapsed: 0,
                ease,
            });
        } else {
            self.current = None;
            self.outgoing = None;
            self.fade = None;
        }
    }

    pub fn tick(&mut self) {
        if let Some(clip) = self.current.as_mut() {
            clip.playback.tick();
        }
        if let Some(clip) = self.outgoing.as_mut() {
            clip.playback.tick();
        }
        if let Some(fade) = self.fade.as_mut() {
            fade.elapsed += 1;
            if fade.is_done() {
                self.fade = None;
                self.outgoing = None;
            }
        }
        if self.current.as_ref().is_some_and(|c| c.playback.is_finished()) {
            self.current = None;
        }
    }

    pub fn evaluate(&self, alpha: f32, aim: AimContext, pose: &mut PlayerPose) {
        let below = *pose;
        let new = self
            .current
            .as_ref()
            .map_or(below, |c| c.apply(&below, alpha, aim));
        *pose = match &self.fade {
            Some(fade) => {
                let old = self
                    .outgoing
                    .as_ref()
                    .map_or(below, |c| c.apply(&below, alpha, aim));
                old.lerp(&new, fade.weight(alpha))
            }
            None => new,
        };
    }
}

/// A player's layers, lowest priority first.
#[derive(Clone, Debug, Default)]
pub struct AnimationStack {
    layers: Vec<AnimationLayer>,
}

impl AnimationStack {
    pub fn layer_mut(&mut self, priority: i32) -> &mut AnimationLayer {
        let index = match self.layers.iter().position(|l| l.priority >= priority) {
            Some(i) if self.layers[i].priority == priority => i,
            Some(i) => {
                self.layers.insert(i, AnimationLayer::new(priority));
                i
            }
            None => {
                self.layers.push(AnimationLayer::new(priority));
                self.layers.len() - 1
            }
        };
        &mut self.layers[index]
    }

    pub fn layer(&self, priority: i32) -> Option<&AnimationLayer> {
        self.layers.iter().find(|l| l.priority == priority)
    }

    pub fn is_idle(&self) -> bool {
        self.layers.iter().all(AnimationLayer::is_idle)
    }

    pub fn tick(&mut self) {
        for layer in &mut self.layers {
            layer.tick();
        }
        self.layers.retain(|l| !l.is_idle());
    }

    pub fn evaluate(&self, alpha: f32, aim: AimContext, pose: &mut PlayerPose) {
        for layer in &self.layers {
            layer.evaluate(alpha, aim, pose);
        }
    }

    /// The first-person configuration of the highest-priority playing clip.
    pub fn first_person(&self) -> Option<FirstPersonConfig> {
        self.layers
            .iter()
            .rev()
            .find_map(|l| l.current())
            .map(|c| c.first_person)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_anim::format::{AXIS_Z, parse_emotecraft};
    use std::sync::Arc;

    fn clip_with_yaw(value: f32) -> ActiveClip {
        let text = format!(
            r#"{{"emote":{{"endTick":20,"stopTick":40,"degrees":false,"isLoop":true,"returnTick":0,
            "moves":[{{"tick":0,"head":{{"yaw":{value}}}}},{{"tick":20,"head":{{"yaw":{value}}}}}]}}}}"#
        );
        let clip = parse_emotecraft(&text).unwrap();
        ActiveClip::new(Playback::new(Arc::new(clip)))
    }

    fn head_yaw(stack: &AnimationStack, alpha: f32) -> f32 {
        let mut pose = PlayerPose::neutral();
        stack.evaluate(alpha, AimContext::default(), &mut pose);
        pose.bone(Bone::Head)[AXIS_YAW]
    }

    #[test]
    fn mirroring_swaps_sides_and_is_an_involution() {
        let mut pose = PlayerPose::neutral();
        pose.bones[Bone::RightArm.index()][AXIS_PITCH] = 1.0;
        pose.bones[Bone::RightArm.index()][AXIS_YAW] = 0.5;
        pose.bones[Bone::RightArm.index()][AXIS_Z] = 3.0;
        let mirrored = pose.mirrored();
        assert_eq!(mirrored.bone(Bone::LeftArm)[AXIS_PITCH], 1.0);
        assert_eq!(mirrored.bone(Bone::LeftArm)[AXIS_YAW], -0.5);
        assert_eq!(mirrored.bone(Bone::LeftArm)[AXIS_Z], 3.0);
        // The right arm's pivot x of -5 becomes the left arm's +5.
        assert_eq!(mirrored.bone(Bone::LeftArm)[AXIS_X], 5.0);
        assert_eq!(mirrored.mirrored(), pose);
    }

    #[test]
    fn a_new_clip_fades_in_over_the_given_ticks() {
        let mut stack = AnimationStack::default();
        stack
            .layer_mut(42)
            .play(clip_with_yaw(1.0), 4, Ease::Linear);
        // Start of the fade: still the pose below.
        let start = head_yaw(&stack, 0.0);
        assert!(start.abs() < 1e-5);
        for _ in 0..2 {
            stack.tick();
        }
        let middle = head_yaw(&stack, 0.0);
        assert!(middle > 0.2 && middle < 0.8, "{middle}");
        for _ in 0..3 {
            stack.tick();
        }
        assert!((head_yaw(&stack, 0.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn replacing_a_clip_fades_between_the_two() {
        let mut stack = AnimationStack::default();
        stack.layer_mut(42).play(clip_with_yaw(1.0), 0, Ease::Linear);
        stack.layer_mut(42).play(clip_with_yaw(3.0), 4, Ease::Linear);
        let start = head_yaw(&stack, 0.0);
        assert!((start - 1.0).abs() < 1e-5);
        for _ in 0..4 {
            stack.tick();
        }
        assert!((head_yaw(&stack, 0.0) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn cancel_fades_back_to_the_pose_below_and_goes_idle() {
        let mut stack = AnimationStack::default();
        stack.layer_mut(42).play(clip_with_yaw(1.0), 0, Ease::Linear);
        stack.tick();
        stack.layer_mut(42).cancel(4, Ease::Linear);
        for _ in 0..2 {
            stack.tick();
        }
        let middle = head_yaw(&stack, 0.0);
        assert!(middle > 0.2 && middle < 0.8, "{middle}");
        for _ in 0..3 {
            stack.tick();
        }
        assert!(stack.is_idle());
        assert!(head_yaw(&stack, 0.0).abs() < 1e-6);
    }

    #[test]
    fn higher_priority_layers_apply_last() {
        let mut stack = AnimationStack::default();
        stack.layer_mut(2000).play(clip_with_yaw(2.0), 0, Ease::Linear);
        stack.layer_mut(42).play(clip_with_yaw(1.0), 0, Ease::Linear);
        assert!((head_yaw(&stack, 0.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn first_person_follows_the_top_clip_and_mirrors() {
        let mut stack = AnimationStack::default();
        let mut clip = clip_with_yaw(1.0);
        clip.first_person = FirstPersonConfig {
            show_right_arm: true,
            ..FirstPersonConfig::third_person_model()
        };
        stack.layer_mut(42).play(clip, 0, Ease::Linear);
        let config = stack.first_person().unwrap();
        assert_eq!(config.mode, FirstPersonMode::ThirdPersonModel);
        assert!(config.mirrored().show_left_arm && !config.mirrored().show_right_arm);
    }

    #[test]
    fn aiming_adds_a_share_of_the_pitch_to_the_arms() {
        let mut clip = clip_with_yaw(0.0);
        clip.aim_arms = true;
        let mut stack = AnimationStack::default();
        stack.layer_mut(42).play(clip, 0, Ease::Linear);
        let mut pose = PlayerPose::neutral();
        stack.evaluate(0.0, AimContext { pitch: -1.0 }, &mut pose);
        let pitch = pose.bone(Bone::RightArm)[AXIS_PITCH];
        assert!((pitch + AIM_ARM_FACTOR).abs() < 1e-6);
    }
}
