//! CBC impact and splash particle providers and emitters.
use super::{CbcParticle, LegacyRandom, RandomSource, normalize};
use crate::particles::CbcBlockSprite;
use crate::particles::{
    Behaviour, Light, Particle, ParticleEngine, ParticleLevel, ParticleOptions, ParticleType,
    Sprite,
};
use glam::{DVec3, Quat, Vec3};
use horizon_world::registry::BlockStateId;
use std::f32::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CbcShard {
    Splinter,
    Glass,
    Leaf,
}

impl CbcShard {
    fn sprite_set(self) -> &'static str {
        match self {
            Self::Splinter => "createbigcannons:splinter",
            Self::Glass => "createbigcannons:glass_shard",
            Self::Leaf => "createbigcannons:leaf",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CbcImpact {
    Block,
    Matter {
        state: BlockStateId,
        deflect: bool,
        force: bool,
    },
    SmokeBurst,
    SparkBurst {
        color: [f32; 3],
        deflect: bool,
        count: i32,
    },
    Spark {
        color: [f32; 3],
    },
    ShardBurst {
        state: BlockStateId,
        kind: CbcShard,
        count: i32,
    },
    Shard(CbcShard),
    Splash {
        color: [f32; 3],
        light: f32,
    },
    Spray {
        color: [f32; 3],
        size: f32,
        light: f32,
        lifetime: i32,
    },
}

fn difference(random: &mut LegacyRandom) -> f64 {
    random.next_double() - random.next_double()
}

fn jitter(random: &mut LegacyRandom) -> DVec3 {
    DVec3::new(difference(random), difference(random), difference(random))
}

fn options(particle: CbcImpact) -> ParticleOptions {
    ParticleOptions::cbc(CbcParticle::Impact(particle))
}

impl ParticleEngine {
    pub(crate) fn make_cbc_impact(
        &mut self,
        impact: CbcImpact,
        options: &ParticleOptions,
        at: [f64; 3],
        velocity: [f64; 3],
        seed: i64,
    ) -> Option<Particle> {
        let mut random = LegacyRandom::new(seed);
        let mut p = match impact {
            CbcImpact::Block | CbcImpact::Shard(_) => {
                let (sprite, tint) = options.block.clone()?;
                let mut p = Particle::terrain(&mut random, at, velocity, sprite, tint);
                if let CbcImpact::Shard(kind) = impact {
                    p.roll = random.next_float() * PI;
                    let yaw = random.next_float() * -0.5 * FRAC_PI_2;
                    p.size = 0.3 + random.next_float() * 0.1;
                    p.set_size(0.1, 0.1);
                    p.lifetime = 30 + random.next_int_bound(8).ok()?;
                    p.set_color(
                        ((tint >> 16) & 255) as f32 / 255.0,
                        ((tint >> 8) & 255) as f32 / 255.0,
                        (tint & 255) as f32 / 255.0,
                    );
                    p.friction = 0.99;
                    let pitch = if kind == CbcShard::Leaf {
                        p.gravity = 0.5;
                        p.friction = 0.9;
                        random.next_float() * FRAC_PI_2
                    } else {
                        0.0
                    };
                    let set = *self.modded_sets.get(kind.sprite_set())?;
                    let sprites = &self.set_sprites[set as usize];
                    let index = self
                        .level_random
                        .next_int_bound(sprites.len() as i32)
                        .ok()? as usize;
                    let mask = sprites[index];
                    let r = self.rects.get(mask as usize).copied().flatten()?;
                    p.uv = [r.u0, r.v0, r.u1, r.v1];
                    p.behaviour = Behaviour::CbcShard { yaw, pitch, mask };
                }
                p
            }
            CbcImpact::Spark { color: [r, g, b] } => {
                let mut p = Particle::base_at(&mut random, ParticleType::Poof, at);
                p.set_color(r, g, b);
                p.gravity = 0.5;
                p.friction = 0.8;
                p.lifetime = 12 + self.level_random.next_int_bound(12).ok()?;
                p.light = Light::CbcSpark;
                p.behaviour = Behaviour::CbcSpark;
                p.sprite = Sprite::Named("createbigcannons:entity/color".into());
                p
            }
            CbcImpact::Spray {
                color: [r, g, b],
                size,
                light,
                lifetime,
            } => {
                let mut p = Particle::at(&mut random, ParticleType::Poof, at);
                p.rgba = [r, g, b, 0.95];
                p.gravity = 0.5;
                p.size = size;
                p.set_size(size * 0.5, size * 0.5);
                p.lifetime = lifetime;
                p.light = Light::CbcSpray(light);
                p.translucent = Some(true);
                p.behaviour = Behaviour::CbcSpray;
                let set = *self.modded_sets.get("createbigcannons:splash_spray")?;
                let sprites = &self.set_sprites[set as usize];
                let index = random.next_int_bound(sprites.len() as i32).ok()? as usize;
                p.sprite = Sprite::Index(sprites[index]);
                p
            }
            _ => {
                let mut p = Particle::base_at(&mut random, ParticleType::Poof, at);
                p.no_render = true;
                p.lifetime = if matches!(impact, CbcImpact::Splash { .. }) {
                    11
                } else {
                    1
                };
                p.behaviour = Behaviour::CbcImpactEmitter(impact);
                if let Some((sprite, tint)) = &options.block {
                    p.sprite = Sprite::Named(sprite.clone());
                    p.set_color(
                        ((tint >> 16) & 255) as f32,
                        ((tint >> 8) & 255) as f32,
                        (tint & 255) as f32,
                    );
                }
                p
            }
        };
        p.velocity = velocity;
        p.random = random;
        Some(p)
    }

    pub(crate) fn tick_cbc_impact(
        &mut self,
        p: &mut Particle,
        level: &ParticleLevel,
        sprite: Option<&CbcBlockSprite<'_>>,
    ) {
        match p.behaviour {
            Behaviour::CbcImpactEmitter(kind) => self.emit_impact(p, kind, sprite),
            Behaviour::CbcShard { .. } => {
                if p.age >= p.lifetime - 10 {
                    p.rgba[3] -= 0.1;
                }
                p.base_tick(level);
            }
            Behaviour::CbcSpark => {
                if p.on_ground {
                    p.removed = true;
                } else {
                    p.rgba[0] *= 0.99;
                    p.rgba[1] *= 0.98;
                    p.rgba[2] *= 0.97;
                    p.base_tick(level);
                }
            }
            Behaviour::CbcSpray => {
                if p.on_ground {
                    p.age = p.age.max(p.lifetime - 30);
                }
                if p.age >= p.lifetime - 30 && p.rgba[3] > 0.01 {
                    p.rgba[3] -= 0.03;
                }
                p.base_tick(level);
            }
            _ => unreachable!(),
        }
    }

    fn emit_impact(
        &mut self,
        p: &mut Particle,
        kind: CbcImpact,
        sprite: Option<&CbcBlockSprite<'_>>,
    ) {
        let at = DVec3::from_array(p.position);
        let velocity = DVec3::from_array(p.velocity);
        let detail = 1 << self.particle_level.min(2);
        let block = |state, at: DVec3| {
            if let Some(resolve) = sprite {
                resolve(state, ParticleLevel::block_at(at.to_array()))
            } else if let Sprite::Named(sprite) = &p.sprite {
                let [r, g, b, _] = p.rgba;
                Some((
                    sprite.clone(),
                    (r as u32) << 16 | (g as u32) << 8 | b as u32,
                ))
            } else {
                None
            }
        };
        match kind {
            CbcImpact::Matter {
                state,
                deflect,
                force,
            } => {
                let (count, speed, spread) = if deflect {
                    (20, 0.25, 0.15)
                } else {
                    (40, 0.15, 0.35)
                };
                let mut child = options(CbcImpact::Block);
                for _ in 0..count {
                    let position = at + jitter(&mut self.level_random) * 1.5;
                    let motion = velocity * speed + jitter(&mut self.level_random) * spread;
                    child.block = block(state, position);
                    self.add_cbc(&child, position, motion, force);
                }
            }
            CbcImpact::SmokeBurst => {
                let child = ParticleOptions::cbc(CbcParticle::DebrisSmoke { scale: 3.0 });
                for _ in 0..10 {
                    let position = at + jitter(&mut self.level_random) * 0.1;
                    let motion = velocity * 0.2 + jitter(&mut self.level_random) * 0.5;
                    self.add_cbc(&child, position, motion, true);
                }
            }
            CbcImpact::SparkBurst {
                color,
                deflect,
                count,
            } => {
                let (along, across, noise) = if deflect {
                    (0.5, 1.0, 0.015)
                } else {
                    (0.75, 0.75, 0.035)
                };
                let forward = normalize(velocity);
                let axis = if forward.cross(DVec3::Y).length_squared() < 1e-4 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                let up = forward.cross(axis);
                let right = forward.cross(up);
                let child = options(CbcImpact::Spark { color });
                for _ in 0..(count / detail).min(4096) {
                    let position = at + jitter(&mut self.level_random) * 0.5;
                    let speed = 1.0 + difference(&mut self.level_random) * along;
                    let vertical = difference(&mut self.level_random) * across;
                    let horizontal = difference(&mut self.level_random) * across;
                    let motion = velocity * speed
                        + up * vertical
                        + right * horizontal
                        + jitter(&mut self.level_random) * noise;
                    self.add_cbc(&child, position, motion, true);
                }
            }
            CbcImpact::ShardBurst { state, kind, count } => {
                let mut child = options(CbcImpact::Shard(kind));
                let speed = (velocity.length() * 0.75).min(2.0);
                for _ in 0..(count / detail).min(4096) {
                    let position = at + jitter(&mut self.level_random) * 0.75;
                    let motion = jitter(&mut self.level_random) * speed;
                    child.block = block(state, position);
                    self.add_cbc(&child, position, motion, false);
                }
            }
            CbcImpact::Splash { color, light } => {
                let size = velocity.length() as f32 * 2.0;
                let velocity = velocity * (1.0 - f64::from(p.age) / f64::from(p.lifetime));
                let length = velocity.length();
                let lifetime = ((80.0 * length).ceil() as i32).max(40);
                let child = options(CbcImpact::Spray {
                    color,
                    size,
                    light,
                    lifetime,
                });
                for _ in 0..10 / detail {
                    let (position, motion) =
                        splash_sample(&mut p.random, at, velocity, size, p.age);
                    self.add_cbc(&child, position, motion, true);
                }
                p.age += 1;
                p.removed = p.age >= p.lifetime;
                return;
            }
            _ => unreachable!(),
        }
        p.removed = true;
    }
}

fn splash_sample(
    random: &mut LegacyRandom,
    at: DVec3,
    velocity: DVec3,
    size: f32,
    age: i32,
) -> (DVec3, DVec3) {
    let initial = age < 2 && size >= 0.5;
    let spread = if initial {
        8.0 * velocity.length()
    } else {
        f64::from(size.min(1.5))
    };
    let x = at.x + difference(random) * spread;
    let y = if initial { at.y - 0.5 } else { at.y + 0.5 };
    let z = at.z + difference(random) * spread;
    let position = DVec3::new(x, y, z);
    let noise = jitter(random);
    let motion = if initial {
        let delta = at + velocity * 6.0 - position;
        delta * DVec3::new(0.01, 0.03, 0.01) + noise * DVec3::new(0.025, 0.3, 0.025)
    } else {
        velocity + noise * DVec3::new(0.25 * spread, 0.35, 0.25 * spread)
    };
    (position, motion)
}

pub(crate) fn shard_rotation(p: &Particle, yaw: f32, pitch: f32) -> Quat {
    let direction = normalize(DVec3::from_array(p.velocity));
    let dot = direction.x;
    let along = if (dot + 1.0).abs() < 1e-4 {
        Quat::from_rotation_y(PI)
    } else {
        let cross = DVec3::X.cross(direction).as_vec3();
        Quat::from_xyzw(cross.x, cross.y, cross.z, 1.0 + dot as f32)
    };
    (Quat::from_rotation_z(p.roll)
        * Quat::from_rotation_y(yaw)
        * Quat::from_rotation_x(pitch)
        * along)
        .normalize()
}

pub(crate) fn spark_axes(displacement: Vec3) -> (Vec3, Vec3, Vec3) {
    let forward = normalize(displacement.as_dvec3()).as_vec3();
    let c = -forward.z;
    if f64::from(c) <= -0.9999 {
        return (-Vec3::X, Vec3::Y, Vec3::Z);
    }
    let x = -forward.y;
    let y = forward.x;
    let k = 1.0 / (1.0 + c);
    (
        Vec3::new(x * x * k + c, y * x * k, y),
        Vec3::new(x * y * k, y * y * k + c, -x),
        forward,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbc_emitters_keep_their_particle_setting_and_lifetime_rules() {
        let mut engine = ParticleEngine::new(
            std::path::Path::new("nonexistent/cbc-impact-test"),
            &Default::default(),
            &Default::default(),
        );
        for level in 0..=2 {
            engine.particle_level = level;
            let mut sample = |impact: CbcImpact, ticks: usize| {
                let mut particle = engine
                    .make_cbc_impact(impact, &options(impact), [0.0; 3], [0.0, 1.0, 0.0], 17)
                    .unwrap();
                engine.requested = Some(Vec::new());
                for _ in 0..ticks {
                    assert!(particle.is_alive());
                    engine.emit_impact(&mut particle, impact, None);
                }
                assert!(!particle.is_alive());
                engine.requested.take().unwrap().len()
            };
            let impact = CbcImpact::Matter {
                state: BlockStateId(0),
                deflect: false,
                force: true,
            };
            let deflection = CbcImpact::Matter {
                state: BlockStateId(0),
                deflect: true,
                force: true,
            };
            assert_eq!(sample(impact, 1), 2 * sample(deflection, 1));

            let spark = CbcImpact::SparkBurst {
                color: [1.0; 3],
                count: 30,
                deflect: false,
            };
            let splash = CbcImpact::Splash {
                color: [1.0; 3],
                light: 0.0,
            };
            let divisor = 1 << level;
            assert_eq!(sample(spark, 1), 30 / divisor);
            assert_eq!(sample(splash, 11), (10 / divisor) * 11);
        }
    }

    #[test]
    fn cbc_debris_resolves_sprite_at_each_scattered_position() {
        let mut engine = ParticleEngine::new(
            std::path::Path::new("nonexistent/cbc-debris-test"),
            &Default::default(),
            &Default::default(),
        );
        let state = BlockStateId(17);
        let impact = CbcImpact::Matter {
            state,
            deflect: false,
            force: true,
        };
        let mut p = engine
            .make_cbc_impact(impact, &options(impact), [0.0; 3], [0.0; 3], 24)
            .unwrap();
        engine.level_random = LegacyRandom::new(29);
        engine.requested = Some(Vec::new());
        let sprite = |id: BlockStateId, [x, y, z]: [i32; 3]| {
            assert_eq!(id, state);
            Some((format!("test:{x}/{y}/{z}"), 0xffffff))
        };
        engine.emit_impact(&mut p, impact, Some(&sprite));
        let output = engine.requested.take().unwrap();
        let positions = output
            .iter()
            .map(|(particle, at, _)| {
                let pos = ParticleLevel::block_at(*at);
                assert_eq!(particle.block, sprite(state, pos));
                pos
            })
            .collect::<std::collections::HashSet<_>>();
        assert!(positions.len() > 1);
    }

    #[test]
    fn cbc_splash_rng_replays_the_java_draw_order() {
        let mut actual = LegacyRandom::new(29);
        let mut oracle = LegacyRandom::new(29);
        let origin = DVec3::new(12.0, 60.0, -13.0);
        let velocity = DVec3::new(0.5, 0.6, -0.1);
        let spread = 8.0 * velocity.length();
        let x = origin.x + (oracle.next_double() - oracle.next_double()) * spread;
        let y = origin.y - 0.5;
        let z = origin.z + (oracle.next_double() - oracle.next_double()) * spread;
        let focal = origin + velocity * 6.0;
        let dx = (focal.x - x) * 0.01 + (oracle.next_double() - oracle.next_double()) * 0.025;
        let dy = (focal.y - y) * 0.03 + (oracle.next_double() - oracle.next_double()) * 0.3;
        let dz = (focal.z - z) * 0.01 + (oracle.next_double() - oracle.next_double()) * 0.025;
        let (at, motion) = splash_sample(&mut actual, origin, velocity, 2.0, 0);
        assert_eq!(at, DVec3::new(x, y, z));
        assert_eq!(motion, DVec3::new(dx, dy, dz));
        assert_eq!(actual.next_long(), oracle.next_long());
    }
}
