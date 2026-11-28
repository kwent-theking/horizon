//! Better Combat: attack clips on layer 2000, weapon poses on layers 1 to 4.
//!
//! An attack plays at the rate that fits the clip into the packet's `length`
//! ticks, then at normal speed while it winds down. The mod's separate upswing
//! multiplier schedule is not reproduced.

use crate::player_anim::ease::Ease;
use crate::player_anim::format::{AXIS_PITCH, Bone};
use crate::player_anim::layer::{ActiveClip, FirstPersonConfig};
use crate::player_anim::library::AnimationLibrary;
use crate::player_anim::payload::{AnimatedHand, BetterCombatAttack};
use crate::player_anim::sample::{Playback, SpeedSchedule};
use crate::player_anim::state::PlayerAnimator;

pub const ATTACK_PRIORITY: i32 = 2000;
pub const POSE_PRIORITIES: std::ops::RangeInclusive<i32> = 1..=4;
pub const POSE_FADE_TICKS: i32 = 5;
const NAMESPACE: &str = "bettercombat";

fn clip_id(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("{NAMESPACE}:{name}")
    }
}

/// Plays a received or locally started attack; false when its clip is unknown.
pub fn play_attack(
    animator: &mut PlayerAnimator,
    library: &AnimationLibrary,
    attack: &BetterCombatAttack,
) -> bool {
    let Some(clip) = library.get(&clip_id(&attack.animation)) else {
        return false;
    };
    let length = attack.length.max(1.0);
    let rate = clip.end_tick as f32 / length;
    let schedule = SpeedSchedule {
        steps: vec![(0.0, rate), (length, 1.0)],
    };
    let fade_ticks = clip.begin_tick;
    // Offhand swings mirror; a left-handed player mirrors them again.
    let mirror = (attack.hand == AnimatedHand::OffHand) != animator.left_handed;
    let mut active = ActiveClip::new(Playback::with_schedule(clip, schedule));
    active.mirror = mirror;
    active.mask.disable(Bone::Head, AXIS_PITCH);
    active.first_person = if mirror {
        FirstPersonConfig::third_person_model().mirrored()
    } else {
        FirstPersonConfig::third_person_model()
    };
    animator
        .stack
        .layer_mut(ATTACK_PRIORITY)
        .play(active, fade_ticks, Ease::IN_OUT_SINE);
    true
}

/// A stop packet fades the attack out over its `length` rounded to ticks.
pub fn stop_attack(animator: &mut PlayerAnimator, fade_length: f32) {
    let ticks = fade_length.round().max(0.0) as i32;
    animator
        .stack
        .layer_mut(ATTACK_PRIORITY)
        .cancel(ticks, Ease::IN_OUT_SINE);
}

/// Handles either kind of attack packet.
pub fn on_attack_packet(
    animator: &mut PlayerAnimator,
    library: &AnimationLibrary,
    attack: &BetterCombatAttack,
) {
    if attack.is_stop() {
        stop_attack(animator, attack.length);
    } else {
        play_attack(animator, library, attack);
    }
}

/// Sets the looping weapon pose on `priority` (1 to 4); `None` removes it.
pub fn update_weapon_pose(
    animator: &mut PlayerAnimator,
    library: &AnimationLibrary,
    priority: i32,
    pose: Option<&str>,
) -> bool {
    if !POSE_PRIORITIES.contains(&priority) {
        return false;
    }
    let layer = animator.stack.layer_mut(priority);
    let Some(name) = pose else {
        layer.cancel(POSE_FADE_TICKS, Ease::IN_OUT_SINE);
        return true;
    };
    let Some(clip) = library.get(&clip_id(name)) else {
        return false;
    };
    layer.play(
        ActiveClip::new(Playback::new(clip)),
        POSE_FADE_TICKS,
        Ease::IN_OUT_SINE,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_anim::format::parse_emotecraft;

    fn library() -> AnimationLibrary {
        let mut library = AnimationLibrary::default();
        let slash = r#"{"name":"slash","emote":{"beginTick":4,"endTick":20,"stopTick":30,
            "isLoop":"false","returnTick":0,"degrees":false,
            "moves":[{"tick":4,"head":{"pitch":1}},{"tick":4,"head":{"yaw":1}},
                     {"tick":20,"head":{"yaw":2}}]}}"#;
        library.insert(NAMESPACE, parse_emotecraft(slash).unwrap());
        let pose = r#"{"name":"pose","emote":{"endTick":10,"isLoop":true,"returnTick":10,
            "moves":[{"tick":0,"rightArm":{"pitch":0}}]}}"#;
        library.insert(NAMESPACE, parse_emotecraft(pose).unwrap());
        library
    }

    fn attack(hand: AnimatedHand, name: &str, length: f32) -> BetterCombatAttack {
        BetterCombatAttack {
            player_id: 5,
            hand,
            animation: name.into(),
            length,
            upswing: 0.5,
            weapon_range: 3.0,
            upswing_ticks: 4,
            particles_json: String::new(),
        }
    }

    #[test]
    fn plays_an_attack_on_layer_2000_at_the_fitted_rate() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        assert!(play_attack(
            &mut animator,
            &library,
            &attack(AnimatedHand::MainHand, "bettercombat:slash", 10.0)
        ));
        let active = animator
            .stack
            .layer(ATTACK_PRIORITY)
            .unwrap()
            .current()
            .unwrap();
        assert!(!active.mirror);
        assert!(active.mask.is_disabled(Bone::Head, AXIS_PITCH));
        // 20 clip ticks over 10 ticks: two clip ticks per client tick.
        animator.stack.tick();
        animator.stack.tick();
        let active = animator
            .stack
            .layer(ATTACK_PRIORITY)
            .unwrap()
            .current()
            .unwrap();
        assert_eq!(active.playback.tick_index(), 4);
    }

    #[test]
    fn offhand_mirrors_and_left_handed_toggles_again() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        play_attack(&mut animator, &library, &attack(AnimatedHand::OffHand, "slash", 10.0));
        let mirrored = animator.stack.layer(ATTACK_PRIORITY).unwrap().current().unwrap().mirror;
        assert!(mirrored);
        animator.left_handed = true;
        play_attack(&mut animator, &library, &attack(AnimatedHand::OffHand, "slash", 10.0));
        let mirrored = animator.stack.layer(ATTACK_PRIORITY).unwrap().current().unwrap().mirror;
        assert!(!mirrored);
    }

    #[test]
    fn stop_sentinel_fades_the_attack_out() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        on_attack_packet(
            &mut animator,
            &library,
            &attack(AnimatedHand::MainHand, "slash", 10.0),
        );
        on_attack_packet(
            &mut animator,
            &library,
            &attack(AnimatedHand::MainHand, "!STOP!", 3.4),
        );
        assert!(
            animator
                .stack
                .layer(ATTACK_PRIORITY)
                .unwrap()
                .current()
                .is_none()
        );
        for _ in 0..4 {
            animator.stack.tick();
        }
        assert!(animator.stack.is_idle());
    }

    #[test]
    fn unknown_attacks_change_nothing() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        assert!(!play_attack(
            &mut animator,
            &library,
            &attack(AnimatedHand::MainHand, "nope", 10.0)
        ));
        assert!(animator.stack.is_idle());
    }

    #[test]
    fn weapon_poses_use_the_low_priorities() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        assert!(update_weapon_pose(&mut animator, &library, 3, Some("pose")));
        assert!(animator.stack.layer(3).unwrap().current().is_some());
        assert!(!update_weapon_pose(&mut animator, &library, 9, Some("pose")));
        assert!(update_weapon_pose(&mut animator, &library, 3, None));
    }
}
