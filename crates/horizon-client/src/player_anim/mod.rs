//! Player animations: a port of PlayerAnimator, the library Iron's Spells and
//! Better Combat animate the player model with.
//!
//! - [`format`]: clips, and the Emotecraft-style and Gecko-style JSON loaders;
//! - [`library`]: the clips of every mod, by `namespace:name`;
//! - [`sample`]: tick-exact sampling and playback with loops and speed schedules;
//! - [`layer`]: the pose, priority layers, fades, mirroring, channel masks;
//! - [`payload`]: the custom payloads that start animations;
//! - [`adapters`]: what Iron's Spells and Better Combat events do to the layers;
//! - [`state`]: per-player state, event routing, and the renderer's entry points
//!   ([`apply_to_poses`], [`root_transform`]).
//!
//! Playback advances once per client tick, counted from the model's `age`;
//! drawing only reads, so every consumer sees the same pose at a partial tick.

pub mod adapters;
pub mod ease;
pub mod format;
pub mod layer;
pub mod library;
pub mod payload;
pub mod sample;
pub mod state;

pub use payload::{AnimationNetEvent, decode, is_animation_channel};
pub use state::{
    LOCAL_KEY, LocalPlayer, apply_to_poses, better_combat_config, dispatch, library, load,
    local_first_person, player_spawned, reset, retain_better_combat_config, root_transform,
    set_local_left_handed, set_spell_binding,
};
