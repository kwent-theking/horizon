//! Client visuals for CBC's Ritchie's Projectile Library bursts.

use crate::game::{Dim, Game, creatures::clip_projectile_blocks};
use glam::{DVec3, Vec3};
use horizon_client::{
    block_entity_render::vertex_light,
    collisions::SceneCollisions,
    hud::Rect,
    mesher::Vertex,
    net::RPL_BURST_SYNC_PACKET,
    particles::{ParticleOptions, ParticleType},
};
use horizon_core::geometry::Vec3 as CoreVec3;
use horizon_protocol::buf::Reader;
use std::collections::HashMap;

/// ShrapnelBurstRenderer's textures/entity/shrapnel.png in the GUI atlas.
pub(super) const SHRAPNEL_SPRITE: &str = "createbigcannons:entity/shrapnel";

#[derive(Clone, Copy)]
pub(in crate::game) enum Kind {
    Shrapnel,
    Flak,
    Grapeshot,
}

impl Kind {
    pub(in crate::game) fn for_entity(kind: &str) -> Option<Self> {
        // CBCEntityTypes registers FlakBurst with ShrapnelBurstRenderer.
        match kind {
            "createbigcannons:shrapnel_burst" => Some(Self::Shrapnel),
            "createbigcannons:flak_burst" => Some(Self::Flak),
            "createbigcannons:grapeshot_burst" => Some(Self::Grapeshot),
            _ => None,
        }
    }

    fn lifetime(self) -> i32 {
        // CBC 5.11.7's bundled munition_properties/projectiles/*_burst.json.
        match self {
            Self::Flak => 5,
            Self::Shrapnel | Self::Grapeshot => 20,
        }
    }

    fn append_quad(
        self,
        origin: Vec3,
        right: Vec3,
        up: Vec3,
        rect: Rect,
        light: u32,
        out: &mut Vec<Vertex>,
    ) {
        // ShrapnelBurstRenderer emits a quad from (0, 0) to (1, 1) with a
        // 180-degree Y rotation. GrapeshotBurstRenderer doubles its size.
        let half = if matches!(self, Self::Grapeshot) {
            1.0
        } else {
            0.5
        };
        let right = -right * half;
        let up = up * half;
        super::projectile::billboard(origin + right + up, right, up, rect, light, out);
    }
}

struct Fragment {
    offset: DVec3,
    velocity: DVec3,
}

impl Fragment {
    fn read(input: &mut Reader<'_>) -> Option<Self> {
        let mut vector = || {
            let v = DVec3::new(
                f64::from(input.f32().ok()?),
                f64::from(input.f32().ok()?),
                f64::from(input.f32().ok()?),
            );
            super::valid_position(v.to_array()).then_some(v)
        };
        Some(Self {
            offset: vector()?,
            velocity: vector()?,
        })
    }
}

struct Burst {
    kind: Kind,
    origin: DVec3,
    age: i32,
    fragments: Vec<Fragment>,
    no_gravity: bool,
}

impl Burst {
    fn at(&self, fragment: &Fragment, partial: f64) -> DVec3 {
        self.origin + (fragment.offset + fragment.velocity * partial)
    }
}

#[derive(Default)]
pub(in crate::game) struct State {
    bursts: HashMap<i32, Burst>,
}

impl State {
    pub(in crate::game) fn add(&mut self, id: i32, kind: Kind, position: [f64; 3]) {
        if super::valid_position(position) {
            self.bursts.insert(
                id,
                Burst {
                    kind,
                    origin: position.into(),
                    age: 0,
                    fragments: Vec::new(),
                    no_gravity: false,
                },
            );
        }
    }

    pub(in crate::game) fn remove(&mut self, id: i32) {
        self.bursts.remove(&id);
    }

    pub(in crate::game) fn packet_position(&self, id: i32) -> Option<[f64; 3]> {
        Some(self.bursts.get(&id)?.origin.to_array())
    }

    pub(in crate::game) fn move_to(&mut self, id: i32, position: Option<[f64; 3]>) -> bool {
        let Some(burst) = self.bursts.get_mut(&id) else {
            return false;
        };
        if let Some(position) = position.filter(|p| super::valid_position(*p)) {
            burst.origin = position.into();
        }
        true
    }

    pub(in crate::game) fn set_no_gravity(&mut self, id: i32, no_gravity: bool) {
        if let Some(burst) = self.bursts.get_mut(&id) {
            burst.no_gravity = no_gravity;
        }
    }

    pub(in crate::game) fn sync(&mut self, data: &[u8]) {
        self.read_sync(data);
    }

    fn read_sync(&mut self, data: &[u8]) -> Option<()> {
        // RPLNetwork's root VarInt and ClientboundSyncBurstSubProjectilesPacket.
        let mut input = Reader::new(data);
        if input.varint().ok()? != i32::from(RPL_BURST_SYNC_PACKET) {
            return None;
        }
        let id = input.varint().ok()?;
        let burst = self.bursts.get_mut(&id)?;
        let age = input.varint().ok()?;
        const FRAGMENT_BYTES: usize = 6 * size_of::<f32>();
        let count = input.count(input.remaining() / FRAGMENT_BYTES).ok()?;
        if !(0..burst.kind.lifetime()).contains(&age) || input.remaining() != count * FRAGMENT_BYTES
        {
            return None;
        }
        let fragments = (0..count)
            .map(|_| Fragment::read(&mut input))
            .collect::<Option<Vec<_>>>()?;
        burst.age = age;
        burst.fragments = fragments;
        Some(())
    }

    fn step(
        &mut self,
        dim: Dim,
        mut hits: impl FnMut(DVec3, DVec3) -> bool,
        mut smoke: impl FnMut(DVec3),
    ) {
        // The three bundled bursts have zero drag and -0.025 gravity.
        let gravity = -0.025
            * match dim {
                Dim::Overworld => 1.0,
                Dim::Nether => 1.1,
                Dim::End => 0.8,
            };
        self.bursts.retain(|_, burst| {
            let origin = burst.origin;
            burst.fragments.retain_mut(|fragment| {
                let from = origin + fragment.offset;
                if hits(from, from + fragment.velocity) {
                    return false;
                }
                fragment.offset += fragment.velocity;
                // CBCProjectileBurst.applyForces uses full drag at low speed.
                if fragment.velocity.length() < 1e-2 {
                    fragment.velocity = DVec3::ZERO;
                }
                if !burst.no_gravity {
                    fragment.velocity.y += gravity;
                }
                smoke(origin + fragment.offset);
                true
            });
            burst.age += 1;
            burst.age < burst.kind.lifetime()
        });
    }
}

impl Game<'_> {
    pub(in crate::game) fn tick_cbc_bursts(&mut self) {
        if self
            .remote
            .as_ref()
            .is_none_or(|r| r.create_contraptions.bursts.bursts.is_empty())
        {
            return;
        }
        // ShrapnelBurst's 0.8-wide fragments use a float half-width in
        // ProjectileUtil.getEntityHitResult. Hits only remove client visuals.
        let half_width = f64::from(0.4_f32);
        let margin = CoreVec3::new(half_width, half_width, half_width);
        let local = self.player_view().filter(|p| p.alive && !p.spectator);
        let Some(remote) = self.remote.as_mut() else {
            return;
        };
        let targets: Vec<_> = self
            .mobs
            .iter()
            .filter(|mob| mob.alive() && !remote.player_is_spectator(mob.id as i32))
            .map(|mob| mob.bounds())
            .chain(local.map(|p| p.bounds))
            .map(|bounds| bounds.inflate(margin))
            .collect();
        let mut scene = SceneCollisions {
            world: &self.world,
            binding: self.collision,
            subject: None,
        };
        let facts = self.res.facts();
        let smoke = ParticleOptions::simple(ParticleType::Smoke);
        let particles = &mut self.particles;
        remote.create_contraptions.bursts.step(
            self.dim,
            |from, to| {
                let from = CoreVec3::new(from.x, from.y, from.z);
                let to = CoreVec3::new(to.x, to.y, to.z);
                clip_projectile_blocks(&mut scene, facts, from, to).is_some()
                    || targets.iter().any(|bounds| bounds.clip(from, to).is_some())
            },
            |at| particles.add_with(&smoke, at.to_array(), [0.0; 3], false, false),
        );
    }

    pub(super) fn cbc_burst_geometry(
        &self,
        camera: DVec3,
        alpha: f64,
        right: Vec3,
        up: Vec3,
        out: &mut Vec<Vertex>,
    ) {
        let Some(remote) = &self.remote else { return };
        let Some(&rect) = self.gui.atlas.sprites.get(SHRAPNEL_SPRITE) else {
            return;
        };
        let range_sq = (f64::from(self.settings.render_distance) * 16.0).powi(2);
        let extent = (right.abs() + up.abs()) * 2.0;
        for burst in remote.create_contraptions.bursts.bursts.values() {
            // ProjectileBurstRenderer passes the parent entity's packed light.
            let sample = (burst.origin + DVec3::Y * f64::from(0.8_f32 * 0.85))
                .floor()
                .as_ivec3();
            let light = vertex_light(self.world.light(sample.x, sample.y, sample.z));
            for fragment in &burst.fragments {
                let relative = burst.at(fragment, alpha) - camera;
                let at = relative.as_vec3();
                if relative.length_squared() <= range_sq
                    && self
                        .view_frustum
                        .as_ref()
                        .is_none_or(|f| f.intersects(at - extent, at + extent))
                {
                    burst.kind.append_quad(at, right, up, rect, light, out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;

    fn sync_packet(id: i32, age: i32, fragments: &[[f32; 6]]) -> Vec<u8> {
        // RPLNetwork.writeToBuf and ClientboundSyncBurstSubProjectilesPacket.rootEncode.
        let mut packet = Writer::new();
        packet.varint(3);
        packet.varint(id);
        packet.varint(age);
        packet.varint(fragments.len() as i32);
        for fragment in fragments {
            for &component in fragment {
                packet.f32(component);
            }
        }
        packet.data
    }

    #[test]
    fn burst_sync_advances_fragments_and_extrapolates_the_next_render_position() {
        let mut state = State::default();
        state.add(302, Kind::Shrapnel, [8.0, 64.0, -3.0]);
        assert!(state.bursts[&302].fragments.is_empty());
        let payload = sync_packet(
            302,
            0,
            &[
                [0.125, -0.25, 0.0625, 1.5, 0.5, -0.25],
                [-0.0625, 0.125, 0.25, 0.00390625, 0.0, 0.0],
            ],
        );
        state.sync(&payload);
        for _ in 0..3 {
            state.step(Dim::Overworld, |_, _| false, |_| {});
        }
        let burst = &state.bursts[&302];
        assert_eq!(burst.age, 3);
        assert_eq!(burst.fragments.len(), 2);
        // Java moves by the current velocity before applying -0.025 gravity.
        // applyForces also zeroes the second fragment's initial speed (< 0.01).
        for (fragment, tick, render) in [
            (
                &burst.fragments[0],
                [12.625, 65.175, -3.6875],
                [13.375, 65.3875, -3.8125],
            ),
            (
                &burst.fragments[1],
                [7.94140625, 64.05, -2.75],
                [7.94140625, 64.0125, -2.75],
            ),
        ] {
            assert!(burst.at(fragment, 0.0).abs_diff_eq(tick.into(), 1e-12));
            assert!(burst.at(fragment, 0.5).abs_diff_eq(render.into(), 1e-12));
        }
    }

    #[test]
    fn burst_sync_replaces_fragments_and_uses_the_server_age() {
        let mut state = State::default();
        state.add(18, Kind::Shrapnel, [0.0; 3]);
        let forward = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        state.sync(&sync_packet(18, 0, &[forward; 2]));
        // ProjectileBurst sends before advancing on tickCount % 10 == 1.
        let next = sync_packet(18, 10, &[[10.0, -1.125, 0.0, 1.0, -0.25, 0.0]]);
        state.sync(&next[..next.len() - 1]);
        assert_eq!(state.bursts[&18].age, 0);
        assert_eq!(state.bursts[&18].fragments.len(), 2);
        state.sync(&next);
        assert_eq!(state.bursts[&18].age, 10);
        assert_eq!(state.bursts[&18].fragments.len(), 1);
        for _ in 0..9 {
            state.step(Dim::Overworld, |_, _| false, |_| {});
        }
        assert_eq!(state.bursts[&18].age, 19);
        state.step(Dim::Overworld, |_, _| false, |_| {});
        assert!(!state.bursts.contains_key(&18)); // shrapnel_burst.json lifetime is 20.
    }

    #[test]
    fn burst_quads_use_shrapnel_png_at_the_java_renderer_sizes() {
        let rect = Rect {
            u0: 0.1,
            v0: 0.2,
            u1: 0.6,
            v1: 0.7,
            w: 16,
            h: 16,
        };
        assert_eq!(SHRAPNEL_SPRITE, "createbigcannons:entity/shrapnel");
        for (kind, size) in [
            (Kind::Shrapnel, 1.0),
            (Kind::Flak, 1.0),
            (Kind::Grapeshot, 2.0),
        ] {
            let origin = Vec3::new(10.0, 20.0, 30.0);
            let mut out = Vec::new();
            kind.append_quad(origin, Vec3::X, Vec3::Y, rect, 123, &mut out);
            assert_eq!(out.len(), 4);
            assert_eq!(out[0].pos, origin.to_array());
            assert_eq!(
                out[2].pos,
                (origin - size * Vec3::X + size * Vec3::Y).to_array()
            );
            assert_eq!(out[0].uv, [rect.u0, rect.v1]);
            assert_eq!(out[2].uv, [rect.u1, rect.v0]);
            assert!(out.iter().all(|v| v.color == u32::MAX && v.light == 123));
        }
    }
}
