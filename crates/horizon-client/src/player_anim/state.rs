//! Per-player animation state, shared by the network and the renderer.
//!
//! Network events (decoded in `payload`) reach [`dispatch`], which resolves
//! the player and hands the event to the mod adapters. The renderer asks
//! [`apply_to_poses`] and [`root_transform`] for the same evaluated stack: they
//! advance it by the client ticks the mob's `age` shows since the last look and
//! never otherwise, so any number of draws in one frame agree.
//!
//! The local player is key [`LOCAL_KEY`] (its model is `Mob` id 0); other
//! players are keyed by entity id.

use super::adapters::irons_spellbooks::{self, SpellAnimationBinding};
use super::adapters::better_combat;
use super::format::{
    AXIS_PITCH, AXIS_ROLL, AXIS_X, AXIS_Y, AXIS_YAW, AXIS_Z, Bone, default_pose,
};
use super::layer::{AimContext, AnimationStack, FirstPersonConfig, PlayerPose};
use super::library::AnimationLibrary;
use super::payload::{AnimationNetEvent, BetterCombatConfig};
use crate::mobs::{Mob, anim::{Poses, State}};
use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError},
};

/// The registry key of the local player (its model is `Mob` id 0).
pub const LOCAL_KEY: i32 = 0;

const MAX_CATCH_UP_TICKS: u32 = 40;
const MAX_ANIMATORS: usize = 512;
const MAX_PENDING_PLAYERS: usize = 64;
const MAX_PENDING_EVENTS: usize = 8;
const PLAYER_TYPE: &str = "minecraft:player";
/// Height of the whole-player rotation pivot, in blocks.
const ROOT_PIVOT_Y: f32 = 0.7;

/// One player's layers and the facts the adapters keep between events.
#[derive(Debug, Default)]
pub struct PlayerAnimator {
    pub stack: AnimationStack,
    /// Casting clips mirror (offhand cast XOR left-handed).
    pub mirror_casting: bool,
    pub left_handed: bool,
    /// The id of the clip the casting layer started last.
    pub casting_clip: Option<String>,
    last_age: Option<u32>,
}

impl PlayerAnimator {
    /// Advances the stack by the game ticks between the last `age` seen and
    /// this one. A smaller age means a new entity took the id: start over.
    pub fn sync_age(&mut self, age: u32) {
        match self.last_age {
            Some(previous) if age >= previous => {
                for _ in 0..(age - previous).min(MAX_CATCH_UP_TICKS) {
                    self.stack.tick();
                }
            }
            Some(_) => {
                self.stack = AnimationStack::default();
                self.casting_clip = None;
            }
            None => {}
        }
        self.last_age = Some(age);
    }

    /// Applies the stack to `pose`; false (and `pose` untouched) while idle.
    pub fn overlay(&mut self, age: u32, alpha: f32, pitch: f32, pose: &mut PlayerPose) -> bool {
        self.sync_age(age);
        if self.stack.is_idle() {
            return false;
        }
        self.stack.evaluate(alpha, AimContext { pitch }, pose);
        true
    }
}

#[derive(Default)]
struct Registry {
    animators: HashMap<i32, PlayerAnimator>,
    /// Events for players whose spawn has not arrived, by profile UUID.
    pending: HashMap<u128, Vec<AnimationNetEvent>>,
    bindings: HashMap<String, SpellAnimationBinding>,
    cast_time_hint: Option<i32>,
    better_combat: BetterCombatConfig,
}

enum Target {
    Local,
    Uuid(u128),
    Entity(i32),
}

fn target_of(event: &AnimationNetEvent) -> Target {
    match event {
        AnimationNetEvent::IronsUpdateCastingState { .. } => Target::Local,
        AnimationNetEvent::IronsCastStarted { caster, .. }
        | AnimationNetEvent::IronsCastFinished { caster, .. } => Target::Uuid(*caster),
        AnimationNetEvent::IronsPlayAnimation { player, .. } => Target::Uuid(*player),
        AnimationNetEvent::IronsSyncPlayerData { entity_id, .. } => Target::Entity(*entity_id),
        AnimationNetEvent::BetterCombatAttack(attack) => Target::Entity(attack.player_id),
    }
}

/// Who the local player is, to tell its packets from the others'.
#[derive(Clone, Copy, Debug)]
pub struct LocalPlayer {
    pub entity_id: i32,
    pub uuid: u128,
}

impl LocalPlayer {
    fn key_of(&self, entity_id: i32) -> i32 {
        if entity_id == self.entity_id {
            LOCAL_KEY
        } else {
            entity_id
        }
    }
}

impl Registry {
    fn animator(&mut self, key: i32) -> &mut PlayerAnimator {
        if self.animators.len() >= MAX_ANIMATORS && !self.animators.contains_key(&key) {
            self.animators.retain(|_, a| !a.stack.is_idle());
        }
        self.animators.entry(key).or_default()
    }

    fn park(&mut self, uuid: u128, event: AnimationNetEvent) {
        if !self.pending.contains_key(&uuid) && self.pending.len() >= MAX_PENDING_PLAYERS {
            return;
        }
        let queue = self.pending.entry(uuid).or_default();
        if queue.len() < MAX_PENDING_EVENTS {
            queue.push(event);
        }
    }

    fn dispatch(
        &mut self,
        event: AnimationNetEvent,
        local: LocalPlayer,
        lookup: &dyn Fn(u128) -> Option<i32>,
        library: &AnimationLibrary,
    ) {
        match target_of(&event) {
            Target::Local => self.apply_local_state(&event),
            Target::Entity(entity_id) => self.apply(local.key_of(entity_id), event, library),
            Target::Uuid(uuid) => {
                let key = if uuid == local.uuid {
                    Some(LOCAL_KEY)
                } else {
                    lookup(uuid).map(|entity_id| local.key_of(entity_id))
                };
                match key {
                    Some(key) => self.apply(key, event, library),
                    None => self.park(uuid, event),
                }
            }
        }
    }

    /// `update_casting_state` reaches only the caster: its cast time tells the
    /// next start packet which clip to use, its slot which way to mirror.
    fn apply_local_state(&mut self, event: &AnimationNetEvent) {
        if let AnimationNetEvent::IronsUpdateCastingState {
            cast_time,
            equipment_slot,
            ..
        } = event
        {
            self.cast_time_hint = Some(*cast_time);
            irons_spellbooks::on_synced_player_data(self.animator(LOCAL_KEY), equipment_slot);
        }
    }

    fn apply(&mut self, key: i32, event: AnimationNetEvent, library: &AnimationLibrary) {
        match event {
            // Reaches only the local player, handled before the key is known.
            AnimationNetEvent::IronsUpdateCastingState { .. } => {}
            AnimationNetEvent::IronsCastStarted { spell_id, .. } => {
                let hint = if key == LOCAL_KEY {
                    self.cast_time_hint.take()
                } else {
                    None
                };
                let binding = self.bindings.get(&spell_id).cloned();
                irons_spellbooks::on_cast_started(
                    self.animator(key),
                    library,
                    binding.as_ref(),
                    hint,
                );
            }
            AnimationNetEvent::IronsCastFinished {
                spell_id,
                cancelled,
                ..
            } => {
                let binding = self.bindings.get(&spell_id).cloned();
                irons_spellbooks::on_cast_finished(
                    self.animator(key),
                    library,
                    binding.as_ref(),
                    cancelled,
                );
            }
            AnimationNetEvent::IronsPlayAnimation { animation, .. } => {
                if !irons_spellbooks::start(self.animator(key), library, &animation) {
                    log::debug!("Horizon: player animation {animation} is not loaded");
                }
            }
            AnimationNetEvent::IronsSyncPlayerData {
                casting_equipment_slot,
                ..
            } => {
                irons_spellbooks::on_synced_player_data(self.animator(key), &casting_equipment_slot);
            }
            AnimationNetEvent::BetterCombatAttack(attack) => {
                better_combat::on_attack_packet(self.animator(key), library, &attack);
            }
        }
    }

    fn spawned(
        &mut self,
        entity_id: i32,
        uuid: u128,
        local_entity_id: i32,
        library: &AnimationLibrary,
    ) {
        let key = if entity_id == local_entity_id {
            LOCAL_KEY
        } else {
            entity_id
        };
        if key != LOCAL_KEY {
            // A new entity: nothing of an earlier holder of the id stays.
            self.animators.remove(&key);
        }
        for event in self.pending.remove(&uuid).unwrap_or_default() {
            self.apply(key, event, library);
        }
    }

    fn local_first_person_pose(
        &mut self,
        age: u32,
        alpha: f32,
        pitch_rad: f32,
    ) -> Option<(FirstPersonConfig, PlayerPose)> {
        let animator = self.animators.get_mut(&LOCAL_KEY)?;
        let mut pose = PlayerPose::neutral();
        if !animator.overlay(age, alpha, pitch_rad, &mut pose) {
            return None;
        }
        let config = animator.stack.first_person()?;
        Some((config, pose))
    }
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
static LIBRARY: OnceLock<AnimationLibrary> = OnceLock::new();

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The loaded clips; `None` before [`load`].
pub fn library() -> Option<&'static AnimationLibrary> {
    LIBRARY.get()
}

/// Reads every mod's player animations under the content root `root`; returns
/// how many clips there are. Runs once; later calls change nothing.
pub fn load(root: &Path) -> usize {
    let library = LIBRARY.get_or_init(|| {
        let (library, report) = AnimationLibrary::load_resources(root);
        log::info!(
            "Horizon: player animations: {} clips from {} files ({} replaced duplicates)",
            library.len(),
            report.files,
            report.replaced.len()
        );
        for error in report.errors.iter().take(5) {
            log::warn!("Horizon: player animation file skipped: {error}");
        }
        library
    });
    library.len()
}

/// Drops every player's animation state: a new connection or world.
pub fn reset() {
    let mut registry = registry();
    registry.animators.clear();
    registry.pending.clear();
    registry.cast_time_hint = None;
}

/// Routes a decoded payload to its player. `lookup` maps a profile UUID to the
/// entity id of that player in the level, when it is there.
pub fn dispatch(
    event: AnimationNetEvent,
    local: LocalPlayer,
    lookup: &dyn Fn(u128) -> Option<i32>,
) {
    let Some(library) = library() else {
        return;
    };
    registry().dispatch(event, local, lookup, library);
}

/// A player entity appeared: its parked events play now.
pub fn player_spawned(entity_id: i32, uuid: u128, local_entity_id: i32) {
    let Some(library) = library() else {
        return;
    };
    registry().spawned(entity_id, uuid, local_entity_id, library);
}

/// Keeps a Better Combat configuration payload for local selection and timing.
pub fn retain_better_combat_config(channel: &str, data: &[u8]) {
    if let Err(e) = registry().better_combat.absorb(channel, data) {
        log::warn!("Horizon: Better Combat configuration payload: {e}");
    }
}

/// The retained Better Combat configuration.
pub fn better_combat_config() -> BetterCombatConfig {
    registry().better_combat.clone()
}

/// Overrides the clips a spell uses (the default follows its cast time).
pub fn set_spell_binding(spell_id: &str, binding: SpellAnimationBinding) {
    registry().bindings.insert(spell_id.to_owned(), binding);
}

/// Whether the local player holds the main arm left (mirrors its animations).
pub fn set_local_left_handed(left_handed: bool) {
    registry().animator(LOCAL_KEY).left_handed = left_handed;
}

/// What first person shows while the local player's animation runs.
pub fn local_first_person() -> Option<FirstPersonConfig> {
    registry()
        .animators
        .get(&LOCAL_KEY)
        .and_then(|a| a.stack.first_person())
}

/// Evaluates the local player's active animation pose for first-person rendering.
/// Returns `None` while idle.
pub fn local_first_person_pose(
    age: u32,
    alpha: f32,
    pitch_rad: f32,
) -> Option<(FirstPersonConfig, PlayerPose)> {
    registry().local_first_person_pose(age, alpha, pitch_rad)
}

/// Converts the animated arm (and optionally held item) pose delta from
/// entity model-part space (`-X` right, `+Y` down, `-Z` forward, pixels) into
/// first-person view space (`+X` right, `+Y` up, `-Z` forward, blocks).
pub fn first_person_hand_delta(pose: &PlayerPose, left: bool, include_item: bool) -> glam::Mat4 {
    let arm_bone = if left { Bone::LeftArm } else { Bone::RightArm };
    let item_bone = if left { Bone::LeftItem } else { Bone::RightItem };
    let arm = pose.bone(arm_bone);
    let def = default_pose(arm_bone);
    let mut dx = -(arm[AXIS_X] - def[AXIS_X]) / 16.0;
    let mut dy = -(arm[AXIS_Y] - def[AXIS_Y]) / 16.0;
    let mut dz = (arm[AXIS_Z] - def[AXIS_Z]) / 16.0;
    let mut rx = -arm[AXIS_PITCH];
    let mut ry = -arm[AXIS_YAW];
    let mut rz = arm[AXIS_ROLL];
    if include_item {
        let item = pose.bone(item_bone);
        dx += -item[AXIS_X] / 16.0;
        dy += -item[AXIS_Y] / 16.0;
        dz += item[AXIS_Z] / 16.0;
        rx += -item[AXIS_PITCH];
        ry += -item[AXIS_YAW];
        rz += item[AXIS_ROLL];
    }
    if ![dx, dy, dz, rx, ry, rz].into_iter().all(f32::is_finite) {
        return glam::Mat4::IDENTITY;
    }
    if dx == 0.0 && dy == 0.0 && dz == 0.0 && rx == 0.0 && ry == 0.0 && rz == 0.0 {
        return glam::Mat4::IDENTITY;
    }
    glam::Mat4::from_translation(glam::Vec3::new(dx, dy, dz))
        * glam::Mat4::from_quat(glam::Quat::from_euler(glam::EulerRot::ZYX, rz, ry, rx))
}

fn read_parts(poses: &mut Poses) -> PlayerPose {
    let mut pose = PlayerPose::neutral();
    for bone in Bone::ALL {
        if let Some(name) = bone.part_name()
            && poses.has(name)
        {
            pose.bones[bone.index()] = *poses.get(name);
        }
    }
    pose
}

fn write_parts(pose: &PlayerPose, poses: &mut Poses) {
    for bone in Bone::ALL {
        if let Some(name) = bone.part_name()
            && poses.has(name)
        {
            *poses.get(name) = pose.bones[bone.index()];
        }
    }
}

fn is_player(m: &Mob) -> bool {
    m.kind.id == PLAYER_TYPE
}

/// Lays the player's animation over the vanilla pose the model was just set
/// up with (called at the end of `anim::setup`, once per model drawn).
pub fn apply_to_poses(m: &Mob, s: &State, poses: &mut Poses) {
    if !is_player(m) {
        return;
    }
    let mut registry = registry();
    let Some(animator) = registry.animators.get_mut(&(m.id as i32)) else {
        return;
    };
    let mut pose = read_parts(poses);
    if animator.overlay(m.age, s.partial, s.x_rot.to_radians(), &mut pose) {
        write_parts(&pose, poses);
    }
}

/// The whole-player transform (`body` channel), to multiply into the model's
/// base matrix before the model flip. Identity when nothing animates it.
pub fn root_transform(m: &Mob, alpha: f32) -> glam::Mat4 {
    if !is_player(m) {
        return glam::Mat4::IDENTITY;
    }
    let mut registry = registry();
    let Some(animator) = registry.animators.get_mut(&(m.id as i32)) else {
        return glam::Mat4::IDENTITY;
    };
    let mut pose = PlayerPose::neutral();
    if !animator.overlay(m.age, alpha, 0.0, &mut pose) {
        return glam::Mat4::IDENTITY;
    }
    root_matrix(pose.bone(Bone::Body))
}

/// The matrix of a whole-player channel set: translation in blocks, then the
/// rotation about the pivot at 0.7 blocks.
pub fn root_matrix(body: &[f32; 9]) -> glam::Mat4 {
    let pivot = glam::Vec3::new(0.0, ROOT_PIVOT_Y, 0.0);
    glam::Mat4::from_translation(glam::Vec3::new(body[0], body[1], body[2]))
        * glam::Mat4::from_translation(pivot)
        * glam::Mat4::from_quat(glam::Quat::from_euler(
            glam::EulerRot::ZYX,
            body[5],
            body[4],
            body[3],
        ))
        * glam::Mat4::from_translation(-pivot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player_anim::format::{AXIS_YAW, parse_emotecraft, parse_gecko};
    use crate::player_anim::layer::FirstPersonMode;
    use crate::player_anim::payload::{AnimatedHand, BetterCombatAttack};

    const LOCAL: LocalPlayer = LocalPlayer {
        entity_id: 10,
        uuid: 0xAA,
    };
    const OTHER_UUID: u128 = 0xBB;

    fn library() -> AnimationLibrary {
        let mut library = AnimationLibrary::default();
        let gecko = r#"{"animations":{
            "long_cast":{"loop":true,"animation_length":1,"bones":{"head":{"rotation":[0,90,0]}}},
            "instant_projectile":{"animation_length":1,"bones":{"head":{"rotation":[0,45,0]}}}}}"#;
        for clip in parse_gecko(gecko).unwrap() {
            library.insert("irons_spellbooks", clip);
        }
        let slash = r#"{"name":"slash","emote":{"endTick":20,"moves":[{"tick":0,"head":{"yaw":1}}]}}"#;
        library.insert("bettercombat", parse_emotecraft(slash).unwrap());
        library
    }

    fn no_players(_: u128) -> Option<i32> {
        None
    }

    fn attack(player_id: i32) -> AnimationNetEvent {
        AnimationNetEvent::BetterCombatAttack(BetterCombatAttack {
            player_id,
            hand: AnimatedHand::MainHand,
            animation: "bettercombat:slash".into(),
            length: 10.0,
            upswing: 0.5,
            weapon_range: 3.0,
            upswing_ticks: 4,
            particles_json: String::new(),
        })
    }

    fn started(caster: u128) -> AnimationNetEvent {
        AnimationNetEvent::IronsCastStarted {
            spell_id: "irons_spellbooks:fireball".into(),
            spell_level: 1,
            caster,
        }
    }

    #[test]
    fn the_local_player_is_told_by_entity_id_and_uuid() {
        let library = library();
        let mut registry = Registry::default();
        registry.dispatch(attack(LOCAL.entity_id), LOCAL, &no_players, &library);
        assert!(registry.animators.contains_key(&LOCAL_KEY));
        registry.dispatch(attack(77), LOCAL, &no_players, &library);
        assert!(registry.animators.contains_key(&77));
        registry.dispatch(started(LOCAL.uuid), LOCAL, &no_players, &library);
        assert!(
            registry.animators[&LOCAL_KEY]
                .casting_clip
                .as_deref()
                .is_some()
        );
    }

    #[test]
    fn update_casting_state_picks_the_long_cast_for_the_next_start() {
        let library = library();
        let mut registry = Registry::default();
        let update = AnimationNetEvent::IronsUpdateCastingState {
            spell_id: "irons_spellbooks:fireball".into(),
            spell_level: 1,
            cast_time: 30,
            cast_source: 0,
            equipment_slot: "mainhand".into(),
        };
        registry.dispatch(update, LOCAL, &no_players, &library);
        registry.dispatch(started(LOCAL.uuid), LOCAL, &no_players, &library);
        assert_eq!(
            registry.animators[&LOCAL_KEY].casting_clip.as_deref(),
            Some("irons_spellbooks:long_cast")
        );
        // The hint was used up: the next start is an instant cast.
        registry.dispatch(started(LOCAL.uuid), LOCAL, &no_players, &library);
        assert_eq!(
            registry.animators[&LOCAL_KEY].casting_clip.as_deref(),
            Some("irons_spellbooks:instant_projectile")
        );
    }

    #[test]
    fn events_for_unspawned_players_wait_for_the_spawn() {
        let library = library();
        let mut registry = Registry::default();
        registry.dispatch(started(OTHER_UUID), LOCAL, &no_players, &library);
        assert!(registry.animators.is_empty());
        assert_eq!(registry.pending[&OTHER_UUID].len(), 1);
        registry.spawned(55, OTHER_UUID, LOCAL.entity_id, &library);
        assert!(registry.pending.is_empty());
        assert!(registry.animators[&55].casting_clip.is_some());
    }

    #[test]
    fn known_players_resolve_through_the_lookup() {
        let library = library();
        let mut registry = Registry::default();
        let lookup = |uuid: u128| (uuid == OTHER_UUID).then_some(55);
        registry.dispatch(started(OTHER_UUID), LOCAL, &lookup, &library);
        assert!(registry.animators[&55].casting_clip.is_some());
    }

    #[test]
    fn pending_events_are_bounded() {
        let library = library();
        let mut registry = Registry::default();
        for _ in 0..(MAX_PENDING_EVENTS + 5) {
            registry.dispatch(started(OTHER_UUID), LOCAL, &no_players, &library);
        }
        assert_eq!(registry.pending[&OTHER_UUID].len(), MAX_PENDING_EVENTS);
        for uuid in 0..(MAX_PENDING_PLAYERS as u128 + 10) {
            registry.dispatch(started(1000 + uuid), LOCAL, &no_players, &library);
        }
        assert!(registry.pending.len() <= MAX_PENDING_PLAYERS);
    }

    #[test]
    fn a_respawned_entity_id_forgets_the_old_animation() {
        let library = library();
        let mut registry = Registry::default();
        registry.dispatch(attack(55), LOCAL, &no_players, &library);
        assert!(!registry.animators[&55].stack.is_idle());
        registry.spawned(55, OTHER_UUID, LOCAL.entity_id, &library);
        assert!(!registry.animators.contains_key(&55));
    }

    #[test]
    fn age_drives_the_ticks_and_a_lower_age_resets() {
        let library = library();
        let mut registry = Registry::default();
        registry.dispatch(attack(55), LOCAL, &no_players, &library);
        let animator = registry.animators.get_mut(&55).unwrap();
        let mut pose = PlayerPose::neutral();
        // The first look only records the age.
        assert!(animator.overlay(100, 0.0, 0.0, &mut pose));
        let first = animator.stack.layer(better_combat::ATTACK_PRIORITY).unwrap();
        assert_eq!(first.current().unwrap().playback.tick_index(), 0);
        // The same age again (another draw in the frame) changes nothing.
        animator.overlay(100, 0.5, 0.0, &mut pose);
        animator.overlay(103, 0.0, 0.0, &mut pose);
        let later = animator.stack.layer(better_combat::ATTACK_PRIORITY).unwrap();
        // Three ticks at 20 clip ticks over 10 ticks.
        assert_eq!(later.current().unwrap().playback.tick_index(), 6);
        // A new mob starts at age 0.
        assert!(!animator.overlay(2, 0.0, 0.0, &mut pose));
    }

    #[test]
    fn overlay_replaces_enabled_channels_only() {
        let library = library();
        let mut registry = Registry::default();
        registry.dispatch(attack(55), LOCAL, &no_players, &library);
        let animator = registry.animators.get_mut(&55).unwrap();
        let mut pose = PlayerPose::neutral();
        pose.bones[Bone::Head.index()][3] = 0.25;
        animator.overlay(0, 0.0, 0.0, &mut pose);
        // Yaw is animated; the pitch of the pose below passes through (the
        // attack masks head pitch).
        assert!(pose.bone(Bone::Head)[AXIS_YAW] != 0.0);
        assert_eq!(pose.bone(Bone::Head)[3], 0.25);
    }

    #[test]
    fn root_matrix_rotates_about_the_pivot() {
        let identity = root_matrix(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(identity.abs_diff_eq(glam::Mat4::IDENTITY, 1e-6));
        // A quarter turn about z leaves the pivot point where it was.
        let turned = root_matrix(&[0.0, 0.0, 0.0, 0.0, 0.0, std::f32::consts::FRAC_PI_2, 1.0, 1.0, 1.0]);
        let pivot = glam::Vec3::new(0.0, ROOT_PIVOT_Y, 0.0);
        assert!(turned.transform_point3(pivot).abs_diff_eq(pivot, 1e-5));
        let moved = root_matrix(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(moved
            .transform_point3(glam::Vec3::ZERO)
            .abs_diff_eq(glam::Vec3::X, 1e-6));
    }

    #[test]
    fn first_person_hand_delta_is_identity_for_neutral_pose() {
        let neutral = PlayerPose::neutral();
        for left in [false, true] {
            for include_item in [false, true] {
                assert!(
                    first_person_hand_delta(&neutral, left, include_item)
                        .abs_diff_eq(glam::Mat4::IDENTITY, 1e-6)
                );
            }
        }
    }

    #[test]
    fn first_person_hand_delta_transforms_animated_arm_and_guards_nan() {
        let mut pose = PlayerPose::neutral();
        pose.bones[Bone::RightArm.index()][AXIS_PITCH] = 0.5;
        pose.bones[Bone::RightArm.index()][AXIS_X] -= 4.0;
        let m = first_person_hand_delta(&pose, false, true);
        assert!(m.is_finite());
        assert!(!m.abs_diff_eq(glam::Mat4::IDENTITY, 1e-4));

        pose.bones[Bone::RightArm.index()][AXIS_Y] = f32::NAN;
        assert!(
            first_person_hand_delta(&pose, false, false).abs_diff_eq(glam::Mat4::IDENTITY, 1e-6)
        );
    }

    #[test]
    fn local_first_person_pose_returns_none_when_idle_and_some_when_active() {
        let library = library();
        let mut registry = Registry::default();
        assert!(registry.local_first_person_pose(0, 0.0, 0.0).is_none());

        registry.dispatch(attack(LOCAL.entity_id), LOCAL, &no_players, &library);
        let (cfg, _pose) = registry
            .local_first_person_pose(0, 0.25, 0.1)
            .expect("active local attack should yield first-person config and pose");
        assert_eq!(cfg.mode, FirstPersonMode::ThirdPersonModel);

        assert!(registry.local_first_person_pose(30, 0.0, 0.0).is_none());
    }
}
