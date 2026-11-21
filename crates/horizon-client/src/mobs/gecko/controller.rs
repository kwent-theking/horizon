//! A small stand-in for GeckoLib's animation controllers: it reads the mob's
//! state into Molang queries, picks a clip by role (attack, death, fly, run,
//! walk, idle), and blends the old clip's pose into the new one over a few
//! ticks. Each mob keeps its own controller, keyed by entity id.
use super::anim::{AnimationFile, BoneSample, Clip, LoopMode, pick_clip};
use super::molang::MolangContext;
use crate::mobs::{Mob, anim::State, living::wrap};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

/// GeckoLib's usual transition length: 5 ticks (0.25 s).
const BLEND_TICKS: f32 = 5.0;
const TICKS_PER_SECOND: f32 = 20.0;
/// `State::speed` (limb speed) above this counts as walking.
const MOVING_SPEED: f32 = 0.05;
/// Limb speed above this plays the run clip when there is one.
const RUN_SPEED: f32 = 0.7;
const DEFAULT_DELTA_SECONDS: f32 = 1.0 / TICKS_PER_SECOND;
const PRUNE_AT: usize = 1024;
const STALE_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Attack,
    Death,
    Fly,
    Run,
    Walk,
    Idle,
}

impl Role {
    /// Name tokens (split at `.`, `_`, `-`) that mark a clip as this role.
    fn words(self) -> &'static [&'static str] {
        match self {
            Role::Attack => &["attack", "melee"],
            Role::Death => &["death", "die", "dead"],
            Role::Fly => &["fly", "flying", "glide", "gliding"],
            Role::Run => &["run", "running", "sprint"],
            Role::Walk => &["walk", "walking", "move", "moving"],
            Role::Idle => &["idle"],
        }
    }

    /// Locomotion clips are looped by GeckoLib's controllers whatever their
    /// json `loop` says (it is usually absent).
    fn is_locomotion(self) -> bool {
        matches!(self, Role::Fly | Role::Run | Role::Walk | Role::Idle)
    }
}

/// What the mob is doing, from which the clip is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intent {
    pub dead: bool,
    pub attacking: bool,
    pub airborne: bool,
    pub moving: bool,
    pub fast: bool,
}

/// One frame's readings of a mob, the source of every Molang query.
#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub age_ticks: f32,
    /// Limb speed (`State::speed`), 0 to 1.
    pub speed: f32,
    /// Limb position (`State::pos`).
    pub limb_swing: f32,
    pub head_x_rot: f32,
    pub head_y_rot: f32,
    pub body_y_rot: f32,
    /// Body turn in degrees per second.
    pub yaw_speed: f32,
    pub on_ground: bool,
    pub in_water: bool,
    pub baby: bool,
    /// Attack swing progress, 0 when not swinging.
    pub swing: f32,
    pub health: f32,
    pub max_health: f32,
    /// Blocks per second.
    pub vertical_speed: f32,
    pub hurt_time: f32,
    pub dead: bool,
    pub riding: bool,
}

impl Snapshot {
    pub fn from_mob(m: &Mob, s: &State) -> Self {
        let body = m.previous_body_yaw + wrap(m.body_yaw - m.previous_body_yaw) * s.partial;
        Self {
            age_ticks: s.age,
            speed: s.speed,
            limb_swing: s.pos,
            head_x_rot: s.x_rot,
            head_y_rot: s.y_rot,
            body_y_rot: wrap(body),
            yaw_speed: wrap(m.body_yaw - m.previous_body_yaw) * TICKS_PER_SECOND,
            on_ground: m.on_ground,
            in_water: m.fluids.in_water,
            baby: s.baby,
            swing: s.swing,
            health: m.health,
            max_health: m.attrs.max_health,
            vertical_speed: m.velocity.y as f32 * TICKS_PER_SECOND,
            hurt_time: m.hurt_time.max(0) as f32,
            dead: !m.alive(),
            riding: m.extra.passenger,
        }
    }

    pub fn intent(&self) -> Intent {
        Intent {
            dead: self.dead,
            attacking: self.swing > 0.0,
            airborne: !self.on_ground && !self.in_water,
            moving: self.speed > MOVING_SPEED,
            fast: self.speed > RUN_SPEED,
        }
    }
}

/// The Molang queries a mob answers. `query.distance_from_camera` stays 0
/// (the animation state does not carry the camera); every other query that is
/// not listed here also reads 0, logged once at debug level.
pub struct MobQueries<'a> {
    pub snapshot: &'a Snapshot,
    pub delta_seconds: f32,
}

fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

impl MolangContext for MobQueries<'_> {
    fn query(&self, name: &str) -> f32 {
        let s = self.snapshot;
        match name {
            "query.life_time" => s.age_ticks / TICKS_PER_SECOND,
            "query.ground_speed" | "query.modified_move_speed" => s.speed,
            "query.is_moving" => flag(s.speed > MOVING_SPEED),
            "query.is_on_ground" => flag(s.on_ground),
            "query.is_in_water" => flag(s.in_water),
            "query.is_baby" => flag(s.baby),
            "query.is_riding" => flag(s.riding),
            "query.is_alive" => flag(!s.dead),
            "query.head_x_rotation" => s.head_x_rot,
            "query.head_y_rotation" => s.head_y_rot,
            "query.body_y_rotation" => s.body_y_rot,
            "query.yaw_speed" => s.yaw_speed,
            "query.vertical_speed" => s.vertical_speed,
            "query.health" => s.health,
            "query.max_health" => s.max_health,
            "query.hurt_time" => s.hurt_time,
            "query.is_swinging" | "query.is_attacking" => flag(s.swing > 0.0),
            "query.attack_time" => s.swing,
            "query.delta_time" => self.delta_seconds,
            "query.limb_swing" => s.limb_swing,
            "query.limb_swing_amount" => s.speed,
            "query.distance_from_camera" => 0.0,
            _ => {
                if name.starts_with("query.") && first_sighting(name) {
                    log::debug!("Horizon: GeckoLib: unknown molang query {name} reads 0");
                }
                0.0
            }
        }
    }
}

/// True the first time `name` is seen, so a missing query logs once.
fn first_sighting(name: &str) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    !seen.contains(name) && seen.insert(name.to_owned())
}

fn name_has_word(name: &str, words: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    let tail = lower.rsplit_once('.').map_or(lower.as_str(), |(_, t)| t);
    tail.split(['_', '-', '.'])
        .any(|token| words.contains(&token))
}

fn find_role_clip(file: &AnimationFile, role: Role) -> Option<&str> {
    file.clips
        .keys()
        .find(|name| name_has_word(name, role.words()))
        .map(String::as_str)
}

/// The clip for `intent` and its role: attack, then death, flying when
/// airborne, run when fast, walk when moving, else idle; a role without a
/// clip falls to the next, and the last resort is `pick_clip`.
pub fn select_clip<'a>(file: &'a AnimationFile, intent: &Intent) -> Option<(&'a str, Role)> {
    let mut chain = Vec::with_capacity(6);
    if intent.dead {
        chain.push(Role::Death);
    }
    if intent.attacking {
        chain.push(Role::Attack);
    }
    if intent.airborne {
        chain.push(Role::Fly);
    }
    if intent.moving {
        if intent.fast {
            chain.extend([Role::Run, Role::Walk]);
        } else {
            chain.extend([Role::Walk, Role::Run]);
        }
    }
    chain.push(Role::Idle);
    for role in chain {
        if let Some(name) = find_role_clip(file, role) {
            return Some((name, role));
        }
    }
    let role = if intent.moving { Role::Walk } else { Role::Idle };
    pick_clip(file, intent.moving).map(|name| (name, role))
}

#[derive(Clone, Debug)]
struct Playing {
    name: String,
    role: Role,
    started: f32,
}

/// Seconds into a clip that has played for `elapsed`: locomotion clips whose
/// json says play-once still loop, death holds its last frame.
fn clip_time(clip: &Clip, role: Role, elapsed: f32) -> f32 {
    let elapsed = elapsed.max(0.0);
    match role {
        Role::Death => elapsed.min(clip.length()),
        _ if role.is_locomotion() && clip.loop_mode == LoopMode::Once && clip.length() > 0.0 => {
            elapsed.rem_euclid(clip.length())
        }
        _ => elapsed,
    }
}

/// A one-shot clip that has run its length (a looping clip has no end).
fn has_played_out(clip: &Clip, elapsed_seconds: f32) -> bool {
    clip.loop_mode != LoopMode::Loop && elapsed_seconds >= clip.length()
}

#[derive(Clone, Debug, Default)]
pub struct Controller {
    current: Option<Playing>,
    previous: Option<Playing>,
    blend_start: f32,
    /// The attack clip ran out while the swing went on: no replay until a
    /// new swing.
    attack_spent: bool,
    last_age: f32,
    delta_seconds: f32,
    seen: Option<Instant>,
}

impl Controller {
    /// Moves the controller to `snapshot`'s moment: picks the clip, starts a
    /// transition when it changes.
    pub fn advance(&mut self, file: &AnimationFile, snapshot: &Snapshot) {
        let age = snapshot.age_ticks;
        if age < self.last_age - 1.0 {
            *self = Self::default();
        }
        if age > self.last_age {
            self.delta_seconds = (age - self.last_age).min(TICKS_PER_SECOND) / TICKS_PER_SECOND;
        }
        self.last_age = age;
        self.seen = Some(Instant::now());

        let mut intent = snapshot.intent();
        if !intent.attacking {
            self.attack_spent = false;
        }
        let attack_run = self
            .current
            .as_ref()
            .filter(|p| p.role == Role::Attack)
            .and_then(|p| Some((p, file.clips.get(&p.name)?)));
        if let Some((playing, clip)) = attack_run
            && has_played_out(clip, (age - playing.started) / TICKS_PER_SECOND)
            && intent.attacking
        {
            self.attack_spent = true;
        }
        if self.attack_spent {
            intent.attacking = false;
        }
        let keep_attack = !intent.dead
            && !intent.attacking
            && attack_run.is_some_and(|(playing, clip)| {
                !has_played_out(clip, (age - playing.started) / TICKS_PER_SECOND)
            });

        let wanted = if keep_attack {
            self.current.as_ref().map(|p| (p.name.clone(), p.role))
        } else {
            select_clip(file, &intent).map(|(name, role)| (name.to_owned(), role))
        };
        let Some((name, role)) = wanted else {
            return;
        };
        if self.current.as_ref().is_none_or(|p| p.name != name) {
            self.previous = self.current.take();
            self.blend_start = age;
            self.current = Some(Playing {
                name,
                role,
                started: age,
            });
        }
        if self.blend_weight(age) >= 1.0 {
            self.previous = None;
        }
    }

    /// How far the new clip has taken over from the previous one, 0 to 1.
    fn blend_weight(&self, age: f32) -> f32 {
        if self.previous.is_none() {
            return 1.0;
        }
        ((age - self.blend_start) / BLEND_TICKS).clamp(0.0, 1.0)
    }

    /// The bone samples for `snapshot`'s moment, after `advance`.
    pub fn pose(&self, file: &AnimationFile, snapshot: &Snapshot) -> Vec<(String, BoneSample)> {
        let queries = MobQueries {
            snapshot,
            delta_seconds: if self.delta_seconds > 0.0 {
                self.delta_seconds
            } else {
                DEFAULT_DELTA_SECONDS
            },
        };
        let age = snapshot.age_ticks;
        let sample = |playing: &Playing| {
            let clip = file.clips.get(&playing.name)?;
            let elapsed = (age - playing.started) / TICKS_PER_SECOND;
            Some(clip.sample(clip_time(clip, playing.role, elapsed), &queries))
        };
        let Some(current) = self.current.as_ref().and_then(sample) else {
            return Vec::new();
        };
        match self.previous.as_ref().and_then(sample) {
            Some(previous) => blend(&previous, &current, self.blend_weight(age)),
            None => current,
        }
    }
}

/// `from` mixed into `to`: weight 0 is `from`, 1 is `to`; a bone only one
/// clip moves blends against its rest pose.
pub fn blend(
    from: &[(String, BoneSample)],
    to: &[(String, BoneSample)],
    weight: f32,
) -> Vec<(String, BoneSample)> {
    let mut bones: BTreeMap<&str, (BoneSample, BoneSample)> = BTreeMap::new();
    for (name, sample) in from {
        bones.entry(name).or_default().0 = *sample;
    }
    for (name, sample) in to {
        bones.entry(name).or_default().1 = *sample;
    }
    let mix = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * weight);
    bones
        .into_iter()
        .map(|(name, (a, b))| {
            let sample = BoneSample {
                rotation_deg: mix(a.rotation_deg, b.rotation_deg),
                position: mix(a.position, b.position),
                scale: mix(a.scale, b.scale),
            };
            (name.to_owned(), sample)
        })
        .collect()
}

static CONTROLLERS: OnceLock<Mutex<HashMap<u32, Controller>>> = OnceLock::new();

/// Poses mob `id` from its clips for this frame, advancing its controller.
pub fn pose(id: u32, clips: &AnimationFile, snapshot: &Snapshot) -> Vec<(String, BoneSample)> {
    let controller = {
        let mut controllers = CONTROLLERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if controllers.len() > PRUNE_AT {
            controllers.retain(|_, c| c.seen.is_some_and(|t| t.elapsed() < STALE_AFTER));
        }
        let controller = controllers.entry(id).or_default();
        controller.advance(clips, snapshot);
        controller.clone()
    };
    controller.pose(clips, snapshot)
}

#[cfg(test)]
mod tests {
    use super::super::anim::parse_animations;
    use super::*;

    fn clips(json: &str) -> AnimationFile {
        parse_animations(json).expect("animation JSON should parse")
    }

    /// One clip per name, each holding bone `a` at rotation x = value.
    fn constant_clips(entries: &[(&str, f32, &str)]) -> AnimationFile {
        let body = entries
            .iter()
            .map(|(name, x, extra)| {
                format!(
                    r#""animation.m.{name}":{{{extra}"animation_length":1.0,"bones":{{"a":{{"rotation":[{x},0,0]}}}}}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        clips(&format!(r#"{{"animations":{{{body}}}}}"#))
    }

    fn all_roles() -> AnimationFile {
        constant_clips(&[
            ("attack", 1.0, ""),
            ("death", 2.0, ""),
            ("fly", 3.0, ""),
            ("idle", 4.0, ""),
            ("run", 5.0, ""),
            ("walk", 6.0, ""),
        ])
    }

    fn picked(file: &AnimationFile, intent: Intent) -> Option<String> {
        select_clip(file, &intent).map(|(name, _)| name.rsplit('.').next().unwrap().to_owned())
    }

    fn rotation_x(pose: &[(String, BoneSample)]) -> f32 {
        pose.iter().find(|(n, _)| n == "a").expect("bone a").1.rotation_deg[0]
    }

    fn at(age: f32) -> Snapshot {
        Snapshot {
            age_ticks: age,
            on_ground: true,
            ..Snapshot::default()
        }
    }

    fn query(snapshot: &Snapshot, name: &str) -> f32 {
        MobQueries {
            snapshot,
            delta_seconds: 0.05,
        }
        .query(name)
    }

    #[test]
    fn queries_report_the_snapshot() {
        let s = Snapshot {
            age_ticks: 40.0,
            speed: 0.5,
            limb_swing: 3.0,
            head_x_rot: 12.0,
            head_y_rot: -30.0,
            body_y_rot: 90.0,
            yaw_speed: 45.0,
            on_ground: false,
            in_water: true,
            baby: true,
            swing: 0.4,
            health: 7.0,
            max_health: 20.0,
            vertical_speed: -2.0,
            hurt_time: 3.0,
            dead: false,
            riding: true,
        };
        let expected = [
            ("query.life_time", 2.0),
            ("query.ground_speed", 0.5),
            ("query.modified_move_speed", 0.5),
            ("query.is_moving", 1.0),
            ("query.is_on_ground", 0.0),
            ("query.is_in_water", 1.0),
            ("query.is_baby", 1.0),
            ("query.is_riding", 1.0),
            ("query.is_alive", 1.0),
            ("query.head_x_rotation", 12.0),
            ("query.head_y_rotation", -30.0),
            ("query.body_y_rotation", 90.0),
            ("query.yaw_speed", 45.0),
            ("query.vertical_speed", -2.0),
            ("query.health", 7.0),
            ("query.max_health", 20.0),
            ("query.hurt_time", 3.0),
            ("query.is_swinging", 1.0),
            ("query.is_attacking", 1.0),
            ("query.attack_time", 0.4),
            ("query.delta_time", 0.05),
            ("query.limb_swing", 3.0),
            ("query.limb_swing_amount", 0.5),
            ("query.distance_from_camera", 0.0),
            ("query.nonexistent_query", 0.0),
        ];
        for (name, value) in expected {
            assert!((query(&s, name) - value).abs() < 1e-6, "{name}");
        }
    }

    #[test]
    fn still_mob_is_not_moving_or_swinging() {
        let s = at(0.0);
        assert_eq!(query(&s, "query.is_moving"), 0.0);
        assert_eq!(query(&s, "query.is_swinging"), 0.0);
        assert_eq!(query(&s, "query.is_on_ground"), 1.0);
    }

    #[test]
    fn unknown_query_is_reported_once() {
        assert!(first_sighting("query.test_only_once"));
        assert!(!first_sighting("query.test_only_once"));
    }

    #[test]
    fn clip_selection_follows_the_role_chain() {
        let file = all_roles();
        let base = Intent::default();
        assert_eq!(picked(&file, base).as_deref(), Some("idle"));
        let walking = Intent { moving: true, ..base };
        assert_eq!(picked(&file, walking).as_deref(), Some("walk"));
        let running = Intent { fast: true, ..walking };
        assert_eq!(picked(&file, running).as_deref(), Some("run"));
        let flying = Intent { airborne: true, ..walking };
        assert_eq!(picked(&file, flying).as_deref(), Some("fly"));
        let attacking = Intent { attacking: true, ..running };
        assert_eq!(picked(&file, attacking).as_deref(), Some("attack"));
        let dead = Intent { dead: true, ..attacking };
        assert_eq!(picked(&file, dead).as_deref(), Some("death"));
    }

    #[test]
    fn clip_selection_falls_back_when_roles_are_missing() {
        let walk_idle = constant_clips(&[("idle", 0.0, ""), ("walk", 1.0, "")]);
        let fast = Intent { moving: true, fast: true, ..Intent::default() };
        assert_eq!(picked(&walk_idle, fast).as_deref(), Some("walk"));
        let airborne = Intent { airborne: true, ..Intent::default() };
        assert_eq!(picked(&walk_idle, airborne).as_deref(), Some("idle"));
        let attacking = Intent { attacking: true, moving: true, ..Intent::default() };
        assert_eq!(picked(&walk_idle, attacking).as_deref(), Some("walk"));
        let run_only = constant_clips(&[("sprint_run", 0.0, "")]);
        let moving = Intent { moving: true, ..Intent::default() };
        assert_eq!(picked(&run_only, moving).as_deref(), Some("sprint_run"));
        assert_eq!(select_clip(&clips(r#"{"animations":{}}"#), &moving), None);
    }

    #[test]
    fn role_words_match_whole_name_tokens() {
        let file = constant_clips(&[("grunt", 0.0, ""), ("walk_loop", 1.0, "")]);
        let moving = Intent { moving: true, fast: true, ..Intent::default() };
        assert_eq!(picked(&file, moving).as_deref(), Some("walk_loop"));
    }

    #[test]
    fn blend_midpoint_is_halfway_between_clips() {
        let a = [("a".to_owned(), BoneSample::default())];
        let moved = BoneSample {
            rotation_deg: [10.0, 0.0, 0.0],
            scale: [3.0; 3],
            ..BoneSample::default()
        };
        let b = [("a".to_owned(), moved)];
        let mid = blend(&a, &b, 0.5);
        assert_eq!(mid[0].1.rotation_deg, [5.0, 0.0, 0.0]);
        assert_eq!(mid[0].1.scale, [2.0; 3]);
        assert_eq!(blend(&a, &b, 0.0)[0].1, BoneSample::default());
        assert_eq!(blend(&a, &b, 1.0)[0].1, moved);
        let only_new = blend(&[], &b, 0.5);
        assert_eq!(only_new[0].1.rotation_deg, [5.0, 0.0, 0.0]);
    }

    #[test]
    fn controller_blends_over_five_ticks_when_the_clip_changes() {
        let file = all_roles();
        let mut ctl = Controller::default();
        let still = at(10.0);
        ctl.advance(&file, &still);
        assert_eq!(rotation_x(&ctl.pose(&file, &still)), 4.0);

        let walking = |age: f32| Snapshot { speed: 0.3, ..at(age) };
        ctl.advance(&file, &walking(11.0));
        assert_eq!(rotation_x(&ctl.pose(&file, &walking(11.0))), 4.0);
        ctl.advance(&file, &walking(13.5));
        assert!((rotation_x(&ctl.pose(&file, &walking(13.5))) - 5.0).abs() < 1e-4);
        ctl.advance(&file, &walking(16.0));
        assert_eq!(rotation_x(&ctl.pose(&file, &walking(16.0))), 6.0);
        assert!(ctl.previous.is_none());
    }

    #[test]
    fn attack_plays_once_then_returns_to_locomotion() {
        let file = all_roles();
        let mut ctl = Controller::default();
        let swinging = |age: f32| Snapshot { swing: 0.5, ..at(age) };
        ctl.advance(&file, &swinging(0.0));
        assert_eq!(ctl.current.as_ref().unwrap().role, Role::Attack);
        // The swing ends early; the one-second clip keeps playing.
        ctl.advance(&file, &at(5.0));
        assert_eq!(ctl.current.as_ref().unwrap().role, Role::Attack);
        // Clip done: back to idle.
        ctl.advance(&file, &at(21.0));
        assert_eq!(ctl.current.as_ref().unwrap().role, Role::Idle);
        // A swing still reported after the clip ended does not replay it.
        let mut ctl = Controller::default();
        ctl.advance(&file, &swinging(0.0));
        ctl.advance(&file, &swinging(25.0));
        assert_eq!(ctl.current.as_ref().unwrap().role, Role::Idle);
        ctl.advance(&file, &at(26.0));
        ctl.advance(&file, &swinging(27.0));
        assert_eq!(ctl.current.as_ref().unwrap().role, Role::Attack);
    }

    #[test]
    fn hold_on_last_frame_holds_and_once_locomotion_loops() {
        let file = clips(
            r#"{"animations":{"animation.m.death":{"loop":"hold_on_last_frame","animation_length":1.0,"bones":{"a":{"rotation":{"0.0":[0,0,0],"1.0":[10,0,0]}}}},
            "animation.m.walk":{"loop":false,"animation_length":1.0,"bones":{"a":{"rotation":{"0.0":[0,0,0],"1.0":[10,0,0]}}}}}}"#,
        );
        let death = file.clips.get("animation.m.death").unwrap();
        assert_eq!(clip_time(death, Role::Death, 5.0), 1.0);
        let end = death.sample(clip_time(death, Role::Death, 5.0), &NoQueries);
        assert_eq!(rotation_x(&end), 10.0);
        let walk = file.clips.get("animation.m.walk").unwrap();
        assert!((clip_time(walk, Role::Walk, 1.5) - 0.5).abs() < 1e-6);
        assert_eq!(clip_time(walk, Role::Attack, 1.5), 1.5);
    }

    #[test]
    fn looping_death_clamps_instead_of_wrapping() {
        let file = constant_clips(&[("death", 0.0, r#""loop":true,"#)]);
        let death = file.clips.values().next().unwrap();
        assert_eq!(clip_time(death, Role::Death, 7.0), 1.0);
    }

    #[test]
    fn age_going_backwards_resets_the_controller() {
        let file = all_roles();
        let mut ctl = Controller::default();
        ctl.advance(&file, &at(500.0));
        ctl.advance(&file, &at(2.0));
        assert_eq!(ctl.current.as_ref().unwrap().started, 2.0);
        assert!(ctl.previous.is_none());
    }

    struct NoQueries;

    impl MolangContext for NoQueries {
        fn query(&self, _name: &str) -> f32 {
            0.0
        }
    }
}
