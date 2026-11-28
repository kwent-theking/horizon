//! Create Big Cannons 5.11.7 client effects that draw nothing (MIT,
//! Cannoneers of Create), with CBC's client config defaults:
//!
//! - the blast wave a firing cannon or an exploding shell sends as a
//!   particle (`BlastWaveEffectParticle`): its sound and screen shake reach
//!   the player as the wave front passes, at `blastEffectDelaySpeed`;
//! - the autocannon's `ClientboundBlastSoundPacket`, delayed by distance;
//! - the screen shake (`CBCScreenShakeHandler` over Ritchie's Projectile
//!   Library's `ModScreenShakeHandler.Impl`, MIT), added to the camera's yaw,
//!   pitch and roll as NeoForge's `ViewportEvent.ComputeCameraAngles`.
use super::Game;
mod hits;
use glam::DVec3;
use horizon_client::{
    audio::Audio,
    chunk::{HEIGHT, MIN_Y},
    net::{CBC_EXPLODE_PACKET, CBC_FLUID_EXPLODE_PACKET},
    particles::{CbcParticle, ParticleOptions, ParticleType},
    sound_engine::{InstanceId, NEUTRAL, SoundEngine, SoundInstance, Sounds},
};
use horizon_core::{
    geometry::Vec3,
    random::{LegacyRandom, RandomSource},
};
use horizon_protocol::{
    buf::Reader,
    entity_data::{BlastWave, CBC_CANNON_BLAST_WAVE, CBC_SHELL_BLAST_WAVE, ParticleData},
    packets::play,
};
use std::collections::HashMap;

/// `blastEffectDelaySpeed` (m/s); `isInstantaneousBlastEffect` below 0.01.
const BLAST_EFFECT_DELAY_SPEED: f32 = 320.0;
/// `blastSoundAirAbsorption`.
const BLAST_SOUND_AIR_ABSORPTION: bool = true;
/// `bigCannonShakePowerMultiplier`/`Limit` and `shellShakePowerMultiplier`/`Limit`.
const BIG_CANNON_SHAKE: (f32, f32) = (6.0, 45.0);
const SHELL_SHAKE: (f32, f32) = (6.0, 45.0);
/// `cannonScreenShakeIntensity`, `cannonScreenShakeSpringiness`, `cannonScreenShakeDecay`.
const SHAKE_INTENSITY: f32 = 1.3;
const SHAKE_SPRINGINESS: f32 = 0.08;
const SHAKE_DECAY: f32 = 0.3;
/// `CBCRootNetwork`'s id of `ClientboundBlastSoundPacket`.
const BLAST_SOUND_PACKET: i32 = 26;
/// Bound the packed block list in `ClientboundCBCExplodePacket`.
const MAX_EXPLOSION_BLOCKS: usize = 1 << 18;
/// The `SoundSource.BLOCKS` ordinal in `ShellBlastWaveEffectParticleData`.
const BLOCKS_SOURCE: i32 = 4;
/// `BlastWaveEffectParticle`'s timeout (`setLifetime(100)`).
const BLAST_WAVE_LIFETIME: i32 = 100;

fn instantaneous() -> bool {
    BLAST_EFFECT_DELAY_SPEED < 1e-2
}

/// A sound to start now: event, `SoundSource` ordinal, position, volume,
/// pitch and air absorption factor.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BlastSound {
    pub event: String,
    pub source: usize,
    pub at: [f64; 3],
    pub volume: f32,
    pub pitch: f32,
    pub air_absorption: f32,
    pub delay: i32,
}

#[derive(Clone, Copy)]
struct LocalSound {
    event: &'static str,
    source: usize,
    volume: f32,
    pitch: f32,
}

impl LocalSound {
    fn play(self, audio: &mut Audio, at: [f64; 3]) {
        audio.play_on(self.source, self.event, Some(at), self.volume, self.pitch);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplosionKind {
    Shrapnel,
    Flak,
    Smoke,
    MortarStone,
    Impact,
    Shell,
    ShellNoEffects,
    Fluid(i32),
}

/// The client effects carried by `ClientboundCBCExplodePacket`.
struct Explosion {
    at: [f64; 3],
    block_power: f32,
    knockback: [f64; 3],
    kind: ExplosionKind,
}

impl Explosion {
    fn decode(data: &[u8]) -> Option<Self> {
        let (&packet, body) = data.split_first()?;
        if !matches!(packet, CBC_EXPLODE_PACKET | CBC_FLUID_EXPLODE_PACKET) {
            return None;
        }
        let mut r = Reader::new(body);
        let at = [r.f64().ok()?, r.f64().ok()?, r.f64().ok()?];
        let block_power = r.f32().ok()?;
        let entity_power = r.f32().ok()?;
        // Block changes arrive from the server separately. Validate the list's
        // size and skip the packed BlockPos entries without allocating.
        let count = usize::try_from(r.varint().ok()?).ok()?;
        if count > MAX_EXPLOSION_BLOCKS {
            return None;
        }
        r.bytes(count * 8).ok()?;
        let knockback = [
            f64::from(r.f32().ok()?),
            f64::from(r.f32().ok()?),
            f64::from(r.f32().ok()?),
        ];
        let option = r.varint().ok()?;
        let kind = if packet == CBC_FLUID_EXPLODE_PACKET {
            ExplosionKind::Fluid(option)
        } else {
            match option {
                0 => ExplosionKind::Shrapnel,
                1 => ExplosionKind::Flak,
                2 => ExplosionKind::Smoke,
                3 => ExplosionKind::MortarStone,
                4 => ExplosionKind::Impact,
                5 => ExplosionKind::Shell,
                6 => ExplosionKind::ShellNoEffects,
                _ => return None,
            }
        };
        if r.remaining() != 0
            || !at
                .iter()
                .all(|v| (f64::from(i32::MIN)..f64::from(i32::MAX)).contains(v))
            || !knockback.iter().all(|v| v.is_finite())
            || !block_power.is_finite()
            || !entity_power.is_finite()
            || (kind == ExplosionKind::Shell && !(block_power * 32.0).is_finite())
        {
            return None;
        }
        Some(Self {
            at,
            block_power,
            knockback,
            kind,
        })
    }

    /// `CustomExplosion.playLocalSound`, redirected by `ExplosionMixin`.
    fn local_sound(&self, random: &mut LegacyRandom) -> Option<LocalSound> {
        let (event, volume, pitch) = match self.kind {
            ExplosionKind::Shrapnel => (
                "createbigcannons:shrapnel_shell_explosion",
                6.0,
                0.7 + random.next_float() * 0.4 - 0.2,
            ),
            ExplosionKind::Flak => (
                "createbigcannons:flak_round_explosion",
                4.0,
                1.4 + random.next_float() * 0.4 - 0.2,
            ),
            ExplosionKind::Smoke => (
                "createbigcannons:smoke_shell_detonate",
                2.0,
                1.5 + random.next_float() * 0.1,
            ),
            ExplosionKind::MortarStone => (
                "createbigcannons:mortar_stone_explode",
                3.0,
                0.8 + random.next_float() * 0.4,
            ),
            ExplosionKind::Fluid(_)
            | ExplosionKind::Impact
            | ExplosionKind::Shell
            | ExplosionKind::ShellNoEffects => {
                return None;
            }
        };
        Some(LocalSound {
            event,
            source: NEUTRAL,
            volume,
            pitch,
        })
    }
}

fn fluid_explosion_sounds(
    name: Option<&str>,
    random: &mut LegacyRandom,
) -> (LocalSound, Option<LocalSound>) {
    // DefaultFluidCompat registers effects for the source fluids.
    let (event, volume, pitch, variation) = match name {
        Some("minecraft:water") => ("createbigcannons:water_fluid_release", 2.5, 0.15, 0.3),
        Some("minecraft:lava") => ("createbigcannons:lava_fluid_release", 2.25, 1.0, 0.1),
        Some("create:potion") => ("createbigcannons:potion_fluid_release", 3.0, 0.9, 0.1),
        _ => {
            return (
                LocalSound {
                    event: "minecraft:entity.generic.explode",
                    source: BLOCKS_SOURCE as usize,
                    volume: 4.0,
                    pitch: (1.0 + (random.next_float() - random.next_float()) * 0.2) * 0.7,
                },
                None,
            );
        }
    };
    (
        LocalSound {
            event,
            source: NEUTRAL,
            volume,
            pitch: pitch + random.next_float() * variation,
        },
        Some(LocalSound {
            event: "createbigcannons:fluid_shell_explosion",
            source: NEUTRAL,
            volume: 3.0,
            pitch: 0.9 + random.next_float() * 0.1,
        }),
    )
}

/// `BlastWaveEffectParticle` between ticks.
#[derive(Clone, Debug)]
struct Wave {
    at: DVec3,
    blast_radius: f64,
    sound: String,
    source: usize,
    volume: f32,
    pitch: f32,
    air_absorption: f32,
    shake_power: f32,
    shake_limit: f32,
    functional_radius: f64,
    current_radius: f64,
    age: i32,
}

/// What one tick of a wave does.
#[derive(Debug, PartialEq)]
enum Step {
    Keep,
    Remove,
    /// The front reached the player: a shake of this power, and the sound.
    Reached {
        shake: Option<f32>,
        sound: bool,
    },
}

impl Wave {
    fn new(at: DVec3, wave: &BlastWave, (multiplier, limit): (f32, f32)) -> Self {
        Self {
            at,
            blast_radius: wave.blast_radius,
            sound: wave.sound.clone(),
            source: usize::try_from(wave.source).unwrap_or(0),
            volume: wave.volume,
            pitch: wave.pitch,
            air_absorption: wave.air_absorption,
            shake_power: wave.power * multiplier,
            shake_limit: limit,
            functional_radius: wave.blast_radius.max(f64::from(wave.volume * 16.0)),
            current_radius: 0.0,
            age: 0,
        }
    }

    /// `BlastWaveEffectParticle.tick`, the player at `player`.
    fn tick(&mut self, player: Option<DVec3>) -> Step {
        let Some(player) = player else {
            return Step::Remove;
        };
        let instant = instantaneous();
        if instant {
            self.current_radius = 0.0;
        }
        let speed = f64::from(BLAST_EFFECT_DELAY_SPEED) * 0.05;
        let new_radius = if instant {
            self.functional_radius
        } else {
            (self.current_radius + speed).min(self.functional_radius)
        };
        let dist = player.distance(self.at);
        if self.current_radius <= dist && dist <= new_radius {
            let shake = (dist < self.blast_radius && self.blast_radius > 0.1).then(|| {
                let f = 1.0 - (dist / self.blast_radius) as f32;
                (self.shake_power * (f * f)).min(self.shake_limit)
            });
            let sound = dist < f64::from(self.volume * 16.0);
            return Step::Reached { shake, sound };
        }
        if instant || new_radius >= self.functional_radius {
            return Step::Remove;
        }
        self.current_radius = new_radius;
        // NoRenderParticle's Particle.tick: the timeout.
        let age = self.age;
        self.age += 1;
        if age >= BLAST_WAVE_LIFETIME {
            Step::Remove
        } else {
            Step::Keep
        }
    }
}

/// `ScreenShakeEffect`'s timer and magnitudes (jitter and position unused).
#[derive(Clone, Copy, Debug)]
struct ShakeEffect {
    duration: i32,
    timer: i32,
    yaw: f32,
    pitch: f32,
    roll: f32,
}

impl ShakeEffect {
    fn new(duration: i32, yaw: f32, pitch: f32, roll: f32) -> Self {
        Self {
            duration,
            timer: duration,
            yaw,
            pitch,
            roll,
        }
    }

    /// `ScreenShakeEffect.tick`: whether it is due.
    fn tick(&mut self) -> bool {
        if self.duration < 1 {
            return true;
        }
        self.timer -= 1;
        self.timer < 1
    }
}

/// `CBCScreenShakeHandler`: a damped spring of yaw, pitch and roll degrees.
pub(super) struct ScreenShake {
    delayed: Vec<ShakeEffect>,
    velocity: DVec3,
    acceleration: DVec3,
    displacement: DVec3,
    random: LegacyRandom,
}

impl Default for ScreenShake {
    fn default() -> Self {
        Self {
            delayed: Vec::new(),
            velocity: DVec3::ZERO,
            acceleration: DVec3::ZERO,
            displacement: DVec3::ZERO,
            random: LegacyRandom::new(horizon_client::sound_engine::unique_seed()),
        }
    }
}

impl ScreenShake {
    /// `addEffect`: now, or after its duration.
    fn add_effect(&mut self, effect: ShakeEffect) {
        if effect.duration <= 0 {
            self.immediately_add(effect);
        } else {
            self.delayed.push(effect);
        }
    }

    /// `immediatelyAddEffect` after `modifyScreenShake` (the intensity scale).
    fn immediately_add(&mut self, effect: ShakeEffect) {
        let scale = |m: f32| {
            if SHAKE_INTENSITY == 1.0 {
                m
            } else {
                m * SHAKE_INTENSITY
            }
        };
        let r = &mut self.random;
        let mut kick = |magnitude: f32| {
            let a = r.next_double();
            let b = r.next_double();
            f64::from(scale(magnitude)) * (a - b) * 0.5
        };
        let yaw = kick(effect.yaw);
        let pitch = kick(effect.pitch);
        let roll = kick(effect.roll);
        self.velocity += DVec3::new(yaw, pitch, roll);
    }

    pub(super) fn clear(&mut self) {
        self.displacement = DVec3::ZERO;
        self.velocity = DVec3::ZERO;
        self.acceleration = DVec3::ZERO;
    }

    /// `applyConstraints`: CBC clamps each angle to 45 degrees times the
    /// intensity (its third test reads the pitch again, as here).
    fn apply_constraints(&mut self) {
        let max = 45.0 * f64::from(SHAKE_INTENSITY);
        if self.displacement.x.abs() > max {
            self.displacement.x = max.copysign(self.displacement.x);
            self.velocity.x = 0.0;
        }
        if self.displacement.y.abs() > max {
            self.displacement.y = max.copysign(self.displacement.y);
            self.velocity.y = 0.0;
        }
        if self.displacement.y.abs() > max {
            self.displacement.z = max.copysign(self.displacement.z);
            self.velocity.z = 0.0;
        }
    }

    /// `ModScreenShakeHandler.Impl.tick`.
    fn tick(&mut self) {
        let mut due = Vec::new();
        self.delayed.retain_mut(|effect| {
            let ready = effect.tick();
            if ready {
                due.push(*effect);
            }
            !ready
        });
        for effect in due {
            self.immediately_add(effect);
        }
        let iterations = 10;
        let dt = 1.0 / f64::from(iterations);
        let restitution = f64::from(SHAKE_SPRINGINESS).max(0.005);
        let drag = f64::from(SHAKE_DECAY).max(0.005);
        for _ in 0..iterations {
            let displacement =
                self.displacement + self.velocity * dt + self.acceleration * (0.5 * dt * dt);
            let acceleration = self.displacement * -restitution + self.velocity * -drag;
            let velocity = self.velocity + (self.acceleration + acceleration) * (0.5 * dt);
            self.displacement = displacement;
            self.velocity = velocity;
            self.acceleration = acceleration;
            self.apply_constraints();
        }
        if self.displacement.length_squared() < 1e-4
            && self.velocity.length_squared() < 1e-4
            && self.acceleration.length_squared() < 1e-4
        {
            self.clear();
        }
    }

    /// `applyEffects`: the yaw, pitch and roll to add this frame.
    pub(super) fn angles(&self, partial: f32) -> [f32; 3] {
        let current = self.displacement
            + self.velocity * f64::from(partial)
            + self.acceleration * f64::from(partial * partial * 0.5);
        [current.x as f32, current.y as f32, current.z as f32]
    }
}

struct SmokeEmitter {
    sizes: [f32; 3],
    gas_color: Option<i32>,
    first_tick: bool,
    random: LegacyRandom,
}

impl SmokeEmitter {
    fn tick(
        &mut self,
        id: i32,
        game_time: i64,
        at: [f64; 3],
        particles: &mut horizon_client::particles::ParticleEngine,
    ) -> usize {
        let first = std::mem::replace(&mut self.first_tick, false);
        if !first && game_time.wrapping_add(i64::from(id)) % 5 != 0 {
            return 0;
        }
        let count = ((self.sizes[0] + self.sizes[1] + self.sizes[2]) / 3.0).ceil() as usize;
        let count = count.min(1024);
        let options = ParticleOptions::cbc(match self.gas_color {
            Some(rgb) => CbcParticle::GasCloud {
                scale: 2.0,
                color: [16, 8, 0].map(|shift| ((rgb >> shift) & 255) as f32 / 255.0),
            },
            None => CbcParticle::SmokeShellSmoke { scale: 4.0 },
        });
        for _ in 0..count {
            let position = std::array::from_fn(|i| {
                let size = f64::from(self.sizes[i]);
                self.random.next_double() * size - size * 0.5 + at[i]
            });
            let velocity = [0.05, 0.02, 0.05].map(|scale| self.random.next_gaussian() * scale);
            particles.add_with(&options, position, velocity, true, false);
        }
        count
    }
}

/// The CBC effects of a server connection.
pub(super) struct State {
    pub(super) break_progress: BreakProgress,
    hit_effects: hits::Effects,
    impact_packet: Option<([f64; 3], u64)>,
    explosions: std::collections::BTreeMap<&'static str, u64>,
    emitter_particles: [usize; 2],
    tracer_meshes: std::cell::RefCell<[Option<TracerFlight>; 3]>,
    emitters: HashMap<i32, SmokeEmitter>,
    waves: Vec<Wave>,
    pub(super) shake: ScreenShake,
    /// Stand-in for `level.random` for sounds and explosion pitch.
    random: LegacyRandom,
    /// The `ShellFlyingSoundInstance` each shell plays, kept to see whether
    /// it stopped itself.
    flybys: HashMap<i32, InstanceId>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            break_progress: Default::default(),
            hit_effects: Default::default(),
            impact_packet: None,
            explosions: Default::default(),
            emitter_particles: [0; 2],
            tracer_meshes: Default::default(),
            emitters: HashMap::new(),
            waves: Vec::new(),
            shake: ScreenShake::default(),
            random: LegacyRandom::new(horizon_client::sound_engine::unique_seed()),
            flybys: HashMap::new(),
        }
    }
}

/// CBC keeps one crack per position and sweeps expired entries every 20 renderer ticks.
#[derive(Debug, Default)]
pub(super) struct BreakProgress {
    ticks: u64,
    blocks: HashMap<[i32; 3], (u8, u64)>,
    last_clear: Option<([i32; 3], u64)>,
}

impl BreakProgress {
    pub(super) fn receive(&mut self, data: &[u8]) {
        let mut reader = Reader::new(data);
        let (Ok(id), Ok(pos), Ok(stage)) = (reader.varint(), reader.i64(), reader.varint()) else {
            return;
        };
        if id != i32::from(horizon_client::net::CBC_BREAK_PROGRESS_PACKET)
            || reader.remaining() != 0
        {
            return;
        }
        let pos = horizon_protocol::buf::unpack_block_pos(pos);
        if (0..10).contains(&stage) {
            self.blocks.insert(pos, (stage as u8, self.ticks));
        } else {
            self.blocks.remove(&pos);
            self.last_clear = Some((pos, self.ticks));
        }
    }

    pub(super) fn tick(&mut self) {
        self.ticks += 1;
        if self.ticks % 20 == 0 {
            self.blocks
                .retain(|_, (_, updated)| self.ticks - *updated <= 400);
        }
    }

    pub(super) fn stages(&self) -> impl Iterator<Item = ([i32; 3], u8)> + '_ {
        self.blocks.iter().map(|(&pos, &(stage, _))| (pos, stage))
    }
}

#[derive(Clone, Copy)]
pub(super) enum TracerKind {
    Shot,
    MachineGun,
    Autocannon,
}

impl TracerKind {
    const ALL: [Self; 3] = [Self::Shot, Self::MachineGun, Self::Autocannon];

    fn name(self) -> &'static str {
        match self {
            Self::Shot => "shot",
            Self::MachineGun => "machine_gun_bullet",
            Self::Autocannon => "ap_autocannon",
        }
    }
}

#[derive(serde::Serialize)]
pub(super) struct TracerSample {
    pub(super) position: [f64; 3],
    pub(super) vertices: usize,
    pub(super) min_offset: [f32; 3],
    pub(super) max_offset: [f32; 3],
}

#[derive(serde::Serialize)]
struct TracerFlight {
    id: i32,
    first: TracerSample,
    last: Option<TracerSample>,
}

impl State {
    pub(super) fn observe_tracer(&self, kind: TracerKind, id: i32, sample: TracerSample) {
        let mut meshes = self.tracer_meshes.borrow_mut();
        let slot = &mut meshes[kind as usize];
        match slot {
            Some(flight) if flight.id == id => flight.last = Some(sample),
            _ => {
                *slot = Some(TracerFlight {
                    id,
                    first: sample,
                    last: None,
                });
            }
        }
    }

    pub(super) fn observation(&self) -> serde_json::Value {
        use serde_json::json;
        let cracks: Vec<_> = self
            .break_progress
            .stages()
            .map(|(pos, stage)| json!({ "pos": pos, "stage": stage }))
            .collect();
        let meshes = self.tracer_meshes.borrow();
        let tracers: serde_json::Map<String, serde_json::Value> = TracerKind::ALL
            .into_iter()
            .filter_map(|kind| Some((kind.name().into(), json!(meshes[kind as usize].as_ref()?))))
            .collect();
        json!({
            "cracks": cracks,
            "crack_clear": self.break_progress.last_clear,
            "impact": self.impact_packet,
            "explosions": self.explosions,
            "smoke_particles_requested": self.emitter_particles[0],
            "gas_particles_requested": self.emitter_particles[1],
            "tracers": tracers,
        })
    }

    pub(super) fn add_smoke_emitter(&mut self, id: i32, kind: &str) -> bool {
        let gas_color = match kind {
            "createbigcannons:smoke_emitter" => None,
            "createbigcannons:gas_cloud" => Some(0),
            _ => return false,
        };
        self.emitters.insert(
            id,
            SmokeEmitter {
                sizes: [0.0; 3],
                gas_color,
                first_tick: true,
                random: LegacyRandom::new(horizon_client::sound_engine::unique_seed()),
            },
        );
        true
    }

    pub(super) fn remove_smoke_emitter(&mut self, id: i32) {
        self.emitters.remove(&id);
    }

    pub(super) fn smoke_emitter_data(
        &mut self,
        id: i32,
        values: &[(u8, i32, horizon_protocol::entity_data::Value)],
    ) {
        use horizon_protocol::entity_data::Value;
        let Some(emitter) = self.emitters.get_mut(&id) else {
            return;
        };
        for (index, _, value) in values {
            match (*index, value) {
                (8..=10, Value::Float(size)) if size.is_finite() => {
                    emitter.sizes[usize::from(*index - 8)] = size.abs();
                }
                (11, Value::Int(color)) if emitter.gas_color.is_some() => {
                    emitter.gas_color = Some(*color)
                }
                _ => {}
            }
        }
    }

    pub(super) fn tick_smoke_emitters(
        &mut self,
        game_time: i64,
        position: impl Fn(i32) -> Option<[f64; 3]>,
        particles: &mut horizon_client::particles::ParticleEngine,
    ) {
        let mut ids: Vec<i32> = self.emitters.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            if let Some(at) = position(id) {
                let emitter = self.emitters.get_mut(&id).expect("tracked emitter");
                let slot = usize::from(emitter.gas_color.is_some());
                self.emitter_particles[slot] += emitter.tick(id, game_time, at, particles);
            }
        }
    }

    /// A blast wave particle, or `false` when `p` is another particle.
    pub(super) fn add_wave(&mut self, p: &play::LevelParticles) -> bool {
        let shake = match p.particle.id {
            CBC_CANNON_BLAST_WAVE => BIG_CANNON_SHAKE,
            CBC_SHELL_BLAST_WAVE => SHELL_SHAKE,
            _ => return false,
        };
        let Some(ParticleData::BlastWave(wave)) = &p.particle.data else {
            return false;
        };
        // ClientPacketListener.handleParticleEvent: count 0 makes one particle
        // at the position, any other count that many (spread, as here unused).
        let count = if p.count == 0 { 1 } else { p.count.max(0) };
        if !DVec3::from_array(p.pos).is_finite() || !wave.blast_radius.is_finite() {
            return true;
        }
        for _ in 0..count.min(64) {
            self.push_wave(p.pos, wave, shake);
        }
        true
    }

    /// `ShellExplosion.spawnParticles`'s `ShellBlastWaveEffectParticleData`.
    fn add_shell_explosion(&mut self, explosion: &Explosion, random: &mut LegacyRandom) -> bool {
        if explosion.kind != ExplosionKind::Shell {
            return false;
        }
        let power = explosion.block_power;
        let wave = BlastWave {
            blast_radius: f64::from(power * 12.0),
            sound: "createbigcannons:shell_explosion".into(),
            fixed_range: None,
            source: BLOCKS_SOURCE,
            volume: (power * 2.0).max(16.0),
            pitch: 0.8 + random.next_float() * 0.4,
            air_absorption: 2.0,
            power,
        };
        self.push_wave(explosion.at, &wave, SHELL_SHAKE);
        true
    }

    fn push_wave(&mut self, at: [f64; 3], wave: &BlastWave, shake: (f32, f32)) {
        self.waves
            .push(Wave::new(DVec3::from_array(at), wave, shake));
    }

    /// Particles go with their level.
    pub(super) fn clear_waves(&mut self) {
        self.waves.clear();
        self.emitters.clear();
    }

    /// One client tick: waves move on (`running`: the particle engine and
    /// the shake handler tick), returning the sounds to start.
    pub(super) fn tick(&mut self, running: bool, player: Option<DVec3>) -> Vec<BlastSound> {
        let mut sounds = Vec::new();
        if !running {
            return sounds;
        }
        let mut shakes = Vec::new();
        self.waves.retain_mut(|wave| match wave.tick(player) {
            Step::Keep => true,
            Step::Remove => false,
            Step::Reached { shake, sound } => {
                if let Some(shake) = shake {
                    shakes.push(ShakeEffect::new(0, shake, shake * 0.5, shake * 0.5));
                }
                if sound {
                    sounds.push(BlastSound {
                        event: wave.sound.clone(),
                        source: wave.source,
                        at: wave.at.to_array(),
                        volume: wave.volume,
                        pitch: wave.pitch,
                        air_absorption: if BLAST_SOUND_AIR_ABSORPTION {
                            wave.air_absorption
                        } else {
                            0.0
                        },
                        delay: 0,
                    });
                }
                false
            }
        });
        for effect in shakes {
            self.shake.add_effect(effect);
        }
        if player.is_some() {
            self.shake.tick();
        }
        sounds
    }
}

/// `ClientboundBlastSoundPacket` on CBC's root channel, delayed for a
/// listener at `player` (`CBCClientHandlers.playBlastSound`).
pub(super) fn blast_sound(data: &[u8], player: Option<DVec3>) -> Option<BlastSound> {
    let mut r = Reader::new(data);
    if r.varint().ok()? != BLAST_SOUND_PACKET {
        return None;
    }
    // SoundEvent.DIRECT_STREAM_CODEC: its id and optional fixed range.
    let event = r.string(32767).ok()?;
    if r.bool().ok()? {
        r.f32().ok()?;
    }
    let source = usize::try_from(r.varint().ok()?).ok()?;
    let at = [r.f64().ok()?, r.f64().ok()?, r.f64().ok()?];
    let volume = r.f32().ok()?;
    let pitch = r.f32().ok()?;
    let air_absorption = r.f32().ok()?;
    if r.remaining() != 0 || !at.iter().all(|v| v.is_finite()) {
        return None;
    }
    let delay = match player {
        Some(player) if !instantaneous() => {
            let seconds =
                player.distance(DVec3::from_array(at)) / f64::from(BLAST_EFFECT_DELAY_SPEED);
            (seconds * 20.0).floor() as i32
        }
        _ => 0,
    };
    Some(BlastSound {
        event,
        source,
        at,
        volume,
        pitch,
        air_absorption: if BLAST_SOUND_AIR_ABSORPTION {
            air_absorption
        } else {
            0.0
        },
        delay,
    })
}

fn play(audio: &mut Audio, random: &mut LegacyRandom, sound: BlastSound) {
    // RandomSource.create(level.random.nextLong()).
    let seed = random.next_long();
    audio.play_delayed_at(
        &sound.event,
        sound.source,
        sound.at,
        sound.volume,
        sound.pitch,
        seed,
        sound.delay,
        sound.air_absorption,
    );
}

impl Game<'_> {
    fn cbc_player(&self) -> Option<DVec3> {
        self.player
            .as_ref()
            .map(|p| DVec3::new(p.position.x, p.position.y, p.position.z))
    }

    /// A CBC blast wave particle: taken here, never drawn.
    pub(super) fn cbc_blast_wave(&mut self, p: &play::LevelParticles) -> bool {
        self.remote
            .as_mut()
            .is_some_and(|r| r.cbc_effects.add_wave(p))
    }

    /// `ClientboundBlastSoundPacket`.
    pub(super) fn cbc_blast_sound(&mut self, data: &[u8]) {
        let player = self.cbc_player();
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if let Some(sound) = blast_sound(data, player) {
            play(&mut self.audio, &mut r.cbc_effects.random, sound);
        }
    }

    /// `CBCClientHandlers.addExplosionFromServer`: finalize the effects, then push the player.
    pub(super) fn cbc_explode(&mut self, data: &[u8]) {
        let (Some(explosion), Some(remote), Some(player)) = (
            Explosion::decode(data),
            self.remote.as_mut(),
            self.player.as_mut(),
        ) else {
            return;
        };
        let random = self.particles.cbc_level_random();
        // Explosion.finalizeExplosion evaluates the vanilla pitch before
        // ExplosionMixin redirects the playLocalSound call.
        random.next_float();
        random.next_float();
        if let Some(sound) = explosion.local_sound(random) {
            sound.play(&mut self.audio, explosion.at);
        }
        if remote.cbc_effects.add_shell_explosion(&explosion, random) {
            let [x, y, z] = explosion.at.map(|coordinate| coordinate.floor() as i32);
            let facts = self.res.facts();
            let plume = (MIN_Y + 1..=MIN_Y + HEIGHT).contains(&y)
                && facts.is_air(self.world.block(x, y + 1, z))
                && !facts.is_air(self.world.block(x, y - 1, z));
            let options = ParticleOptions::cbc(CbcParticle::ShellExplosionCloud {
                power: explosion.block_power,
                plume,
            });
            self.particles
                .add_with(&options, explosion.at, [0.0; 3], true, false);
        }
        let effect_name = match explosion.kind {
            ExplosionKind::Shrapnel => Some("shrapnel"),
            ExplosionKind::Flak => Some("flak"),
            ExplosionKind::Smoke => Some("smoke"),
            ExplosionKind::MortarStone => Some("mortar_stone"),
            ExplosionKind::Fluid(_) => Some("fluid"),
            _ => None,
        };
        if let Some(name) = effect_name {
            *remote.cbc_effects.explosions.entry(name).or_default() += 1;
        }
        let cloud = match explosion.kind {
            ExplosionKind::Shrapnel => Some(CbcParticle::ShrapnelCloud),
            ExplosionKind::Flak => Some(CbcParticle::FlakCloud),
            ExplosionKind::Fluid(fluid) => {
                let name = remote
                    .builtin
                    .get("minecraft:fluid")
                    .and_then(|registry| registry.name(fluid));
                let (sound, extra) =
                    fluid_explosion_sounds(name, self.particles.cbc_level_random());
                sound.play(&mut self.audio, explosion.at);
                if let Some(sound) = extra {
                    sound.play(&mut self.audio, explosion.at);
                    Some(CbcParticle::FluidCloud)
                } else {
                    let kind = if explosion.block_power >= 2.0 {
                        ParticleType::ExplosionEmitter
                    } else {
                        ParticleType::Explosion
                    };
                    self.particles.add_with(
                        &ParticleOptions::simple(kind),
                        explosion.at,
                        [1.0, 0.0, 0.0],
                        false,
                        false,
                    );
                    None
                }
            }
            ExplosionKind::MortarStone => {
                self.particles
                    .cbc_mortar_stone_cloud(explosion.at, explosion.block_power);
                None
            }
            _ => None,
        };
        if let Some(cloud) = cloud {
            self.particles.add_with(
                &ParticleOptions::cbc(cloud),
                explosion.at,
                [0.0; 3],
                true,
                false,
            );
        }
        let [x, y, z] = explosion.knockback;
        player.velocity = Vec3::new(
            player.velocity.x + x,
            player.velocity.y + y,
            player.velocity.z + z,
        );
    }

    pub(super) fn tick_cbc_effects(&mut self, running: bool) {
        let player = self.cbc_player();
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        for sound in r.cbc_effects.tick(running, player) {
            play(&mut self.audio, &mut r.cbc_effects.random, sound);
        }
    }

    /// A `ShellFlyingSoundInstance` that stopped itself set its shell's
    /// `localSoundCooldown` to 3; one that never played or was dropped by the
    /// engine left it at -1, so the shell stays silent, as in CBC.
    pub(super) fn settle_cbc_flybys(&mut self) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let audio = &mut self.audio;
        let shells = &mut r.create_contraptions;
        r.cbc_effects.flybys.retain(|&shell, &mut id| {
            let stopped = match audio.engine.instance(id) {
                Some(instance) if instance.stopped => true,
                Some(_) if audio.engine.is_active(id) => return true,
                _ => false,
            };
            if stopped {
                shells.set_shell_sound_cooldown(shell, 3);
            }
            audio.release(id);
            false
        });
    }

    /// `AbstractBigCannonProjectile.tick` on the client once the shells have
    /// moved: their trails, and the flyby sound of a shell heading at the
    /// player.
    pub(super) fn tick_cbc_shells(&mut self) {
        let player = self.cbc_player();
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let ticks = r.create_contraptions.shell_ticks(player);
        if ticks.is_empty() {
            return;
        }
        let sounds = r.create_contraptions.shell_sounds();
        for shell in ticks {
            if let Some(round) = shell.round {
                self.particles.cbc_autocannon_trail(shell.from, shell.to);
                if let Some((volume, pitch)) = shell.round_sound {
                    self.audio
                        .play_on(NEUTRAL, round.flyby_event(), Some(shell.to), volume, pitch);
                }
                continue;
            }
            self.particles.cbc_shell_trail(shell.from, shell.to);
            let (Some(radius), Some(sound)) =
                (shell.flyby_radius, sounds.iter().find(|s| s.id == shell.id))
            else {
                continue;
            };
            let base_pitch = 0.95 + r.cbc_effects.random.next_float() * 0.3;
            let instance =
                SoundInstance::cbc_shell_flying(sound, radius, base_pitch, SoundEngine::unseeded());
            let (id, _) = self.audio.play_instance(instance, true);
            if let Some(old) = r.cbc_effects.flybys.insert(shell.id, id) {
                self.audio.release(old);
            }
            r.create_contraptions.set_shell_sound_cooldown(shell.id, -1);
        }
    }

    /// The screen shake's yaw, pitch and roll for this frame.
    pub(super) fn cbc_camera_shake(&self, partial: f32) -> [f32; 3] {
        self.remote
            .as_ref()
            .map_or([0.0; 3], |r| r.cbc_effects.shake.angles(partial))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::settings::SOUND_SOURCES;
    use horizon_protocol::{buf::Writer, entity_data::Particle};

    #[test]
    fn cbc_break_progress_tracks_each_position_and_expires_on_a_sweep() {
        use horizon_protocol::buf::pack_block_pos;
        fn packet(pos: [i32; 3], stage: i32) -> Vec<u8> {
            let mut bytes = Writer::new();
            bytes.varint(i32::from(horizon_client::net::CBC_BREAK_PROGRESS_PACKET));
            bytes.i64(pack_block_pos(pos));
            bytes.varint(stage);
            bytes.data
        }
        let mut progress = BreakProgress::default();
        let a = [-30, -40, 45];
        let b = [100, 70, -100];
        progress.receive(&packet(a, 0));
        progress.receive(&packet(b, 9));
        progress.receive(&packet(a, 7));
        let mut malformed = packet(a, -1);
        malformed.push(0);
        progress.receive(&malformed);
        assert!(progress.last_clear.is_none());
        assert_eq!(
            progress.stages().collect::<HashMap<_, _>>(),
            HashMap::from([(a, 7), (b, 9)])
        );
        progress.receive(&packet(a, -1));
        assert_eq!(progress.last_clear, Some((a, 0)));
        for _ in 0..419 {
            progress.tick();
        }
        assert_eq!(progress.stages().collect::<Vec<_>>(), [(b, 9)]);
        progress.tick();
        assert!(progress.stages().next().is_none());
        assert_eq!(progress.last_clear, Some((a, 0)));
        progress.receive(&packet(a, 0));
        progress.receive(&packet(a, 10));
        assert!(progress.stages().next().is_none());
    }

    fn explosion_packet(kind: i32, block_power: f32, entity_power: f32) -> Vec<u8> {
        let mut w = Writer::new();
        w.varint(i32::from(CBC_EXPLODE_PACKET));
        for value in [1.25, -2.5, 3.75] {
            w.f64(value);
        }
        w.f32(block_power);
        w.f32(entity_power);
        w.varint(2);
        w.i64(-123);
        w.i64(456);
        for value in [0.125, -0.25, 0.5] {
            w.f32(value);
        }
        w.varint(kind);
        w.data
    }

    #[test]
    fn cbc_explosion_packet_checks_the_entire_body() {
        let packet = explosion_packet(5, 4.0, 9.0);
        let explosion = Explosion::decode(&packet).unwrap();
        assert_eq!(explosion.at, [1.25, -2.5, 3.75]);
        assert_eq!(explosion.block_power, 4.0);
        assert_eq!(explosion.knockback, [0.125, -0.25, 0.5]);
        assert_eq!(explosion.kind, ExplosionKind::Shell);
        for n in 0..packet.len() {
            assert!(Explosion::decode(&packet[..n]).is_none(), "length {n}");
        }
        let mut bad_center = packet.clone();
        bad_center[9..17].copy_from_slice(&f64::MAX.to_be_bytes());
        assert!(Explosion::decode(&bad_center).is_none());
        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(Explosion::decode(&trailing).is_none());
        assert!(Explosion::decode(&explosion_packet(7, 4.0, 9.0)).is_none());
        assert!(Explosion::decode(&explosion_packet(5, f32::NAN, 9.0)).is_none());
        assert!(Explosion::decode(&explosion_packet(5, 4.0, f32::INFINITY)).is_none());

        let mut bad_count = Writer::new();
        bad_count.bytes(&packet[..33]);
        bad_count.varint(MAX_EXPLOSION_BLOCKS as i32 + 1);
        bad_count.bytes(&packet[34..]);
        assert!(Explosion::decode(&bad_count.data).is_none());
    }

    #[test]
    fn fluid_explosion_uses_the_fluid_registry_id_at_the_end() {
        let mut packet = explosion_packet(6, 4.0, 7.0);
        packet[0] = CBC_FLUID_EXPLODE_PACKET;
        packet.pop();
        let mut fluid = Writer::new();
        fluid.varint(193);
        packet.extend(fluid.data);
        let explosion = Explosion::decode(&packet).unwrap();
        assert_eq!(explosion.kind, ExplosionKind::Fluid(193));
        assert_eq!(explosion.at, [1.25, -2.5, 3.75]);
        assert!(Explosion::decode(&packet[..packet.len() - 1]).is_none());
    }

    #[test]
    fn cbc_immediate_explosion_sounds() {
        let mut audio = Audio::silent(std::path::Path::new(""));
        let sample = LegacyRandom::new(7).next_float();
        let mut sound = |kind| {
            let explosion = Explosion::decode(&explosion_packet(kind, 4.0, 2.0)).unwrap();
            let sound = explosion.local_sound(&mut LegacyRandom::new(7)).unwrap();
            assert_immediate_sound(&mut audio, sound, explosion.at);
            sound_fields(sound)
        };
        // ShrapnelExplosion.java:29-31, CBCSoundEvents.java:107-108.
        assert_eq!(
            sound(0),
            (
                "createbigcannons:shrapnel_shell_explosion",
                "neutral",
                6.0,
                0.7 + sample * 0.4 - 0.2,
            )
        );
        // FlakExplosion.java:29-31, CBCSoundEvents.java:49-50.
        assert_eq!(
            sound(1),
            (
                "createbigcannons:flak_round_explosion",
                "neutral",
                4.0,
                1.4 + sample * 0.4 - 0.2,
            )
        );
        // SmokeExplosion.java:26-27, CBCSoundEvents.java:111-113.
        assert_eq!(
            sound(2),
            (
                "createbigcannons:smoke_shell_detonate",
                "neutral",
                2.0,
                1.5 + sample * 0.1,
            )
        );
        // MortarStoneExplosion.java:50-51, CBCSoundEvents.java:70-73.
        assert_eq!(
            sound(3),
            (
                "createbigcannons:mortar_stone_explode",
                "neutral",
                3.0,
                0.8 + sample * 0.4,
            )
        );
    }

    #[test]
    fn cbc_smoke_shell_packet_plays_the_extinguish_fire_alias() {
        // CBCRootNetwork and ClientboundCBCExplodePacket.STREAM_CODEC, with
        // the powers from SmokeShellProjectile.detonate and no player hit.
        let at = [1.25, -2.5, 3.75];
        let mut packet = Writer::new();
        packet.varint(20);
        for coordinate in at {
            packet.f64(coordinate);
        }
        packet.f32(0.0);
        packet.f32(2.0);
        packet.varint(0); // Empty block list for BlockInteraction.KEEP.
        for component in [0.0; 3] {
            packet.f32(component);
        }
        packet.varint(2); // ExplosionType.SMOKE.

        let explosion = Explosion::decode(&packet.data).unwrap();
        assert_eq!(explosion.at, at);
        assert_eq!(explosion.block_power, 0.0);
        assert_eq!(explosion.knockback, [0.0; 3]);
        assert_eq!(explosion.kind, ExplosionKind::Smoke);
        let sound = explosion.local_sound(&mut LegacyRandom::new(7)).unwrap();
        // SmokeExplosion.playLocalSound and CBCSoundEvents.SMOKE_SHELL_DETONATE.
        let (event, source, volume, pitch) = sound_fields(sound);
        assert_eq!(
            (event, source, volume),
            ("createbigcannons:smoke_shell_detonate", "neutral", 2.0)
        );
        assert!((1.5..1.6).contains(&pitch));

        let mut audio = Audio::silent(std::path::Path::new(""));
        audio.engine.registry.add_file(
            "createbigcannons",
            br#"{"smoke_shell_detonate":{"sounds":[{"type":"event","name":"minecraft:entity.generic.extinguish_fire"}]}}"#,
            &|_, _| false,
        );
        audio.engine.registry.add_file(
            "minecraft",
            br#"{"entity.generic.extinguish_fire":{"sounds":["test/extinguish_fire"]}}"#,
            &|_, _| true,
        );
        audio.engine.loaded = true;
        audio.record_heard = true;
        audio.set_listener(at, 0.0, 0.0);
        sound.play(&mut audio, explosion.at);
        assert_eq!(audio.heard.get(event), Some(&1));
        let [started] = audio.engine.notifications.as_slice() else {
            panic!(
                "expected one immediate sound, got {:?}",
                audio.engine.notifications
            );
        };
        assert_eq!(started.identifier, event);
        assert_eq!(started.position, at);
    }

    fn sound_fields(sound: LocalSound) -> (&'static str, &'static str, f32, f32) {
        (
            sound.event,
            SOUND_SOURCES[sound.source],
            sound.volume,
            sound.pitch,
        )
    }

    fn assert_immediate_sound(audio: &mut Audio, sound: LocalSound, at: [f64; 3]) {
        let (namespace, event) = sound.event.split_once(':').unwrap();
        let sounds = serde_json::json!({(event): {"sounds": [sound.event]}}).to_string();
        audio
            .engine
            .registry
            .add_file(namespace, sounds.as_bytes(), &|_, _| true);
        audio.engine.loaded = true;
        audio.record_heard = true;
        for listener in [at, [at[0] + 24.0, at[1], at[2]]] {
            audio.set_listener(listener, 0.0, 0.0);
            sound.play(audio, at);
            let notifications = std::mem::take(&mut audio.engine.notifications);
            let [started] = notifications.as_slice() else {
                panic!("expected one immediate sound at {listener:?}, got {notifications:?}");
            };
            // ShrapnelExplosion.java:31, FlakExplosion.java:31, SmokeExplosion.java:27,
            // MortarStoneExplosion.java:51, FluidExplosion.java:40-41, DefaultFluidCompat.java:101-122:
            // Immediate positional playback at the explosion, using normal attenuation.
            assert_eq!(
                (
                    started.identifier.as_str(),
                    started.position,
                    started.relative,
                    started.range.is_finite()
                ),
                (sound.event, at, false, true),
            );
        }
    }

    #[test]
    fn cbc_fluid_shells_play_release_then_explosion() {
        let mut audio = Audio::silent(std::path::Path::new(""));
        let mut random = LegacyRandom::new(7);
        let (first, second) = (random.next_float(), random.next_float());
        let mut sounds = |name| {
            let (release, explosion) =
                fluid_explosion_sounds(Some(name), &mut LegacyRandom::new(7));
            let explosion = explosion.unwrap();
            assert_immediate_sound(&mut audio, release, [1.25, -2.5, 3.75]);
            assert_immediate_sound(&mut audio, explosion, [1.25, -2.5, 3.75]);
            (sound_fields(release), sound_fields(explosion))
        };
        let explosion = (
            "createbigcannons:fluid_shell_explosion",
            "neutral",
            3.0,
            0.9 + second * 0.1,
        );
        // DefaultFluidCompat.java:48,101-102, CBCSoundEvents.java:53-54,116-118.
        assert_eq!(
            sounds("minecraft:water"),
            (
                (
                    "createbigcannons:water_fluid_release",
                    "neutral",
                    2.5,
                    0.15 + first * 0.3
                ),
                explosion
            )
        );
        // DefaultFluidCompat.java:49,111-112, CBCSoundEvents.java:53-54,61-63.
        assert_eq!(
            sounds("minecraft:lava"),
            (
                (
                    "createbigcannons:lava_fluid_release",
                    "neutral",
                    2.25,
                    1.0 + first * 0.1
                ),
                explosion
            )
        );
        // DefaultFluidCompat.java:55,121-122, CBCSoundEvents.java:53-54,81-83.
        assert_eq!(
            sounds("create:potion"),
            (
                (
                    "createbigcannons:potion_fluid_release",
                    "neutral",
                    3.0,
                    0.9 + first * 0.1
                ),
                explosion
            )
        );
    }

    #[test]
    fn cbc_fluid_shell_without_a_registered_effect_plays_generic_explosion() {
        let mut audio = Audio::silent(std::path::Path::new(""));
        let mut random = LegacyRandom::new(7);
        let pitch = (1.0 + (random.next_float() - random.next_float()) * 0.2) * 0.7;
        for name in [None, Some("test:unregistered_effect")] {
            let (sound, extra) = fluid_explosion_sounds(name, &mut LegacyRandom::new(7));
            // FluidExplosion.java:37-45.
            assert!(extra.is_none());
            // FluidExplosion.java:40-41.
            assert_eq!(
                sound_fields(sound),
                ("minecraft:entity.generic.explode", "block", 4.0, pitch)
            );
            assert_immediate_sound(&mut audio, sound, [1.25, -2.5, 3.75]);
        }
    }

    #[test]
    fn cbc_shell_explosion_sound_arrives_at_the_wave_front_with_a_strict_cutoff() {
        let pitch = 0.8 + LegacyRandom::new(7).next_float() * 0.4;
        for (power, volume, ticks) in [(4.0, 16.0, 16), (10.0, 20.0, 20)] {
            let explosion = Explosion::decode(&explosion_packet(5, power, 2.0)).unwrap();
            // CustomExplosion.java:21-22, ShellExplosion.java:55-63.
            assert!(explosion.local_sound(&mut LegacyRandom::new(7)).is_none());
            let cutoff = f64::from(volume * 16.0);
            for (distance, audible) in
                [(cutoff - 1.0, true), (cutoff, false), (cutoff + 1.0, false)]
            {
                let mut state = State::default();
                // ShellExplosion.java:55-62.
                assert!(state.add_shell_explosion(&explosion, &mut LegacyRandom::new(7)));
                // ShellExplosion.java:58.
                assert_eq!(state.waves[0].blast_radius, f64::from(power * 12.0));
                let player = Some(DVec3::from_array(explosion.at) + DVec3::X * distance);
                for _ in 1..ticks {
                    // CBCCfgClient.java:19, BlastWaveEffectParticle.java:63-67.
                    assert!(state.tick(true, player).is_empty());
                }
                let sounds = state.tick(true, player);
                if audible {
                    // ShellExplosion.java:59-60, CBCSoundEvents.java:96-99,
                    // BlastWaveEffectParticle.java:74-80.
                    assert_eq!(
                        sounds,
                        [BlastSound {
                            event: "createbigcannons:shell_explosion".into(),
                            source: 4,
                            at: explosion.at,
                            volume,
                            pitch,
                            air_absorption: 2.0,
                            delay: 0,
                        }]
                    );
                } else {
                    // BlastWaveEffectParticle.java:74-75.
                    assert!(sounds.is_empty());
                }
                // BlastWaveEffectParticle.java:82-85.
                assert!(state.tick(true, player).is_empty());
            }
        }
    }

    #[test]
    fn cbc_impact_and_shell_without_effects_are_silent() {
        let mut state = State::default();
        let mut random = LegacyRandom::new(7);
        let impact = Explosion::decode(&explosion_packet(4, 4.0, 2.0)).unwrap();
        // CBCClientHandlers.java:116-119, ImpactExplosion.java:24-35, CustomExplosion.java:21-22.
        assert!(impact.local_sound(&mut random).is_none());
        // ImpactExplosion.java:24-35, CustomExplosion.java:91-92.
        assert!(!state.add_shell_explosion(&impact, &mut random));
        let shell = Explosion::decode(&explosion_packet(6, 4.0, 2.0)).unwrap();
        // CBCClientHandlers.java:117-119, CustomExplosion.java:21-22.
        assert!(shell.local_sound(&mut random).is_none());
        // ShellExplosion.java:51,55-57.
        assert!(!state.add_shell_explosion(&shell, &mut random));
    }

    fn wave(blast_radius: f64, volume: f32) -> BlastWave {
        BlastWave {
            blast_radius,
            sound: "createbigcannons:fire_big_cannon".into(),
            fixed_range: None,
            source: 4,
            volume,
            pitch: 0.9,
            air_absorption: 2.0,
            power: 3.0,
        }
    }

    fn packet(id: i32, wave: BlastWave, at: [f64; 3]) -> play::LevelParticles {
        play::LevelParticles {
            particle: Particle {
                id,
                data: Some(ParticleData::BlastWave(wave)),
            },
            override_limiter: true,
            always_show: true,
            pos: at,
            dist: [0.0; 3],
            max_speed: [1.0; 3],
            count: 0,
            randomization: 0,
        }
    }

    #[test]
    fn the_blast_reaches_a_player_when_its_front_passes() {
        let mut state = State::default();
        // 40 blocks away: 16 blocks a tick, so the third tick (radius 32..48).
        assert!(state.add_wave(&packet(
            CBC_CANNON_BLAST_WAVE,
            wave(50.0, 5.0),
            [40.0, 0.0, 0.0]
        )));
        let player = Some(DVec3::ZERO);
        assert!(state.tick(true, player).is_empty());
        assert!(state.tick(true, player).is_empty());
        // A paused game holds the wave.
        assert!(state.tick(false, player).is_empty());
        let sounds = state.tick(true, player);
        assert_eq!(
            sounds,
            vec![BlastSound {
                event: "createbigcannons:fire_big_cannon".into(),
                source: 4,
                at: [40.0, 0.0, 0.0],
                volume: 5.0,
                pitch: 0.9,
                air_absorption: 2.0,
                delay: 0,
            }]
        );
        assert!(state.waves.is_empty());
        // Inside the blast radius: shake min(45, 3 * 6 * (1 - 40/50)^2).
        assert!(state.shake.velocity.length() > 0.0);
        // The spring settles and clears.
        for _ in 0..400 {
            state.tick(true, player);
        }
        assert_eq!(state.shake.angles(0.5), [0.0; 3]);
    }

    #[test]
    fn far_blasts_are_silent_and_vanish_at_their_functional_radius() {
        let mut state = State::default();
        // volume 2: heard within 32 blocks; functional radius max(0, 32).
        state.add_wave(&packet(
            CBC_SHELL_BLAST_WAVE,
            wave(0.0, 2.0),
            [100.0, 0.0, 0.0],
        ));
        let player = Some(DVec3::ZERO);
        assert!(state.tick(true, player).is_empty());
        assert!(state.tick(true, player).is_empty());
        assert!(state.waves.is_empty());
        assert_eq!(state.shake.velocity, DVec3::ZERO);
        // No player: the particle goes at once.
        state.add_wave(&packet(
            CBC_SHELL_BLAST_WAVE,
            wave(0.0, 2.0),
            [1.0, 0.0, 0.0],
        ));
        assert!(state.tick(true, None).is_empty());
        assert!(state.waves.is_empty());
        // Other particles are not waves.
        let mut other = packet(CBC_SHELL_BLAST_WAVE, wave(0.0, 2.0), [0.0; 3]);
        other.particle.id = 5;
        assert!(!state.add_wave(&other));
    }

    #[test]
    fn shake_kicks_settle_and_respect_the_constraints() {
        let mut shake = ScreenShake::default();
        shake.add_effect(ShakeEffect::new(0, 45.0, 22.5, 22.5));
        let kick = shake.velocity;
        assert!(kick.x.abs() <= 45.0 * 1.3 * 0.5 && kick.y.abs() <= 22.5 * 1.3 * 0.5);
        shake.tick();
        assert!(shake.angles(0.0) != [0.0; 3]);
        shake.displacement = DVec3::new(100.0, -100.0, 100.0);
        shake.apply_constraints();
        // The third test reads the pitch again, which is now in range.
        let max = 45.0 * f64::from(SHAKE_INTENSITY);
        assert_eq!(shake.displacement, DVec3::new(max, -max, 100.0));
        // A delayed effect waits for its duration.
        let mut shake = ScreenShake::default();
        shake.add_effect(ShakeEffect::new(2, 10.0, 5.0, 5.0));
        shake.tick();
        assert_eq!(shake.velocity, DVec3::ZERO);
        shake.tick();
        assert!(shake.velocity != DVec3::ZERO || shake.displacement != DVec3::ZERO);
    }

    #[test]
    fn blast_sound_packet_waits_for_the_sound_to_travel() {
        let mut w = Writer::new();
        w.varint(BLAST_SOUND_PACKET);
        w.string("createbigcannons:fire_autocannon");
        w.bool(true);
        w.f32(32.0);
        w.varint(4);
        [64.0, 70.0, 0.0].into_iter().for_each(|v| w.f64(v));
        [3.0, 1.1, 2.0].into_iter().for_each(|v| w.f32(v));
        let sound = blast_sound(&w.data, Some(DVec3::new(0.0, 70.0, 0.0))).unwrap();
        // 64 m at 320 m/s: 0.2 s, four ticks.
        assert_eq!(sound.delay, 4);
        assert_eq!(
            (
                sound.event.as_str(),
                sound.source,
                sound.volume,
                sound.pitch
            ),
            ("createbigcannons:fire_autocannon", 4, 3.0, 1.1)
        );
        assert_eq!(sound.air_absorption, 2.0);
        assert_eq!(blast_sound(&w.data, None).unwrap().delay, 0);
        assert!(blast_sound(&w.data[..w.data.len() - 1], None).is_none());
        let mut other = w.data.clone();
        other[0] = 2;
        assert!(blast_sound(&other, None).is_none());
    }
}
