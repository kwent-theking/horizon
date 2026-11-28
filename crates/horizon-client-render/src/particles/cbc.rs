//! Create Big Cannons 5.11.7 client particles (MIT, Cannoneers of Create):
//! the firing plumes of big cannons, autocannons and drop mortars
//! (`effects/particles/plumes`) and the cannon smoke they leave
//! (`effects/particles/smoke/CannonSmokeParticle`), with the particle wind
//! (`ParticleWindHandler`). Defaults are CBC's client config defaults.
//!
//! CBC draws cannon smoke with its own shader (`cannon_smoke_particle.fsh`):
//! a sprite texel's red byte picks a column of
//! `cannon_smoke_particle_gradient.png` and the cannon's power picks its
//! row. [`gradient_sprites`] bakes those lookups into atlas sprites once, so
//! the generic particle pipeline draws the same texels.
use super::{Behaviour, Light, Particle, ParticleEngine, ParticleOptions, ParticleType, Size};
pub(super) mod impact;
use glam::DVec3;
use horizon_core::{
    noise::PerlinNoise,
    random::{LegacyRandom, PositionalRandom, RandomSource},
};
pub use impact::{CbcImpact, CbcShard};
use std::path::Path;

/// The sprite set of `CannonSmokeParticle`.
const CANNON_SMOKE: &str = "createbigcannons:cannon_smoke";
/// The atlas sprites its frames come from.
const CANNON_SMOKE_SPRITE: &str = "createbigcannons:particle/cannon_smoke_";
const GRADIENT: &str =
    "assets/createbigcannons/textures/particle/cannon_smoke_particle_gradient.png";
/// The sprite set of `TrailSmokeParticle` (vanilla's big smoke frames).
const TRAIL_SMOKE: &str = "createbigcannons:trail_smoke";
/// The sprite set of `ShellExplosionSmokeParticle`.
const SHELL_EXPLOSION_SMOKE: &str = "createbigcannons:shell_explosion_smoke";
/// The trail's lifetime under `CBCCfgMunitions.bigCannonTrailType`'s default,
/// `SHORT`.
const SHORT_TRAIL_LIFETIME: i32 = 100;
/// The lifetime of `ShellExplosionCloudParticle.TrailSubparticle`.
const SHELL_TRAIL_LIFETIME: i32 = 5;
/// `CBCCfgClient.maximumWindSpeed` (m/s) and `maxWindBearingChangeSpeed`.
const MAXIMUM_WIND_SPEED: f32 = 1.25;
const MAX_WIND_BEARING_CHANGE: f32 = 8.0;

/// A CBC particle's options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CbcParticle {
    Impact(CbcImpact),
    /// `BigCannonPlumeParticleData` (its lifetime field is never read).
    BigCannonPlume {
        size: f32,
        power: f32,
    },
    /// `AutocannonPlumeParticleData`.
    AutocannonPlume {
        scale: f32,
    },
    /// `DropMortarPlumeParticleData`.
    DropMortarPlume {
        scale: f32,
    },
    /// `CannonSmokeParticleData(power, size, lifetime, friction)`.
    CannonSmoke {
        power: f32,
        size: f32,
        lifetime: i32,
        friction: f32,
    },
    /// `TrailSmokeParticleData(lifetime)`: the smoke a big cannon's shell
    /// leaves behind it.
    TrailSmoke {
        lifetime: i32,
    },
    /// `ShellExplosionCloudParticleData`.
    ShellExplosionCloud {
        power: f32,
        plume: bool,
    },
    /// `ShellExplosionSmokeParticleData`.
    ShellExplosionSmoke {
        lifetime: i32,
        scale: f32,
    },
    ShrapnelCloud,
    FlakCloud,
    FluidCloud,
    ShrapnelSmoke {
        lifetime: i32,
    },
    FlakSmoke {
        lifetime: i32,
        scale: f32,
    },
    DebrisSmoke {
        scale: f32,
    },
    QuickFiringBreechSmoke,
    SmokeShellSmoke {
        scale: f32,
    },
    GasCloud {
        scale: f32,
        color: [f32; 3],
    },
    FallbackCannonSmoke {
        size: f32,
        lifetime: i32,
        friction: f32,
    },
}

/// The atlas name of a cannon smoke frame recoloured with gradient row `row`.
pub(crate) fn gradient_name(sprite: &str, row: u32) -> String {
    format!("{sprite}#gradient{row}")
}

/// Every cannon smoke frame of `images` recoloured with every row of CBC's
/// gradient, as `cannon_smoke_particle.fsh` does per texel: a texel under 0.1
/// alpha is discarded, any other takes the gradient's colour at column
/// `floor(red * 255)` (black past its width, as an out-of-range `texelFetch`
/// reads) and keeps its alpha.
pub(crate) fn gradient_sprites(
    root: &Path,
    images: &[(String, u32, u32, Vec<u8>)],
) -> Vec<(String, u32, u32, Vec<u8>)> {
    let frames: Vec<_> = images
        .iter()
        .filter(|(name, ..)| {
            name.strip_prefix(CANNON_SMOKE_SPRITE)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    if frames.is_empty() {
        return Vec::new();
    }
    let Ok((gw, gh, gradient)) = crate::hud::decode(&root.join(GRADIENT)) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(frames.len() * gh as usize);
    for row in 0..gh {
        for (name, w, h, pixels) in &frames {
            out.push((
                gradient_name(name, row),
                *w,
                *h,
                recolor(pixels, &gradient, gw, row),
            ));
        }
    }
    out
}

fn recolor(pixels: &[u8], gradient: &[u8], width: u32, row: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for p in pixels.chunks_exact(4) {
        if f32::from(p[3]) / 255.0 < 0.1 {
            out.extend([0; 4]);
            continue;
        }
        let column = u32::from(p[0]);
        let rgb = if column < width {
            let i = ((row * width + column) * 4) as usize;
            [gradient[i], gradient[i + 1], gradient[i + 2]]
        } else {
            [0; 3]
        };
        out.extend([rgb[0], rgb[1], rgb[2], p[3]]);
    }
    out
}

/// `Mth.wrapDegrees(float)`.
fn wrap_degrees(degrees: f32) -> f32 {
    let mut f = degrees % 360.0;
    if f >= 180.0 {
        f -= 360.0;
    }
    if f < -180.0 {
        f += 360.0;
    }
    f
}

/// `PerlinNoise.wrap`.
fn wrap(value: f64) -> f64 {
    value - ((value / 3.355_443_2e7 + 0.5).floor()) * 3.355_443_2e7
}

/// 1.21.1's `PerlinNoise.create(random, firstOctave, amplitudes)` with the
/// positional octave seeds (`octave_<n>`), summing `ImprovedNoise` octaves.
struct OctaveNoise {
    levels: Vec<Option<PerlinNoise>>,
    amplitudes: Vec<f64>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
}

impl OctaveNoise {
    fn create(random: &mut LegacyRandom, first_octave: i32, amplitudes: &[f64]) -> Self {
        let positional = random.fork_positional();
        let levels = amplitudes
            .iter()
            .enumerate()
            .map(|(k, &amplitude)| {
                (amplitude != 0.0).then(|| {
                    let octave = first_octave + k as i32;
                    PerlinNoise::new(&mut positional.from_hash_of(&format!("octave_{octave}")))
                })
            })
            .collect();
        let count = amplitudes.len() as i32;
        Self {
            levels,
            amplitudes: amplitudes.to_vec(),
            lowest_freq_input_factor: 2f64.powi(first_octave),
            lowest_freq_value_factor: 2f64.powi(count - 1) / (2f64.powi(count) - 1.0),
        }
    }

    fn value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut total = 0.0;
        let mut input = self.lowest_freq_input_factor;
        let mut weight = self.lowest_freq_value_factor;
        for (level, amplitude) in self.levels.iter().zip(&self.amplitudes) {
            if let Some(noise) = level {
                let sample = noise.get3(wrap(x * input), wrap(y * input), wrap(z * input));
                total += amplitude * f64::from(sample) * weight;
            }
            input *= 2.0;
            weight /= 2.0;
        }
        total
    }
}

/// `ParticleWindHandler`: a bearing and speed drifting along two noises.
pub(super) struct Wind {
    bearing_noise: OctaveNoise,
    speed_noise: OctaveNoise,
    delta_bearing: f32,
    bearing: f32,
    time: f64,
    old_force: DVec3,
    force: DVec3,
}

impl Wind {
    fn noise(seed: i64) -> OctaveNoise {
        OctaveNoise::create(
            &mut LegacyRandom::new(seed),
            6,
            &[1.0, 1.0, 0.1, 0.1, 0.1, 0.1],
        )
    }

    pub(super) fn new() -> Self {
        Self {
            bearing_noise: Self::noise(1024),
            speed_noise: Self::noise(1026),
            delta_bearing: 0.0,
            bearing: 0.0,
            time: 0.0,
            old_force: DVec3::ZERO,
            force: DVec3::ZERO,
        }
    }

    /// `refreshNoise`: a new level's wind.
    pub(super) fn refresh(&mut self, seed: i64) {
        self.bearing_noise = Self::noise(seed);
        self.speed_noise = Self::noise(seed.wrapping_add(2));
        self.time = 0.0;
        self.update();
        self.bearing = 0.0;
        self.old_force = self.force;
    }

    /// `updateWind`, once a client tick.
    pub(super) fn update(&mut self) {
        self.bearing = wrap_degrees(self.bearing + self.delta_bearing);
        self.old_force = self.force;
        self.time += 0.0005;
        self.delta_bearing =
            self.bearing_noise.value(self.time, 0.0, 0.0) as f32 * MAX_WIND_BEARING_CHANGE;
        let speed = self.speed_noise.value(0.0, self.time, 0.0) as f32 * 0.5 + 0.5;
        let radians =
            f64::from((self.bearing + self.delta_bearing) * (std::f32::consts::PI / 180.0));
        self.force = DVec3::new(radians.cos(), 0.0, radians.sin()) * f64::from(speed);
    }

    /// `getWindForce(partialTick)`, in blocks per tick.
    pub(super) fn force(&self, partial: f64) -> DVec3 {
        let maximum = f64::from(MAXIMUM_WIND_SPEED) * 0.05;
        if maximum < 1e-2 {
            return DVec3::ZERO;
        }
        self.old_force.lerp(self.force, partial) * maximum
    }
}

/// `Vec3.normalize`: zero below 1e-5.
fn normalize(v: DVec3) -> DVec3 {
    let length = v.length();
    if length < f64::from(1.0e-5f32) {
        DVec3::ZERO
    } else {
        v / length
    }
}

/// The plume's right and up axes about its direction.
fn plume_axes(direction: DVec3) -> (DVec3, DVec3) {
    let mut right = normalize(direction.cross(DVec3::Y));
    if right.length_squared() < 1e-6 {
        right = DVec3::X;
    }
    (right, normalize(direction.cross(right)))
}

/// `(random.nextDouble() - random.nextDouble()) * scale` along an axis.
fn spread(random: &mut LegacyRandom, axis: DVec3, scale: f64) -> DVec3 {
    let a = random.next_double();
    let b = random.next_double();
    axis * ((a - b) * scale)
}

/// `ShellExplosionCloudParticle.TrailSubparticle`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShellTrail {
    displacement: DVec3,
    velocity: DVec3,
    drag: f64,
    gravity: f64,
}

impl ShellTrail {
    fn displacement_at(self, ticks: i32) -> DVec3 {
        if ticks <= 0 {
            return self.displacement;
        }
        let ticks = f64::from(ticks);
        if self.drag == 1.0 {
            return self.displacement + self.velocity * ticks;
        }
        let geo = (1.0 - horizon_core::jmath::pow(self.drag, ticks)) / (1.0 - self.drag);
        self.displacement + self.velocity * geo + DVec3::new(0.0, self.gravity * ticks, 0.0)
    }
}

fn shell_jitter(random: &mut LegacyRandom) -> DVec3 {
    let mut next = || random.next_double() - random.next_double();
    DVec3::new(next(), next(), next())
}

/// `ShellExplosionCloudParticle`'s initial trails, using the particle's own random.
fn shell_trails(
    mut random: impl std::borrow::BorrowMut<LegacyRandom>,
    level: u8,
    power: f32,
    plume: bool,
) -> impl Iterator<Item = ShellTrail> {
    let count = match level {
        0 => 12 + random.borrow_mut().next_int_bound(6).unwrap_or(0),
        1 => 6 + random.borrow_mut().next_int_bound(3).unwrap_or(0),
        _ => 4,
    };
    (0..count).map(move |_| {
        let random = random.borrow_mut();
        let displacement = shell_jitter(random) * 2.5;
        let mut velocity = shell_jitter(random);
        if plume {
            velocity.y += 2.0;
        }
        ShellTrail {
            displacement,
            velocity: velocity * (f64::from(power) * 0.35),
            drag: 0.85,
            gravity: if plume { -0.5 } else { -0.1 },
        }
    })
}

fn replay_shell_trails(
    seed: i64,
    level: u8,
    power: f32,
    plume: bool,
) -> impl Iterator<Item = ShellTrail> {
    let mut random = LegacyRandom::new(seed);
    // Particle::base_at chooses a lifetime before the cloud starts its draws.
    random.next_float();
    shell_trails(random, level, power, plume)
}

fn flak_trail(random: &mut LegacyRandom) -> ShellTrail {
    let velocity = shell_jitter(random) * 2.0;
    ShellTrail {
        displacement: shell_jitter(random) * 0.25,
        velocity,
        drag: 0.5,
        gravity: 0.0,
    }
}

const SHOW_MORTAR_STONE_CLOUDS: bool = true;
const USE_SHADER_COMPATIBLE_GRAPHICS: bool = false;

impl ParticleEngine {
    /// The particle a CBC provider makes.
    pub(super) fn make_cbc(
        &mut self,
        cbc: CbcParticle,
        p: [f64; 3],
        v: [f64; 3],
        seed: i64,
    ) -> Option<Particle> {
        if USE_SHADER_COMPATIBLE_GRAPHICS
            && let CbcParticle::CannonSmoke {
                size,
                lifetime,
                friction,
                ..
            } = cbc
        {
            let fallback = CbcParticle::FallbackCannonSmoke {
                size,
                lifetime,
                friction,
            };
            self.add_with(&ParticleOptions::cbc(fallback), p, v, true, false);
            return None;
        }
        let mut local = LegacyRandom::new(seed);
        let mut q = match cbc {
            CbcParticle::Impact(_) => unreachable!("handled with the full ParticleOptions"),
            CbcParticle::BigCannonPlume { size, power } => {
                // NoRenderParticle(level, x, y, z), then setParticleSpeed.
                let mut q = Particle::base_at(&mut local, ParticleType::Poof, p);
                q.gravity = 0.0;
                q.friction = 0.9;
                let f = f64::from(power / 4.0);
                q.velocity = v.map(|c| c * f);
                q.behaviour = Behaviour::CbcBigCannonPlume {
                    direction: v,
                    size,
                    power,
                };
                q.no_render = true;
                q
            }
            CbcParticle::AutocannonPlume { scale } => {
                let mut q = Particle::base_at(&mut local, ParticleType::Poof, p);
                q.lifetime = 5;
                q.behaviour = Behaviour::CbcAutocannonPlume {
                    direction: v,
                    scale,
                    visible: self.particle_level == 0,
                };
                q.no_render = true;
                q
            }
            CbcParticle::DropMortarPlume { scale } => {
                let mut q = Particle::base_at(&mut local, ParticleType::Poof, p);
                q.lifetime = 0;
                q.behaviour = Behaviour::CbcDropMortarPlume {
                    direction: v,
                    scale,
                    visible: self.particle_level == 0,
                };
                q.no_render = true;
                q
            }
            CbcParticle::CannonSmoke {
                size,
                lifetime,
                friction,
                ..
            }
            | CbcParticle::FallbackCannonSmoke {
                size,
                lifetime,
                friction,
            } => {
                let set = if let CbcParticle::CannonSmoke { power, .. } = cbc {
                    let last = self.cbc_smoke_sets.len().checked_sub(1)?;
                    let row = ((power.floor() as i32).saturating_sub(1)).clamp(0, last as i32);
                    self.cbc_smoke_sets[row as usize]
                } else {
                    *self
                        .modded_sets
                        .get("createbigcannons:cannon_smoke_fallback")?
                };
                // BaseAshSmokeParticle(level, x, y, z, 0.1, 0.1, 0.1, xa, ya, za,
                // 1, sprites, 1, 8, -0.05, true): Particle(x, y, z, 0, 0, 0)
                // scaled by a tenth, then the provider's size, friction and lifetime.
                let mut q = Particle::moving(&mut local, ParticleType::Poof, p, [0.0; 3]);
                q.speed_up_when_y_blocked = true;
                q.gravity = -0.05;
                for (c, add) in q.velocity.iter_mut().zip(v) {
                    *c = *c * f64::from(0.1f32) + add;
                }
                let _color = self.level_random.next_float();
                let _lifetime = self.level_random.next_float();
                q.rgba = [1.0; 4];
                q.size = size;
                q.friction = friction;
                q.lifetime = lifetime;
                q.set = Some(set);
                q.size_curve = Size::CbcCannonSmoke;
                q.light = Light::CbcCannonSmoke;
                q.translucent = Some(true);
                q.behaviour = Behaviour::CbcCannonSmoke {
                    wind: self.cbc_wind.force(0.0).to_array(),
                };
                q
            }

            CbcParticle::ShellExplosionCloud { power, plume } => {
                let mut q = Particle::base_at(&mut local, ParticleType::Poof, p);
                q.velocity = v;
                q.lifetime = 10;
                q.no_render = true;
                let trail_level = self.particle_level;
                // Advance the live random past the constructor's trails.
                shell_trails(&mut local, trail_level, power, plume).for_each(drop);
                q.behaviour = Behaviour::CbcShellExplosionCloud {
                    power,
                    plume,
                    seed,
                    trail_level,
                };
                q
            }
            CbcParticle::ShrapnelCloud | CbcParticle::FlakCloud | CbcParticle::FluidCloud => {
                let flak = matches!(cbc, CbcParticle::FlakCloud);
                let mut q = Particle::base_moving(&mut local, ParticleType::Poof, p, v);
                let trails = if flak {
                    match self.particle_level {
                        0 => 12 + self.level_random.next_int_bound(5).unwrap_or(0),
                        1 => 4 + self.level_random.next_int_bound(3).unwrap_or(0),
                        _ => 0,
                    }
                } else {
                    0
                };
                (0..trails).for_each(|_| {
                    flak_trail(&mut local);
                });
                if flak {
                    q.velocity = v;
                }
                q.set_size(0.25, 0.25);
                q.lifetime = if flak { 30 } else { 1 };
                q.no_render = true;
                q.behaviour = Behaviour::CbcFragmentCloud { flak, seed, trails };
                q
            }
            CbcParticle::QuickFiringBreechSmoke => {
                let mut q = Particle::base_moving(&mut local, ParticleType::Poof, p, [0.0; 3]);
                q.velocity = [0.0; 3];
                q.lifetime = 2;
                q.no_render = true;
                q.behaviour = Behaviour::CbcBreechSmoke { direction: v };
                q
            }
            CbcParticle::SmokeShellSmoke { scale } | CbcParticle::GasCloud { scale, .. } => {
                let (name, color, wind, gas) = match cbc {
                    CbcParticle::GasCloud { color, .. } => {
                        ("createbigcannons:gas_cloud", color, DVec3::ZERO, true)
                    }
                    _ => (
                        "createbigcannons:smoke_shell_smoke",
                        [0.0; 3],
                        self.cbc_wind.force(0.0),
                        false,
                    ),
                };
                let set = *self.modded_sets.get(name)?;
                let mut q = Particle::moving(&mut local, ParticleType::Poof, p, [0.0; 3]);
                for (speed, input) in q.velocity.iter_mut().zip(v) {
                    *speed = *speed * f64::from(0.1f32) + input;
                }
                self.level_random.next_float(); // BaseAshSmokeParticle's colour
                self.level_random.next_float(); // BaseAshSmokeParticle's lifetime
                q.rgba = [color[0], color[1], color[2], 1.0];
                q.size *= 0.75;
                q.size_curve = Size::GrowIn;
                q.gravity = 0.0;
                q.friction = 0.95;
                q.speed_up_when_y_blocked = true;
                q.lifetime = 200;
                q.set = Some(set);
                q.translucent = Some(true);
                q.behaviour = Behaviour::CbcEmitterSmoke {
                    wind: wind.to_array(),
                    max_size: scale,
                    growth: 0.5,
                    gas,
                };
                self.sprite_from_age(&mut q);
                q
            }
            CbcParticle::ShellExplosionSmoke { lifetime, .. }
            | CbcParticle::ShrapnelSmoke { lifetime }
            | CbcParticle::FlakSmoke { lifetime, .. } => {
                let (name, scale, width, gravity) = match cbc {
                    CbcParticle::ShrapnelSmoke { .. } => {
                        ("createbigcannons:shrapnel_smoke", 6.0, 0.35, 3.0e-6)
                    }
                    CbcParticle::FlakSmoke { scale, .. } => {
                        ("createbigcannons:flak_smoke", scale, 0.25, 3.0e-6)
                    }
                    CbcParticle::ShellExplosionSmoke { scale, .. } => {
                        (SHELL_EXPLOSION_SMOKE, scale, 0.25, 0.05)
                    }
                    _ => unreachable!(),
                };
                let set = *self.modded_sets.get(name)?;
                // BaseAshSmokeParticle's initial colour and lifetime are replaced
                // by ShellExplosionSmokeParticle and its provider.
                let mut q = Particle::moving(&mut local, ParticleType::Poof, p, [0.0; 3]);
                let _color = self.level_random.next_float();
                let _lifetime = self.level_random.next_float();
                q.speed_up_when_y_blocked = true;
                q.gravity = gravity;
                q.friction = 0.8;
                q.velocity = [
                    v[0],
                    v[1] + f64::from(self.level_random.next_float() / 500.0),
                    v[2],
                ];
                q.size *= 0.75;
                q.scale(scale);
                q.set_size(width, width);
                q.lifetime = lifetime;
                q.set = Some(set);
                q.size_curve = Size::CbcShellExplosionSmoke;
                q.light = Light::CbcShellExplosionSmoke;
                q.translucent = Some(true);
                q.behaviour = Behaviour::CbcShellExplosionSmoke {
                    wind: self.cbc_wind.force(0.0).to_array(),
                };
                q
            }
            CbcParticle::TrailSmoke { .. } | CbcParticle::DebrisSmoke { .. } => {
                let (name, lifetime, scale, alpha, gravity, friction) = match cbc {
                    CbcParticle::DebrisSmoke { scale } => (
                        "createbigcannons:debris_smoke",
                        120,
                        scale,
                        0.8,
                        0.0025,
                        0.92,
                    ),
                    CbcParticle::TrailSmoke { lifetime } => {
                        (TRAIL_SMOKE, lifetime, 1.0, 0.35, 3.0e-6, 1.0)
                    }
                    _ => unreachable!(),
                };
                let set = *self.modded_sets.get(name)?;
                // TrailSmokeParticle(level, x, y, z, xd, yd, zd) on
                // TextureSheetParticle(level, x, y, z).
                let mut q = Particle::at(&mut local, ParticleType::Poof, p);
                q.gravity = gravity;
                q.friction = friction;
                q.velocity = [v[0], v[1] + local.next_double() / 500.0, v[2]];
                // The provider: scale(3), setSize(0.25, 0.25), setAlpha(0.35),
                // setLifetime and pickSprite.
                q.scale(3.0 * scale);
                q.set_size(0.25 * scale, 0.25 * scale);
                q.rgba[3] = alpha;
                q.lifetime = lifetime;
                let sprites = &self.set_sprites[set as usize];
                let index = local.next_int_bound(sprites.len() as i32).unwrap_or(0);
                q.sprite = super::Sprite::Index(sprites[index as usize]);
                q.translucent = Some(true);
                q.behaviour = Behaviour::CbcTrailSmoke;
                q
            }
        };
        q.random = local;
        match cbc {
            CbcParticle::CannonSmoke { .. } | CbcParticle::FallbackCannonSmoke { .. } => {
                self.cbc_smoke_sprite(&mut q, 1.5)
            }
            CbcParticle::ShellExplosionSmoke { .. }
            | CbcParticle::ShrapnelSmoke { .. }
            | CbcParticle::FlakSmoke { .. } => self.cbc_smoke_sprite(&mut q, 4.0),
            _ => {}
        }
        Some(q)
    }

    /// CBC smoke's `setSpriteFromAge`: frames run fast, then settle.
    fn cbc_smoke_sprite(&self, p: &mut Particle, speed: f32) {
        if p.removed {
            return;
        }
        let Some(sprites) = p.set.and_then(|set| self.set_sprites.get(set as usize)) else {
            return;
        };
        let lifetime = p.lifetime;
        let progress = (p.age as f32 / lifetime as f32 * speed).clamp(0.0, 1.0);
        let inv = 1.0 - progress;
        let sprite_progress = 1.0 - inv * inv * inv * inv;
        // SpriteSet.get(age, lifetime): age * (size - 1) / lifetime.
        let age = (sprite_progress * lifetime as f32).floor() as i64;
        let index = if lifetime == 0 {
            0
        } else {
            (age * (sprites.len() as i64 - 1) / i64::from(lifetime))
                .clamp(0, sprites.len() as i64 - 1)
        };
        p.sprite = super::Sprite::Index(sprites[index as usize]);
    }

    /// The recoloured cannon smoke sets, one per gradient row present.
    pub(super) fn load_cbc_sprites(&mut self) {
        self.cbc_smoke_sets.clear();
        let Some(&base) = self.modded_sets.get(CANNON_SMOKE) else {
            return;
        };
        let frames: Vec<String> = self.set_sprites[base as usize]
            .iter()
            .map(|&i| self.names[i as usize].clone())
            .collect();
        for row in 0.. {
            let names: Vec<String> = frames.iter().map(|f| gradient_name(f, row)).collect();
            if !names.iter().all(|n| self.atlas.contains_key(n)) {
                break;
            }
            let sprites = names.iter().map(|n| self.intern(n)).collect();
            self.cbc_smoke_sets.push(self.set_sprites.len() as u32);
            self.set_sprites.push(sprites);
        }
    }

    fn add_cbc(&mut self, options: &ParticleOptions, at: DVec3, velocity: DVec3, force: bool) {
        self.add_with(options, at.to_array(), velocity.to_array(), force, false);
    }

    /// `AbstractBigCannonProjectile.tick` with the default `SHORT` trail: a
    /// flying shell leaves ten always-visible smoke puffs between its last
    /// position and this one.
    pub fn cbc_shell_trail(&mut self, from: [f64; 3], to: [f64; 3]) {
        self.cbc_projectile_trail(from, to, SHORT_TRAIL_LIFETIME);
    }

    pub fn cbc_autocannon_trail(&mut self, from: [f64; 3], to: [f64; 3]) {
        self.cbc_projectile_trail(from, to, 50);
    }

    fn cbc_projectile_trail(&mut self, from: [f64; 3], to: [f64; 3], lifetime: i32) {
        let options = ParticleOptions::cbc(CbcParticle::TrailSmoke { lifetime });
        for i in 0..10 {
            // `double partial = i * 0.1f`, then Mth.lerp.
            let partial = f64::from(i as f32 * 0.1);
            let at = std::array::from_fn(|k| from[k] + partial * (to[k] - from[k]));
            let r = &mut self.level_random;
            let spread =
                [r.next_double(), r.next_double(), r.next_double()].map(|d| d * 0.004 - 0.002);
            self.add_with(&options, at, spread, true, true);
        }
    }

    /// `BigCannonPlumeParticle.tick`.
    fn tick_big_cannon_plume(&mut self, p: &mut Particle, direction: DVec3, size: f32, power: f32) {
        let status = self.particle_level.min(2);
        let (right, up) = plume_axes(direction);
        let progress = if p.lifetime == 0 {
            1.0
        } else {
            f64::from((p.age as f32 / p.lifetime as f32).clamp(0.0, 1.0))
        };
        let at = DVec3::from_array(p.position);
        let small_scale = size * 0.25;
        let count_scale = match status {
            0 => 5.0,
            1 => 3.0,
            _ => 1.0,
        };
        let count = 3.min((small_scale * count_scale).ceil() as i32);
        for _ in 0..count {
            let r = &mut p.random;
            let dir_scale = 0.3 * progress + 0.8 + r.next_double() * 0.25;
            let perp = f64::from(small_scale * 0.25);
            let velocity = direction * dir_scale + spread(r, right, perp) + spread(r, up, perp);
            let lifetime = 30
                + r.next_int_bound(10).unwrap_or(0)
                + (10.0 * progress).ceil() as i32
                + (power * power).ceil() as i32 / 2;
            let smoke = ParticleOptions::cbc(CbcParticle::CannonSmoke {
                power,
                size: small_scale,
                lifetime,
                friction: 0.95,
            });
            self.add_cbc(&smoke, at, velocity, true);
        }
        // showExtraSmoke: cloud puffs the first tick.
        if p.age == 0 && status == 0 {
            let scale = f64::from(small_scale * 0.25);
            let count = (small_scale * 10.0).floor() as i32;
            let cloud = ParticleOptions::simple(ParticleType::Cloud);
            for _ in 0..count {
                let r = &mut p.random;
                let velocity = direction * 0.5 + spread(r, right, scale) + spread(r, up, scale);
                self.add_cbc(&cloud, at, velocity, true);
            }
        }
        // showExtraFlames: flames the first five ticks.
        if p.age < 5 && status == 0 {
            let scale = f64::from(0.05f32);
            let count = small_scale * 5.0;
            let flame = ParticleOptions::simple(ParticleType::Flame);
            let mut i = 0;
            while (i as f32) < count {
                let r = &mut p.random;
                let along = 0.05 + 0.2 * r.next_double();
                let velocity = direction * along + spread(r, right, scale) + spread(r, up, scale);
                self.add_cbc(&flame, at, velocity, false);
                i += 1;
            }
        }
    }

    /// `AutocannonPlumeParticle.tick`.
    fn tick_autocannon_plume(&mut self, p: &mut Particle, direction: DVec3, scale: f32) {
        let (right, up) = plume_axes(direction);
        let at = DVec3::from_array(p.position);
        let scale = f64::from(scale);
        let cloud = ParticleOptions::simple(ParticleType::Cloud);
        for _ in 0..(scale * 2.0).ceil() as i32 {
            let r = &mut p.random;
            let dir_scale = scale * (0.05 + 0.01 * r.next_double());
            let perp = scale * f64::from(0.05f32);
            let velocity = direction * dir_scale
                + spread(r, right, perp)
                + spread(r, up, perp)
                + DVec3::new(0.0, 0.02, 0.0);
            self.add_cbc(&cloud, at, velocity, true);
        }
        if p.age == 0 {
            let flame = ParticleOptions::simple(ParticleType::Flame);
            for _ in 0..5.max((scale * 8.0).ceil() as i32) {
                let dir_scale = scale * (0.05 + 0.01 * p.random.next_double());
                self.add_cbc(&flame, at, direction * dir_scale, true);
            }
        }
    }

    /// `DropMortarPlumeParticle.tick`.
    fn tick_drop_mortar_plume(&mut self, p: &mut Particle, direction: DVec3, scale: f32) {
        let (right, up) = plume_axes(direction);
        let at = DVec3::from_array(p.position);
        let scale = f64::from(scale);
        let cloud = ParticleOptions::simple(ParticleType::Cloud);
        for _ in 0..(scale * 20.0).ceil() as i32 {
            let r = &mut p.random;
            let dir_scale = scale * (0.0125 + 0.05 * r.next_double());
            let perp = scale * f64::from(0.125f32);
            let velocity = direction * dir_scale
                + spread(r, right, perp)
                + spread(r, up, perp)
                + DVec3::new(0.0, 0.025, 0.0);
            self.add_cbc(&cloud, at, velocity, true);
        }
    }

    /// The primary plumes of `ShellExplosionCloudParticle.tick`.
    fn emit_shell_plumes(&mut self, p: &mut Particle, power: f32, plume: bool, at: DVec3) {
        let plumes = match self.particle_level {
            0 => 20,
            1 => 10,
            _ => 5,
        };
        let vel_scale = f64::from(power) * 0.25 * f64::from(1.0 - p.age as f32 / 5.0);
        let disp_scale = f64::from(power) * 0.35;
        for _ in 0..=plumes {
            let mut offset = shell_jitter(&mut p.random);
            if plume {
                offset.y = offset.y.abs();
            }
            let pos = DVec3::new(
                at.x + offset.x * disp_scale,
                offset.y * disp_scale + (at.y + 0.5),
                at.z + offset.z * disp_scale,
            );
            let mut velocity = shell_jitter(&mut p.random);
            if plume {
                velocity.y += 1.2;
            }
            let lifetime = 160 + p.random.next_int_bound(80).unwrap_or(0);
            self.add_cbc(
                &ParticleOptions::cbc(CbcParticle::ShellExplosionSmoke {
                    lifetime,
                    scale: power * 2.0,
                }),
                pos,
                velocity * vel_scale,
                true,
            );
        }
    }

    /// `ShellExplosionCloudParticle.tick`: primary plumes, then the secondary trails.
    fn emit_shell_explosion(
        &mut self,
        p: &mut Particle,
        power: f32,
        plume: bool,
        seed: i64,
        trail_level: u8,
    ) {
        let at = DVec3::from_array(p.position);
        if p.age < 5 {
            self.emit_shell_plumes(p, power, plume, at);
        }

        if p.age > SHELL_TRAIL_LIFETIME {
            return;
        }
        let steps = if self.particle_level == 0 { 2 } else { 1 };
        for trail in replay_shell_trails(seed, trail_level, power, plume) {
            let origin = trail.displacement_at(p.age);
            let next = trail.displacement_at(p.age + 1);
            for i in 0..=steps {
                let pos = origin + (next - origin) * (f64::from(i) / f64::from(steps)) + at;
                let pos = pos + shell_jitter(&mut p.random) * 0.1;
                let velocity = shell_jitter(&mut p.random) * 0.125;
                let lifetime = 30 + p.random.next_int_bound(15).unwrap_or(0);
                self.add_cbc(
                    &ParticleOptions::cbc(CbcParticle::ShellExplosionSmoke {
                        lifetime,
                        scale: power * 0.75,
                    }),
                    pos,
                    velocity,
                    true,
                );
            }
        }
    }

    /// CBC's explosions use the same level random for pitch and particle emission.
    pub fn cbc_level_random(&mut self) -> &mut LegacyRandom {
        &mut self.level_random
    }

    /// MortarStoneExplosion uses the level random for every position and velocity.
    pub fn cbc_mortar_stone_cloud(&mut self, center: [f64; 3], scale: f32) {
        if !SHOW_MORTAR_STONE_CLOUDS {
            return;
        }
        let options = ParticleOptions::cbc(CbcParticle::DebrisSmoke { scale });
        let center = DVec3::from_array(center);
        for _ in 0..50 {
            let at = center + shell_jitter(&mut self.level_random) * f64::from(0.1f32);
            let mut velocity = shell_jitter(&mut self.level_random);
            velocity.y += 0.5;
            self.add_cbc(&options, at, velocity * f64::from(0.15 * scale), true);
        }
    }

    fn emit_fragment_cloud(&mut self, p: &mut Particle, flak: bool, seed: i64, trails: i32) {
        let at = DVec3::from_array(p.position);
        let status = self.particle_level.min(2) as usize;
        if !flak || p.age == 0 {
            let count = if flak { [30, 20, 10] } else { [20, 10, 5] }[status];
            for _ in 0..count {
                let velocity = shell_jitter(&mut p.random) * if flak { 0.5 } else { 0.7 };
                let pos = at + shell_jitter(&mut p.random) * 0.25;
                let age = p.random.next_int_bound(80).unwrap_or(0);
                let smoke = if flak {
                    CbcParticle::FlakSmoke {
                        lifetime: 45 + age,
                        scale: 3.0,
                    }
                } else {
                    CbcParticle::ShrapnelSmoke { lifetime: 85 + age }
                };
                self.add_cbc(&ParticleOptions::cbc(smoke), pos, velocity, true);
            }
            if status == 0 {
                let flame = ParticleOptions::simple(ParticleType::Flame);
                for _ in 0..20 {
                    let velocity = shell_jitter(&mut p.random) * if flak { 0.3 } else { 0.45 };
                    let pos = at + shell_jitter(&mut p.random) * 0.25;
                    self.add_cbc(&flame, pos, velocity, true);
                }
                self.add_cbc(
                    &ParticleOptions::simple(ParticleType::Explosion),
                    at,
                    DVec3::ZERO,
                    false,
                );
            }
        }
        if trails == 0 || status == 2 || p.age > 5 {
            return;
        }
        let mut replay = LegacyRandom::new(seed);
        Particle::base_moving(&mut replay, ParticleType::Poof, [0.0; 3], [0.0; 3]);
        let steps = if status == 0 { 5 } else { 2 };
        for _ in 0..trails {
            let trail = flak_trail(&mut replay);
            let from = trail.displacement_at(p.age);
            let to = trail.displacement_at(p.age + 1);
            for i in 0..=steps {
                let pos = from + (to - from) * (f64::from(i) / f64::from(steps)) + at;
                let velocity = shell_jitter(&mut p.random) * 0.01;
                let pos = pos + shell_jitter(&mut p.random) * 0.25;
                let lifetime = 20 + p.random.next_int_bound(5).unwrap_or(0);
                let smoke = ParticleOptions::cbc(CbcParticle::FlakSmoke {
                    lifetime,
                    scale: 1.0,
                });
                self.add_cbc(&smoke, pos, velocity, true);
            }
        }
    }

    /// A CBC particle's tick; `false` for any other behaviour.
    pub(super) fn tick_cbc(&mut self, p: &mut Particle, level: &super::ParticleLevel) -> bool {
        match p.behaviour {
            Behaviour::CbcBigCannonPlume {
                direction,
                size,
                power,
            } => {
                self.tick_big_cannon_plume(p, DVec3::from_array(direction), size, power);
                p.base_tick(level);
            }
            Behaviour::CbcAutocannonPlume {
                direction,
                scale,
                visible,
            } => {
                if !visible {
                    p.removed = true;
                    return true;
                }
                self.tick_autocannon_plume(p, DVec3::from_array(direction), scale);
                p.base_tick(level);
            }
            Behaviour::CbcDropMortarPlume {
                direction,
                scale,
                visible,
            } => {
                if !visible {
                    p.removed = true;
                    return true;
                }
                self.tick_drop_mortar_plume(p, DVec3::from_array(direction), scale);
                p.base_tick(level);
            }
            Behaviour::CbcCannonSmoke { wind } => {
                // BaseAshSmokeParticle.tick, then the wind and the fade.
                p.base_tick(level);
                self.cbc_smoke_sprite(p, 1.5);
                let f = if p.on_ground { 1.0 } else { 0.5 };
                p.move_by(level, [wind[0] * f, wind[1], wind[2] * f]);
                let progress = (p.age as f32 / p.lifetime as f32).clamp(0.0, 1.0);
                p.rgba[3] = if p.lifetime == 0 || p.age >= p.lifetime {
                    0.0
                } else {
                    1.0 - progress * progress
                };
            }

            Behaviour::CbcShellExplosionCloud {
                power,
                plume,
                seed,
                trail_level,
            } => {
                self.emit_shell_explosion(p, power, plume, seed, trail_level);
                p.base_tick(level);
            }
            Behaviour::CbcShellExplosionSmoke { wind } => {
                p.base_tick(level);
                self.cbc_smoke_sprite(p, 4.0);
                let f = if p.on_ground { 1.0 } else { 0.5 };
                p.move_by(level, [wind[0] * f, wind[1], wind[2] * f]);
                p.rgba[3] = p.shell_explosion_smoke_alpha();
            }
            Behaviour::CbcTrailSmoke => tick_trail_smoke(p, level),
            Behaviour::CbcFragmentCloud { flak, seed, trails } => {
                self.emit_fragment_cloud(p, flak, seed, trails);
                p.base_tick(level);
            }
            Behaviour::CbcBreechSmoke { direction } => {
                let at = DVec3::from_array(p.position);
                let direction = DVec3::from_array(direction);
                let poof = ParticleOptions::simple(ParticleType::Poof);
                for _ in 0..[10, 2, 0][self.particle_level.min(2) as usize] {
                    let velocity = direction + shell_jitter(&mut p.random) * 0.005;
                    self.add_cbc(&poof, at, velocity, false);
                }
                p.base_tick(level);
            }
            Behaviour::CbcEmitterSmoke {
                wind,
                max_size,
                growth,
                gas,
            } => {
                p.base_tick(level);
                self.sprite_from_age(p);
                let f = if p.on_ground { 1.0 } else { 0.5 };
                p.move_by(level, [wind[0] * f, wind[1], wind[2] * f]);
                if p.age > p.lifetime - 10 {
                    p.size = (p.size - max_size / 10.0).max(0.0);
                } else if p.size < max_size {
                    p.size *= 1.0 + growth;
                }
                if !gas {
                    let progress = (p.age as f32 / 60.0).min(1.0);
                    p.rgba[..3].fill(0.85 + progress * (0.75 - 0.85));
                }
                p.behaviour = Behaviour::CbcEmitterSmoke {
                    wind,
                    max_size,
                    growth: growth * 0.95,
                    gas,
                };
            }
            _ => return false,
        }
        true
    }
}

/// `TrailSmokeParticle.tick`: a slow random drift, fading over its last three
/// seconds.
fn tick_trail_smoke(p: &mut Particle, level: &super::ParticleLevel) {
    p.previous = p.position;
    let age = p.age;
    p.age += 1;
    if age >= p.lifetime || p.rgba[3] <= 0.0 {
        p.removed = true;
        return;
    }
    let friction = f64::from(p.friction);
    for c in &mut p.velocity {
        *c *= friction;
    }
    for axis in [0, 2] {
        let r = &mut p.random;
        let drift = r.next_float() / 5000.0;
        let sign = if r.next_boolean() { 1.0 } else { -1.0 };
        p.velocity[axis] += f64::from(drift * sign);
    }
    p.velocity[1] -= f64::from(p.gravity);
    p.move_by(level, p.velocity);
    if p.age >= p.lifetime - 60 && p.rgba[3] > 0.01 {
        p.rgba[3] -= 0.015;
    }
}

impl Particle {
    /// `CannonSmokeParticle.getQuadSize`: grows from 0.9 to 4.1 times.
    pub(super) fn cbc_smoke_size(&self, partial: f32) -> f32 {
        let f = (self.age as f32 + partial) / self.lifetime as f32 * 32.0;
        self.size * (0.9 + f * (1.0 - 0.9))
    }

    /// `ShellExplosionSmokeParticle.getQuadSize`.
    pub(super) fn shell_explosion_smoke_size(&self, partial: f32) -> f32 {
        let f = (self.age as f32 + partial) / self.lifetime as f32 * 32.0;
        self.size * (0.95 + f * (1.0 - 0.95))
    }

    /// `ShellExplosionSmokeParticle.tick`'s cubic fade.
    fn shell_explosion_smoke_alpha(&self) -> f32 {
        if self.lifetime == 0 || self.age >= self.lifetime {
            return 0.0;
        }
        let progress = (self.age as f32 / self.lifetime as f32).clamp(0.0, 1.0);
        1.0 - progress * progress * progress
    }

    /// CBC smoke's `getLightColor`: glowing at first, as fire.
    pub(super) fn cbc_smoke_block_light(&self, partial: f32, block: u32, speed: f32) -> u32 {
        let progress =
            1.0 - ((self.age as f32 + partial) / self.lifetime as f32 * speed).clamp(0.0, 1.0);
        let brightness = progress * progress * progress * progress;
        (block + (brightness * 240.0) as u32).min(240)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_lookup_follows_the_cannon_smoke_shader() {
        // Two rows of a 3-wide gradient.
        let gradient = [
            10, 11, 12, 255, 20, 21, 22, 255, 30, 31, 32, 255, //
            40, 41, 42, 255, 50, 51, 52, 255, 60, 61, 62, 255,
        ];
        let frame = [
            2, 0, 0, 200, // column 2
            1, 9, 9, 20, // under 0.1 alpha: discarded
            7, 0, 0, 255, // past the gradient: black
        ];
        assert_eq!(
            recolor(&frame, &gradient, 3, 1),
            vec![60, 61, 62, 200, 0, 0, 0, 0, 0, 0, 0, 255]
        );
        assert_eq!(recolor(&frame, &gradient, 3, 0)[..4], [30, 31, 32, 200]);
        assert_eq!(
            gradient_name("createbigcannons:particle/cannon_smoke_3", 4),
            "createbigcannons:particle/cannon_smoke_3#gradient4"
        );
    }

    #[test]
    fn wind_drifts_slowly_within_the_configured_speed() {
        let mut wind = Wind::new();
        wind.refresh(42);
        let maximum = 1.25 * 0.05;
        let mut previous = wind.force(1.0);
        for _ in 0..200 {
            wind.update();
            let force = wind.force(1.0);
            assert_eq!(force.y, 0.0);
            assert!(force.length() <= maximum + 1e-9, "{force:?}");
            // One tick changes the bearing by at most a few degrees.
            assert!(
                force.distance(previous) < maximum * 0.5,
                "{force:?} {previous:?}"
            );
            assert!(wind.force(0.0).abs_diff_eq(previous, 1e-12));
            previous = force;
        }
        let mut same = Wind::new();
        same.refresh(42);
        for _ in 0..200 {
            same.update();
        }
        assert_eq!(same.force(1.0), previous);
    }

    /// An engine whose atlas holds CBC's eleven smoke frames and `rows`
    /// gradient rows of them, over a pack with CBC's `cannon_smoke.json`.
    fn engine_with_smoke(rows: u32) -> ParticleEngine {
        let root = std::env::temp_dir().join(format!(
            "horizon-cbc-particles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let json = root.join("assets/createbigcannons/particles/cannon_smoke.json");
        std::fs::create_dir_all(json.parent().unwrap()).unwrap();
        let textures: Vec<String> = (0..=10)
            .map(|i| format!("\"createbigcannons:cannon_smoke_{i}\""))
            .collect();
        std::fs::write(&json, format!("{{\"textures\":[{}]}}", textures.join(","))).unwrap();
        let rect = crate::hud::Rect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            w: 16,
            h: 16,
        };
        let mut atlas = std::collections::HashMap::new();
        for i in 0..=10 {
            let frame = format!("{CANNON_SMOKE_SPRITE}{i}");
            for row in 0..rows {
                atlas.insert(gradient_name(&frame, row), rect);
            }
            atlas.insert(frame, rect);
        }
        let shell_json = json.with_file_name("shell_explosion_smoke.json");
        std::fs::write(
            &shell_json,
            include_str!("fixtures/shell_explosion_smoke.json"),
        )
        .unwrap();
        for i in 0..10 {
            atlas.insert(
                format!("createbigcannons:particle/explosion_smoke{i}"),
                rect,
            );
        }
        let engine = ParticleEngine::new(&root, &atlas, &Default::default());
        let _ = std::fs::remove_dir_all(&root);
        engine
    }

    fn emitted(engine: &mut ParticleEngine) -> Vec<(ParticleOptions, [f64; 3], [f64; 3])> {
        engine.requested.replace(Vec::new()).unwrap_or_default()
    }

    fn plume(engine: &mut ParticleEngine, options: CbcParticle, v: [f64; 3]) -> Particle {
        engine.make_cbc(options, [1.0, 2.0, 3.0], v, 7).unwrap()
    }

    #[test]
    fn big_cannon_plume_emits_smoke_clouds_and_flames_by_particle_setting() {
        let mut engine = engine_with_smoke(16);
        let options = CbcParticle::BigCannonPlume {
            size: 8.0,
            power: 2.0,
        };
        let mut p = plume(&mut engine, options, [0.0, 0.0, 1.0]);
        assert!(p.no_render);
        assert_eq!(
            (p.velocity, p.friction, p.gravity),
            ([0.0, 0.0, 0.5], 0.9, 0.0)
        );
        engine.requested = Some(Vec::new());
        engine.tick_big_cannon_plume(&mut p, DVec3::Z, 8.0, 2.0);
        let all = emitted(&mut engine);
        let smoke: Vec<_> = all.iter().filter(|(o, ..)| o.cbc.is_some()).collect();
        let kind = |ty| {
            all.iter()
                .filter(|(o, ..)| o.cbc.is_none() && o.ty == ty)
                .count()
        };
        // smallScale 2: min(3, ceil(2 * 5)) smoke, floor(2 * 10) clouds, 2 * 5 flames.
        assert_eq!(smoke.len(), 3);
        assert_eq!(kind(ParticleType::Cloud), 20);
        assert_eq!(kind(ParticleType::Flame), 10);
        for (options, at, v) in smoke {
            let Some(CbcParticle::CannonSmoke {
                power,
                size,
                lifetime,
                friction,
            }) = options.cbc
            else {
                panic!("smoke");
            };
            assert_eq!((power, size, friction), (2.0, 2.0, 0.95));
            // 30 + nextInt(10) + ceil(10 * 0) + ceil(2 * 2) / 2.
            assert!((32..42).contains(&lifetime), "{lifetime}");
            assert_eq!(*at, [1.0, 2.0, 3.0]);
            assert!((0.8..1.05).contains(&v[2]), "{v:?}");
        }
        // Decreased: smoke only, min(3, ceil(2 * 3)); minimal: ceil(2 * 1).
        for (level, count) in [(1, 3), (2, 2)] {
            engine.particle_level = level;
            engine.requested = Some(Vec::new());
            engine.tick_big_cannon_plume(&mut p, DVec3::Z, 8.0, 2.0);
            let all = emitted(&mut engine);
            assert_eq!(all.len(), count);
            assert!(all.iter().all(|(o, ..)| o.cbc.is_some()));
        }
        // No clouds after the first tick, no flames from the fifth.
        engine.particle_level = 0;
        p.age = 5;
        engine.requested = Some(Vec::new());
        engine.tick_big_cannon_plume(&mut p, DVec3::Z, 8.0, 2.0);
        assert!(emitted(&mut engine).iter().all(|(o, ..)| o.cbc.is_some()));
    }

    #[test]
    fn autocannon_and_drop_mortar_plumes_puff_clouds() {
        let mut engine = engine_with_smoke(1);
        let mut p = plume(
            &mut engine,
            CbcParticle::AutocannonPlume { scale: 1.0 },
            [1.0, 0.0, 0.0],
        );
        assert_eq!((p.lifetime, p.velocity), (5, [0.0; 3]));
        engine.requested = Some(Vec::new());
        engine.tick_autocannon_plume(&mut p, DVec3::X, 1.0);
        let all = emitted(&mut engine);
        let count = |ty| all.iter().filter(|(o, ..)| o.ty == ty).count();
        // ceil(1 * 2) clouds, then max(5, ceil(1 * 8)) flames the first tick.
        assert_eq!(
            (count(ParticleType::Cloud), count(ParticleType::Flame)),
            (2, 8)
        );
        let mut p = plume(
            &mut engine,
            CbcParticle::DropMortarPlume { scale: 1.0 },
            [0.0, 1.0, 0.0],
        );
        assert_eq!(p.lifetime, 0);
        engine.requested = Some(Vec::new());
        engine.tick_drop_mortar_plume(&mut p, DVec3::Y, 1.0);
        assert_eq!(emitted(&mut engine).len(), 20);
        // Only the All particle setting shows them.
        engine.particle_level = 1;
        let p = plume(
            &mut engine,
            CbcParticle::AutocannonPlume { scale: 1.0 },
            [1.0, 0.0, 0.0],
        );
        assert!(matches!(
            p.behaviour,
            Behaviour::CbcAutocannonPlume { visible: false, .. }
        ));
    }

    #[test]
    fn cannon_smoke_takes_its_power_row_and_grows_glowing_then_dark() {
        let mut engine = engine_with_smoke(2);
        assert_eq!(engine.cbc_smoke_sets.len(), 2);
        let smoke = |power| CbcParticle::CannonSmoke {
            power,
            size: 2.0,
            lifetime: 30,
            friction: 0.95,
        };
        let name = |engine: &ParticleEngine, p: &Particle| match p.sprite {
            super::super::Sprite::Index(i) => engine.names[i as usize].clone(),
            _ => panic!("indexed sprite"),
        };
        for (power, row) in [(0.5, 0), (1.0, 0), (2.0, 1), (9.0, 1)] {
            let p = engine
                .make_cbc(smoke(power), [0.0; 3], [0.0; 3], 1)
                .unwrap();
            assert_eq!(p.set, Some(engine.cbc_smoke_sets[row]));
            assert_eq!(
                name(&engine, &p),
                gradient_name(&format!("{CANNON_SMOKE_SPRITE}0"), row as u32)
            );
            assert_eq!(p.translucent, Some(true));
            assert_eq!(
                (p.size, p.friction, p.lifetime, p.gravity),
                (2.0, 0.95, 30, -0.05)
            );
        }
        let mut p = engine.make_cbc(smoke(2.0), [0.0; 3], [0.0; 3], 1).unwrap();
        assert!((p.cbc_smoke_size(0.0) - 1.8).abs() < 1e-6);
        assert_eq!(p.cbc_smoke_block_light(0.0, 32, 1.5), 240);
        p.age = 15;
        assert!((p.cbc_smoke_size(0.0) - 5.0).abs() < 1e-5);
        engine.cbc_smoke_sprite(&mut p, 1.5);
        // progress 0.75: the last frames already.
        assert_eq!(
            name(&engine, &p),
            gradient_name(&format!("{CANNON_SMOKE_SPRITE}9"), 1)
        );
        p.age = 20;
        assert_eq!(p.cbc_smoke_block_light(0.0, 32, 1.5), 32);
        // Without its gradient no smoke is drawn.
        let mut bare = engine_with_smoke(0);
        assert!(bare.make_cbc(smoke(2.0), [0.0; 3], [0.0; 3], 1).is_none());
    }

    #[test]
    fn shell_explosion_cloud_emits_primary_smoke_until_age_five() {
        let mut engine = engine_with_smoke(0);
        let options = CbcParticle::ShellExplosionCloud {
            power: 2.0,
            plume: true,
        };
        for (level, count) in [(0, 21), (1, 11), (2, 6)] {
            engine.particle_level = level;
            let mut p = plume(&mut engine, options, [0.0; 3]);
            assert!(p.no_render);
            assert_eq!(p.lifetime, 10);
            let tick = |engine: &mut ParticleEngine, p: &mut Particle| {
                engine.requested = Some(Vec::new());
                engine.emit_shell_explosion(p, 2.0, true, 7, level);
                emitted(engine)
            };
            let primary = |o: &ParticleOptions| {
                matches!(
                    o.cbc,
                    Some(CbcParticle::ShellExplosionSmoke {
                        lifetime: 160..=239,
                        scale: 4.0
                    })
                )
            };
            let age_zero = tick(&mut engine, &mut p);
            assert_eq!(age_zero.iter().filter(|(o, ..)| primary(o)).count(), count);
            assert!(
                age_zero[..count]
                    .iter()
                    .all(|(o, at, _)| primary(o) && at[1] >= 2.5)
            );
            if level == 0 {
                // The first plume, from the Java formulas on the particle's own
                // random: Particle(level, x, y, z) draws its lifetime, the
                // constructor draws 12 + nextInt(6) trails of six jitters, then
                // rx, ry, rz, dx, dy, dz and 160 + nextInt(80).
                let mut r = LegacyRandom::new(7);
                r.next_float();
                let trails = 12 + r.next_int_bound(6).unwrap();
                for _ in 0..trails * 12 {
                    r.next_double();
                }
                let mut jitter = || {
                    let a = r.next_double();
                    a - r.next_double()
                };
                let (rx, ry, rz) = (jitter(), jitter().abs(), jitter());
                let (dx, dy, dz) = (jitter(), jitter() + 1.2, jitter());
                let lifetime = 160 + r.next_int_bound(80).unwrap();
                // power 2: displacement 0.7, velocity 0.5 at age 0; the cloud sits at (1, 2, 3).
                let displacement = 2.0 * 0.35;
                let expected_pos = DVec3::new(
                    1.0 + rx * displacement,
                    ry * displacement + (2.0 + 0.5),
                    3.0 + rz * displacement,
                );
                let expected_velocity = DVec3::new(dx, dy, dz) * (2.0 * 0.25);
                let (options, pos, velocity) = &age_zero[0];
                assert_eq!(
                    options.cbc,
                    Some(CbcParticle::ShellExplosionSmoke {
                        lifetime,
                        scale: 4.0
                    })
                );
                assert!(DVec3::from_array(*pos).abs_diff_eq(expected_pos, 1e-12));
                assert!(DVec3::from_array(*velocity).abs_diff_eq(expected_velocity, 1e-12));
            }
            p.age = 5;
            // Changing the setting changes the steps, while the constructor's
            // trail count and random draws still use the original setting.
            engine.particle_level = 2;
            let last_trails = tick(&mut engine, &mut p);
            let trails = replay_shell_trails(7, level, 2.0, true).count();
            assert_eq!(last_trails.len(), trails * 2);
            assert!(last_trails.iter().all(|(o, ..)| matches!(
                o.cbc,
                Some(CbcParticle::ShellExplosionSmoke {
                    lifetime: 30..=44,
                    scale: 1.5
                })
            )));
            p.age = 6;
            assert!(tick(&mut engine, &mut p).is_empty());
        }
    }

    #[test]
    fn shell_explosion_trails_replay_the_constructor_rng() {
        let mut engine = engine_with_smoke(0);
        for level in 0..=2 {
            engine.particle_level = level;
            let mut p = plume(
                &mut engine,
                CbcParticle::ShellExplosionCloud {
                    power: 2.0,
                    plume: true,
                },
                [0.0; 3],
            );
            let mut constructor = LegacyRandom::new(7);
            constructor.next_float(); // Particle::base_at
            let built: Vec<_> = shell_trails(&mut constructor, level, 2.0, true).collect();
            let replayed: Vec<_> = replay_shell_trails(7, level, 2.0, true).collect();
            assert_eq!(built, replayed);
            assert_eq!(p.random.next_long(), constructor.next_long());
            assert!((4..=17).contains(&built.len()));
            assert_eq!(built[0].displacement_at(0), built[0].displacement);
            assert!(
                (built[0].displacement_at(1)
                    - (built[0].displacement + built[0].velocity - DVec3::Y * 0.5))
                    .length()
                    < 1e-12
            );
        }
    }

    #[test]
    fn shell_explosion_smoke_uses_its_own_sprites_and_curves() {
        let mut engine = engine_with_smoke(0);
        let mut p = plume(
            &mut engine,
            CbcParticle::ShellExplosionSmoke {
                lifetime: 64,
                scale: 2.0,
            },
            [0.0; 3],
        );
        let frame = |p: &Particle| match p.sprite {
            super::super::Sprite::Index(i) => engine.names[i as usize].as_str(),
            _ => panic!("indexed sprite"),
        };
        assert_eq!(frame(&p), "createbigcannons:particle/explosion_smoke0");
        assert_eq!((p.bb_width, p.bb_height), (0.25, 0.25));
        assert_eq!(p.translucent, Some(true));
        assert!((p.quad_size(0.0) / p.size - 0.95).abs() < 1e-6);
        assert_eq!(p.light_coords(0.0, 80, 32), (80, 240));
        p.age = 7;
        assert_eq!(p.light_coords(1.0, 80, 32), (80, 47));
        p.age = 32;
        assert!((p.quad_size(0.0) / p.size - 1.75).abs() < 1e-6);
        assert_eq!(p.light_coords(0.0, 80, 32), (80, 32));
        assert_eq!(p.shell_explosion_smoke_alpha(), 0.875);
        engine.cbc_smoke_sprite(&mut p, 4.0);
        assert_eq!(frame(&p), "createbigcannons:particle/explosion_smoke9");
        p.age = p.lifetime;
        assert_eq!(p.shell_explosion_smoke_alpha(), 0.0);
    }

    fn empty_level() -> (
        super::super::World,
        super::super::BlockFacts,
        horizon_world::block_collision::BlockCollisionRegistry<'static>,
    ) {
        use horizon_world::{
            block_collision::BlockCollisionRegistry,
            chunk::BlockState,
            collision::CachedCollisionRegistry,
            nbt::Tag,
            registry::{BlockRegistry, BlockStateId},
        };
        use std::{
            collections::BTreeMap,
            sync::{Arc, OnceLock},
        };
        static REGISTRY: OnceLock<BlockRegistry> = OnceLock::new();
        let registry = REGISTRY.get_or_init(|| {
            BlockRegistry::from_ordered_states(
                vec![BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                }],
                BTreeMap::from([("minecraft:air".into(), 0)]),
            )
            .unwrap()
        });
        let state = Tag::String("minecraft:air".into());
        let tag = Tag::Compound(BTreeMap::from([
            ("DataVersion".into(), Tag::Int(horizon_world::DATA_VERSION)),
            ("states".into(), Tag::List(vec![state])),
            ("shapes".into(), Tag::List(Vec::new())),
            ("cached_shapes".into(), Tag::IntArray(vec![-1])),
        ]));
        let cached = CachedCollisionRegistry::from_nbt(&tag, registry).unwrap();
        let collision = BlockCollisionRegistry::new(Arc::new(cached)).unwrap();
        let air = BlockStateId(0);
        (
            super::super::World::new(air, 0),
            super::super::BlockFacts {
                states: Vec::new(),
                air,
            },
            collision,
        )
    }

    fn fragment_fixture() -> ParticleEngine {
        let mut engine = engine_with_trail();
        let set = engine.modded_sets[TRAIL_SMOKE];
        for name in [
            "shrapnel_smoke",
            "flak_smoke",
            "debris_smoke",
            "gas_cloud",
            "smoke_shell_smoke",
            "cannon_smoke_fallback",
        ] {
            let new_set = engine.set_sprites.len() as u32;
            engine
                .set_sprites
                .push(engine.set_sprites[set as usize].clone());
            engine
                .modded_sets
                .insert(format!("createbigcannons:{name}"), new_set);
        }
        engine
    }

    #[test]
    fn shrapnel_and_fluid_clouds_emit_on_both_ticks_of_the_one_tick_lifetime() {
        let (world, facts, collision) = empty_level();
        let level = super::super::ParticleLevel {
            world: &world,
            facts: &facts,
            collision: &collision,
            player: None,
        };
        for cloud in [CbcParticle::ShrapnelCloud, CbcParticle::FluidCloud] {
            let mut engine = fragment_fixture();
            engine.particle_level = 1;
            for age in 0..=1 {
                let mut parent = plume(&mut engine, cloud, [0.0; 3]);
                parent.age = age;
                assert!(parent.no_render);
                assert_eq!(parent.lifetime, 1);
                engine.requested = Some(Vec::new());
                engine.tick_cbc(&mut parent, &level);
                let spawned = emitted(&mut engine);
                assert_eq!(spawned.len(), 10);
                assert!(spawned.iter().all(|(o, _, v)| {
                    matches!(
                        o.cbc,
                        Some(CbcParticle::ShrapnelSmoke { lifetime: 85..=164 })
                    ) && v.iter().all(|component| component.abs() <= 0.7)
                }));
                assert_eq!(parent.removed, age == 1);
            }
        }
    }

    #[test]
    fn flak_trail_count_uses_level_random_and_continues_through_age_five() {
        let mut engine = fragment_fixture();
        let mut level_random = LegacyRandom::new(42);
        let trails = 4 + level_random.next_int_bound(3).unwrap();
        engine.level_random = LegacyRandom::new(42);
        engine.particle_level = 1;
        let mut parent = plume(&mut engine, CbcParticle::FlakCloud, [0.0; 3]);
        assert_eq!(parent.lifetime, 30);
        let Behaviour::CbcFragmentCloud {
            seed,
            trails: count,
            ..
        } = parent.behaviour
        else {
            panic!("expected a flak emitter");
        };
        assert_eq!(count, trails);
        for (age, primary, secondary) in [(0, 20, trails * 3), (5, 0, trails * 3), (6, 0, 0)] {
            parent.age = age;
            engine.requested = Some(Vec::new());
            engine.emit_fragment_cloud(&mut parent, true, seed, count);
            let spawned = emitted(&mut engine);
            assert_eq!(spawned.len(), (primary + secondary) as usize);
            for (o, _, _) in &spawned[..primary as usize] {
                assert!(matches!(
                    o.cbc,
                    Some(CbcParticle::FlakSmoke {
                        lifetime: 45..=124,
                        scale: 3.0
                    })
                ));
            }
            for (o, _, v) in &spawned[primary as usize..] {
                assert!(matches!(
                    o.cbc,
                    Some(CbcParticle::FlakSmoke {
                        lifetime: 20..=24,
                        scale: 1.0
                    })
                ));
                assert!(v.iter().all(|component| component.abs() <= 0.01));
            }
        }
    }

    #[test]
    fn fragment_smoke_providers_use_their_sprites_and_scales() {
        let mut engine = fragment_fixture();
        let mut local = LegacyRandom::new(12);
        for _ in 0..6 {
            local.next_float();
        }
        let base = 0.1 * (local.next_float() * 0.5 + 0.5) * 2.0;
        for (options, scale, width, set) in [
            (
                CbcParticle::ShrapnelSmoke { lifetime: 90 },
                6.0,
                0.35,
                "createbigcannons:shrapnel_smoke",
            ),
            (
                CbcParticle::FlakSmoke {
                    lifetime: 90,
                    scale: 3.0,
                },
                3.0,
                0.25,
                "createbigcannons:flak_smoke",
            ),
        ] {
            let mut replay = LegacyRandom::new(9);
            replay.next_float();
            replay.next_float();
            engine.level_random = LegacyRandom::new(9);
            let p = engine.make_cbc(options, [0.0; 3], [1.0; 3], 12).unwrap();
            assert_eq!(
                p.velocity,
                [1.0, 1.0 + f64::from(replay.next_float() / 500.0), 1.0]
            );
            assert_eq!(p.lifetime, 90);
            assert_eq!(p.size, (base * 0.75) * scale);
            assert_eq!(p.bb_width, width);
            assert_eq!(p.set, engine.modded_sets.get(set).copied());
            assert_eq!(p.translucent, Some(true));
            assert!(p.quad_size(1.0) > p.quad_size(0.0));
            assert_eq!(p.cbc_smoke_block_light(0.0, 0, 4.0), 240);
        }
    }

    #[test]
    fn mortar_debris_replays_the_level_jitter_and_uses_trail_physics() {
        let mut engine = fragment_fixture();
        engine.requested = Some(Vec::new());
        engine.level_random = LegacyRandom::new(73);
        engine.cbc_mortar_stone_cloud([0.0; 3], 2.0);
        let spawned = emitted(&mut engine);
        assert_eq!(spawned.len(), 50);
        let mut replay = LegacyRandom::new(73);
        let mut sample = || replay.next_double() - replay.next_double();
        let at = std::array::from_fn(|_| sample() * f64::from(0.1f32));
        let spread = f64::from(0.15f32 * 2.0);
        let velocity = [
            sample() * spread,
            (sample() + 0.5) * spread,
            sample() * spread,
        ];
        assert_eq!((spawned[0].1, spawned[0].2), (at, velocity));
        let options = CbcParticle::DebrisSmoke { scale: 2.0 };
        assert_eq!(spawned[0].0.cbc, Some(options));
        let p = engine.make_cbc(options, at, velocity, 12).unwrap();
        assert_eq!((p.gravity, p.friction, p.lifetime), (0.0025, 0.92, 120));
        assert_eq!((p.rgba[3], p.bb_width), (0.8, 0.5));
        assert_eq!(p.behaviour, Behaviour::CbcTrailSmoke);
    }

    #[test]
    fn breech_poofs_use_the_input_direction_and_particle_setting() {
        let mut engine = fragment_fixture();
        let (world, facts, collision) = empty_level();
        let level = super::super::ParticleLevel {
            world: &world,
            facts: &facts,
            collision: &collision,
            player: None,
        };
        for (status, count) in [(0, 10), (1, 2), (2, 0)] {
            engine.particle_level = status;
            let mut p = plume(&mut engine, CbcParticle::QuickFiringBreechSmoke, [1.0; 3]);
            assert_eq!(p.lifetime, 2);
            assert_eq!(p.velocity, [0.0; 3]);
            engine.requested = Some(Vec::new());
            engine.tick_cbc(&mut p, &level);
            let spawned = emitted(&mut engine);
            assert_eq!(spawned.len(), count);
            assert!(spawned.iter().all(|(o, _, v)| {
                o.ty == ParticleType::Poof && v.iter().all(|x| (0.995..=1.005).contains(x))
            }));
        }
    }

    #[test]
    fn smoke_shell_and_gas_grow_but_only_smoke_shell_changes_color() {
        let mut engine = fragment_fixture();
        let (world, facts, collision) = empty_level();
        let level = super::super::ParticleLevel {
            world: &world,
            facts: &facts,
            collision: &collision,
            player: None,
        };
        let color = [0.25, 0.5, 0.75];
        let mut smoke = plume(
            &mut engine,
            CbcParticle::SmokeShellSmoke { scale: 4.0 },
            [0.0; 3],
        );
        let mut gas = plume(
            &mut engine,
            CbcParticle::GasCloud { scale: 2.0, color },
            [0.0; 3],
        );
        assert_eq!(smoke.lifetime, 200);
        assert_eq!(gas.lifetime, smoke.lifetime);
        let initial = gas.size;
        engine.tick_cbc(&mut smoke, &level);
        engine.tick_cbc(&mut gas, &level);
        assert!(gas.size > initial);
        assert_eq!(gas.rgba[..3], color);
        assert_eq!(smoke.rgba[..3], [0.85 + (0.75 - 0.85) / 60.0; 3]);
    }

    #[test]
    fn fallback_smoke_keeps_the_provider_lifetime_size_and_friction() {
        let mut engine = fragment_fixture();
        let p = plume(
            &mut engine,
            CbcParticle::FallbackCannonSmoke {
                size: 2.0,
                lifetime: 70,
                friction: 0.75,
            },
            [0.0; 3],
        );
        assert_eq!((p.size, p.lifetime, p.friction), (2.0, 70, 0.75));
        assert_eq!(
            p.set,
            engine
                .modded_sets
                .get("createbigcannons:cannon_smoke_fallback")
                .copied()
        );
        assert_eq!(p.translucent, Some(true));
    }

    #[test]
    fn wrap_degrees_matches_mth() {
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(180.0), -180.0);
        assert_eq!(wrap_degrees(540.0), -180.0);
    }

    #[test]
    fn plume_axes_fall_back_for_vertical_shots() {
        let (right, up) = plume_axes(DVec3::Y * 2.0);
        assert_eq!(right, DVec3::X);
        assert!(up.abs_diff_eq(DVec3::new(0.0, 0.0, -1.0), 1e-12));
        let (right, up) = plume_axes(DVec3::Z);
        assert!(right.abs_diff_eq(DVec3::new(-1.0, 0.0, 0.0), 1e-12));
        assert!(up.abs_diff_eq(DVec3::NEG_Y, 1e-12));
    }

    /// An engine over a pack with CBC's `trail_smoke.json` and vanilla's
    /// twelve big smoke frames, named in the atlas as vanilla's own particle
    /// sprites are (`particle/big_smoke_0`, no namespace).
    fn engine_with_trail() -> ParticleEngine {
        let root = std::env::temp_dir().join(format!(
            "horizon-cbc-trail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let json = root.join("assets/createbigcannons/particles/trail_smoke.json");
        std::fs::create_dir_all(json.parent().unwrap()).unwrap();
        let textures: Vec<String> = (0..12)
            .map(|i| format!("\"minecraft:big_smoke_{i}\""))
            .collect();
        std::fs::write(&json, format!("{{\"textures\":[{}]}}", textures.join(","))).unwrap();
        let rect = crate::hud::Rect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            w: 16,
            h: 16,
        };
        let atlas = (0..12)
            .map(|i| (format!("particle/big_smoke_{i}"), rect))
            .collect();
        let engine = ParticleEngine::new(&root, &atlas, &Default::default());
        let _ = std::fs::remove_dir_all(&root);
        engine
    }

    #[test]
    fn autocannon_short_trail_uses_ten_fifty_tick_puffs() {
        let mut engine = engine_with_trail();
        engine.level_random = LegacyRandom::new(29);
        engine.requested = Some(Vec::new());
        engine.cbc_autocannon_trail([0.0; 3], [2.0, 0.0, 0.0]);
        let spawned = emitted(&mut engine);
        assert_eq!(spawned.len(), 10);
        let mut replay = LegacyRandom::new(29);
        for (index, (option, at, velocity)) in spawned.into_iter().enumerate() {
            assert_eq!(option.cbc, Some(CbcParticle::TrailSmoke { lifetime: 50 }));
            assert_eq!(at, [f64::from(index as f32 * 0.1) * 2.0, 0.0, 0.0]);
            assert_eq!(
                velocity,
                std::array::from_fn(|_| replay.next_double() * 0.004 - 0.002)
            );
        }
    }

    #[test]
    fn shell_trail_lays_ten_always_visible_puffs_along_the_tick() {
        let mut engine = engine_with_trail();
        engine.requested = Some(Vec::new());
        engine.cbc_shell_trail([0.0, 64.0, 0.0], [0.0, 64.0, 10.0]);
        let all = emitted(&mut engine);
        assert_eq!(all.len(), 10);
        for (i, (options, at, v)) in all.iter().enumerate() {
            assert_eq!(options.cbc, Some(CbcParticle::TrailSmoke { lifetime: 100 }));
            assert_eq!(at[..2], [0.0, 64.0]);
            assert!((at[2] - i as f64).abs() < 1e-6, "{at:?}");
            assert!(v.iter().all(|c| c.abs() <= 0.002), "{v:?}");
        }
        let p = engine
            .make_cbc(
                CbcParticle::TrailSmoke { lifetime: 100 },
                [0.0; 3],
                [0.0; 3],
                3,
            )
            .unwrap();
        assert_eq!(p.behaviour, Behaviour::CbcTrailSmoke);
        assert_eq!((p.gravity, p.friction, p.lifetime), (3.0e-6, 1.0, 100));
        assert_eq!((p.rgba[3], p.bb_width, p.bb_height), (0.35, 0.25, 0.25));
        // SingleQuadParticle's 0.1..0.2 quad, scaled three times.
        assert!((0.3..=0.6).contains(&p.size), "{}", p.size);
        // A tiny upward drift of at most 1/500.
        assert!((0.0..0.002).contains(&p.velocity[1]), "{:?}", p.velocity);
        assert_eq!(p.translucent, Some(true));
        assert!(matches!(p.sprite, super::super::Sprite::Index(i) if i != u32::MAX));
    }
}
