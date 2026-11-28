//! Iron's Spells 'n Spellbooks: casting clips on layer 42.
//!
//! The start packet carries a spell id, and the mod resolves the clip through
//! Java (`AbstractSpell.getCastStartAnimation`), which the client cannot see.
//! The default here follows the mod's own defaults: a spell with a cast time is
//! a `long_cast`, any other an `instant_projectile`; its finish clip is
//! `<start>_finish` when the library has one. Spells that override either
//! answer to a [`SpellAnimationBinding`].

use crate::player_anim::ease::Ease;
use crate::player_anim::layer::{ActiveClip, FirstPersonConfig};
use crate::player_anim::library::AnimationLibrary;
use crate::player_anim::sample::Playback;
use crate::player_anim::state::PlayerAnimator;

pub const PRIORITY: i32 = 42;
pub const START_FADE_TICKS: i32 = 2;
pub const CANCEL_FADE_TICKS: i32 = 4;

pub const LONG_CAST: &str = "irons_spellbooks:long_cast";
pub const INSTANT_PROJECTILE: &str = "irons_spellbooks:instant_projectile";

/// What a successful finish does to the running clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpellFinish {
    /// `AnimationHolder.pass()`: keep the current clip.
    Pass,
    Cancel,
    Play(String),
}

/// Versioned per-spell overrides of the default clips.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpellAnimationBinding {
    pub start: Option<String>,
    pub finish: Option<SpellFinish>,
}

/// The clip a cast starts when no binding says otherwise.
pub fn default_start_clip(cast_time: Option<i32>) -> &'static str {
    match cast_time {
        Some(ticks) if ticks > 0 => LONG_CAST,
        _ => INSTANT_PROJECTILE,
    }
}

/// `casting slot is OFFHAND XOR the main arm is LEFT` (`AnimationHelper$1.isEnabled`).
pub fn mirror_for(casting_slot: &str, main_arm_left: bool) -> bool {
    casting_slot.eq_ignore_ascii_case("offhand") != main_arm_left
}

pub fn on_synced_player_data(animator: &mut PlayerAnimator, casting_slot: &str) {
    animator.mirror_casting = mirror_for(casting_slot, animator.left_handed);
}

/// Starts `clip_id` on the casting layer with the two-tick fade; false when
/// the library has no such clip.
pub fn start(animator: &mut PlayerAnimator, library: &AnimationLibrary, clip_id: &str) -> bool {
    let Some(clip) = library.get(clip_id) else {
        return false;
    };
    let mirror = animator.mirror_casting;
    let mut active = ActiveClip::new(Playback::new(clip));
    active.mirror = mirror;
    active.first_person = if mirror {
        FirstPersonConfig::third_person_model().mirrored()
    } else {
        FirstPersonConfig::third_person_model()
    };
    active.aim_arms = true;
    animator
        .stack
        .layer_mut(PRIORITY)
        .play(active, START_FADE_TICKS, Ease::IN_OUT_SINE);
    animator.casting_clip = Some(clip_id.to_owned());
    true
}

pub fn cancel(animator: &mut PlayerAnimator) {
    animator
        .stack
        .layer_mut(PRIORITY)
        .cancel(CANCEL_FADE_TICKS, Ease::IN_OUT_SINE);
    animator.casting_clip = None;
}

/// `on_cast_started`: returns the clip id it started.
pub fn on_cast_started(
    animator: &mut PlayerAnimator,
    library: &AnimationLibrary,
    binding: Option<&SpellAnimationBinding>,
    cast_time: Option<i32>,
) -> Option<String> {
    let clip_id = binding
        .and_then(|b| b.start.clone())
        .unwrap_or_else(|| default_start_clip(cast_time).to_owned());
    start(animator, library, &clip_id).then_some(clip_id)
}

fn default_finish(animator: &PlayerAnimator, library: &AnimationLibrary) -> SpellFinish {
    let Some(started) = animator.casting_clip.as_deref() else {
        return SpellFinish::Pass;
    };
    let finish = format!("{started}_finish");
    if library.contains(&finish) {
        SpellFinish::Play(finish)
    } else if library.get(started).is_some_and(|c| c.looped) {
        SpellFinish::Cancel
    } else {
        SpellFinish::Pass
    }
}

/// `on_cast_finished`: cancellation beats every finish behaviour.
pub fn on_cast_finished(
    animator: &mut PlayerAnimator,
    library: &AnimationLibrary,
    binding: Option<&SpellAnimationBinding>,
    cancelled: bool,
) {
    if cancelled {
        cancel(animator);
        return;
    }
    let finish = binding
        .and_then(|b| b.finish.clone())
        .unwrap_or_else(|| default_finish(animator, library));
    match finish {
        SpellFinish::Pass => {}
        SpellFinish::Cancel => cancel(animator),
        SpellFinish::Play(id) => {
            if !start(animator, library, &id) {
                cancel(animator);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_anim::format::parse_gecko;
    use crate::player_anim::layer::FirstPersonMode;

    fn library() -> AnimationLibrary {
        let text = r#"{"animations":{
            "long_cast":{"loop":true,"animation_length":1,"bones":{"right_arm":{"rotation":[90,0,0]}}},
            "long_cast_finish":{"animation_length":1,"bones":{"right_arm":{"rotation":[-90,0,0]}}},
            "instant_projectile":{"animation_length":1,"bones":{"right_arm":{"rotation":[45,0,0]}}},
            "continuous_thrust":{"loop":true,"animation_length":1,"bones":{"right_arm":{"rotation":[10,0,0]}}}}}"#;
        let mut library = AnimationLibrary::default();
        for clip in parse_gecko(text).unwrap() {
            library.insert("irons_spellbooks", clip);
        }
        library
    }

    #[test]
    fn mirror_is_offhand_xor_left_handed() {
        assert!(!mirror_for("mainhand", false));
        assert!(mirror_for("offhand", false));
        assert!(mirror_for("mainhand", true));
        assert!(!mirror_for("offhand", true));
    }

    #[test]
    fn cast_time_picks_the_default_clip() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        assert_eq!(
            on_cast_started(&mut animator, &library, None, Some(40)).as_deref(),
            Some(LONG_CAST)
        );
        assert_eq!(
            on_cast_started(&mut animator, &library, None, Some(0)).as_deref(),
            Some(INSTANT_PROJECTILE)
        );
        let layer = animator.stack.layer(PRIORITY).unwrap();
        let config = layer.current().unwrap().first_person;
        assert_eq!(config.mode, FirstPersonMode::ThirdPersonModel);
    }

    #[test]
    fn a_long_cast_finishes_with_its_finish_clip() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        on_cast_started(&mut animator, &library, None, Some(40));
        on_cast_finished(&mut animator, &library, None, false);
        assert_eq!(
            animator.casting_clip.as_deref(),
            Some("irons_spellbooks:long_cast_finish")
        );
    }

    #[test]
    fn instant_casts_pass_and_looping_casts_stop() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        on_cast_started(&mut animator, &library, None, None);
        on_cast_finished(&mut animator, &library, None, false);
        assert_eq!(animator.casting_clip.as_deref(), Some(INSTANT_PROJECTILE));

        let binding = SpellAnimationBinding {
            start: Some("irons_spellbooks:continuous_thrust".into()),
            finish: None,
        };
        on_cast_started(&mut animator, &library, Some(&binding), None);
        on_cast_finished(&mut animator, &library, Some(&binding), false);
        assert_eq!(animator.casting_clip, None);
    }

    #[test]
    fn cancellation_overrides_pass() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        let pass = SpellAnimationBinding {
            start: None,
            finish: Some(SpellFinish::Pass),
        };
        on_cast_started(&mut animator, &library, Some(&pass), None);
        on_cast_finished(&mut animator, &library, Some(&pass), true);
        assert_eq!(animator.casting_clip, None);
        assert!(animator.stack.layer(PRIORITY).unwrap().current().is_none());
    }

    #[test]
    fn offhand_sync_mirrors_the_next_clip() {
        let library = library();
        let mut animator = PlayerAnimator::default();
        on_synced_player_data(&mut animator, "offhand");
        on_cast_started(&mut animator, &library, None, None);
        let active = animator.stack.layer(PRIORITY).unwrap().current().unwrap();
        assert!(active.mirror);
        assert!(active.first_person.show_left_item);
    }
}
