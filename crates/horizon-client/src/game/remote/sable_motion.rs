//! Sable coordinate frames at the boundary of the remote entity state.

use super::{Game, Path, Remote, Vec3};
use glam::{DQuat, DVec3};
use horizon_client::{
    sable::{Packet, Plot},
    sublevel::{SubLevel, SubLevelManager},
};

pub(super) struct EntityDeck {
    pub(super) wire_position: [f64; 3],
    plot: Plot,
    pub(super) id: Option<u128>,
    resident: bool,
    retired: bool,
    local: Option<DVec3>,
    previous_local: Option<DVec3>,
    remaining: u8,
}

enum EntityFrameUpdate {
    Terrain,
    PlotMob,
    PlotObject([f64; 3]),
}

impl EntityDeck {
    pub(super) fn new(
        wire_position: [f64; 3],
        plot: Plot,
        id: Option<u128>,
        resident: bool,
        local: Option<DVec3>,
    ) -> Self {
        Self {
            wire_position,
            plot,
            id,
            resident,
            retired: false,
            local,
            previous_local: local,
            remaining: 3,
        }
    }

    fn retarget(
        old: Option<Self>,
        levels: &SubLevelManager,
        position: [f64; 3],
        resident: bool,
        world: impl FnOnce() -> DVec3,
    ) -> (Option<Self>, bool) {
        let Some((plot, id)) = levels.plot_at(position.into()) else {
            return (None, old.is_none());
        };
        let same_frame = old
            .as_ref()
            .is_some_and(|old| !old.retired && old.plot == plot && old.id == id);
        let local = if same_frame {
            old.as_ref().and_then(|old| old.local)
        } else {
            id.and_then(|id| levels.get(id)).map(|level| {
                let world = world();
                if levels.plot_at(world).is_some() {
                    DVec3::from(position)
                } else {
                    level.logical_pose().to_plot(world)
                }
            })
        };
        let mut next = Self::new(position, plot, id, resident, local);
        if same_frame {
            next.previous_local = old.and_then(|old| old.previous_local);
        }
        (Some(next), same_frame)
    }

    pub(super) fn advance<'a>(
        &mut self,
        levels: &'a SubLevelManager,
        world: DVec3,
    ) -> Option<(&'a SubLevel, [DVec3; 2])> {
        if self.retired {
            return None;
        }
        let target = DVec3::from(self.wire_position);
        if self.id.is_none() {
            self.id = levels
                .plot_at(target)
                .and_then(|(plot, id)| (plot == self.plot).then_some(id).flatten());
        }
        let level = levels.get(self.id?)?;
        let local = self.local.get_or_insert_with(|| {
            if levels.plot_at(world).is_some() {
                target
            } else {
                level.previous_pose().to_plot(world)
            }
        });
        let previous = *local;
        if self.remaining != 0 {
            *local = local.lerp(target, 1.0 / f64::from(self.remaining));
            self.remaining -= 1;
        }
        self.previous_local = Some(previous);
        Some((level, [previous, *local]))
    }

    fn retire(&mut self) {
        // The client loses the pose, but the next short delta still starts
        // from the last packet position, even while the plot is out of view.
        self.retired = true;
        self.id = None;
        self.local = None;
        self.previous_local = None;
    }

    fn waiting_for_vehicle(&self) -> bool {
        self.resident && !self.retired && self.id.is_none()
    }

    fn retire_for_tracking_change(&mut self, packet: &Packet) -> bool {
        let invalidated = match packet {
            Packet::Start(start) => {
                self.id.is_some() && (self.plot == start.plot || self.id == Some(start.id))
            }
            Packet::StopTracking(plot) => self.plot == *plot,
            _ => false,
        };
        if invalidated {
            self.retire();
        }
        invalidated
    }
}

fn passenger_feet(rotation: DQuat, origin: DVec3, offset: DVec3, eye: DVec3) -> DVec3 {
    origin + rotation * (offset + eye) - eye
}

fn ride_view_rotation(old: Option<(u128, DQuat)>, next: Option<(u128, DQuat)>) -> Option<DQuat> {
    if old.map(|(id, _)| id) == next.map(|(id, _)| id) {
        return None;
    }
    let from = old.map_or(DQuat::IDENTITY, |(_, rotation)| rotation);
    let to = next.map_or(DQuat::IDENTITY, |(_, rotation)| rotation);
    Some(to.conjugate() * from)
}

fn mob_render_transform(
    level: &SubLevel,
    [previous, current]: [DVec3; 2],
    eye_height: f64,
    rotates: bool,
    alpha: f64,
) -> (DVec3, DVec3, DQuat) {
    let eye = DVec3::Y * eye_height;
    let previous = previous + eye;
    let current = current + eye;
    (
        previous.lerp(current, alpha),
        level.render_point(previous, current, alpha),
        if rotates {
            level.render_pose(alpha).orientation()
        } else {
            DQuat::IDENTITY
        },
    )
}

pub(super) fn world_vec(p: DVec3) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

impl Remote {
    pub(super) fn clear_sable_level(&mut self) {
        if self
            .destroying
            .is_some_and(|(pos, _, _)| self.sublevels.is_backing_column(pos[0] >> 4, pos[2] >> 4))
        {
            self.destroying = None;
        }
        self.sublevels.clear_level();
        for id in self.entity_decks.keys() {
            self.lerps.remove(id);
            self.stepped.remove(id);
        }
        self.entity_decks.clear();
        self.ride_frame = None;
        self.ride_position_reset = false;
        self.deck_rider = Default::default();
        self.deck_freeze = None;
        self.deck_correction = None;
    }

    pub(in crate::game) fn retire_passenger_deck(&mut self, id: i32) {
        if let Some(deck) = self.entity_decks.get_mut(&id) {
            deck.retire();
        }
    }

    pub(super) fn entity_wire_position(&self, id: i32) -> Option<[f64; 3]> {
        self.entity_decks
            .get(&id)
            .map(|d| d.wire_position)
            .or_else(|| self.create_contraptions.projectiles.packet_position(id))
            .or_else(|| self.create_contraptions.bursts.packet_position(id))
            .or_else(|| self.entities.get(&id).map(|t| t.position))
    }

    pub(super) fn player_wire_position(&self, world: Vec3) -> [f64; 3] {
        let world = DVec3::from(world.components());
        self.deck_rider
            .support
            .and_then(|id| self.sublevels.get(id))
            .map_or(world, |level| level.logical_pose().to_plot(world))
            .to_array()
    }
}

impl Game<'_> {
    pub(super) fn clear_sable_level(&mut self) {
        if self
            .riding
            .is_some_and(|id| self.resident_deck(id).is_some() || self.waiting_for_plot_vehicle(id))
            || self.remote.as_ref().is_some_and(|r| r.ride_frame.is_some())
        {
            self.riding = None;
            self.sync_ride_view();
        }
        if let Some(remote) = self.remote.as_mut() {
            for mob in &mut self.mobs {
                if mob.extra.mount.is_some_and(|id| {
                    remote
                        .entity_decks
                        .get(&(id as i32))
                        .is_some_and(|d| d.resident)
                }) {
                    mob.extra.mount = None;
                    mob.extra.passenger = false;
                }
            }
            remote.clear_sable_level();
        }
    }

    pub(super) fn handle_sable_packet(&mut self, packet: horizon_client::sable::Packet) {
        use horizon_client::sable::plot_from_chunk;

        let Some(remote) = self.remote.as_mut() else {
            return;
        };
        let replaced = match &packet {
            Packet::Start(start) => Some(start.plot),
            Packet::StopTracking(plot) => Some(*plot),
            _ => None,
        };
        if let Some(plot) = replaced {
            let in_plot = |pos: [i32; 3]| plot_from_chunk(pos[0] >> 4, pos[2] >> 4) == Some(plot);
            if remote.destroying.is_some_and(|(pos, _, _)| in_plot(pos)) {
                remote.destroying = None;
                self.mining = None;
            }
            self.destroying.retain(|_, (pos, _, _)| !in_plot(*pos));
            for (&id, deck) in &mut remote.entity_decks {
                if deck.retire_for_tracking_change(&packet) {
                    remote.lerps.remove(&id);
                    remote.stepped.remove(&id);
                    if deck.resident && self.riding == Some(id as u32) {
                        self.riding = None;
                    }
                }
            }
        }
        match packet {
            Packet::FreezePlayer(id, anchor) => {
                let anchor = DVec3::from(anchor);
                if anchor.is_finite() {
                    remote.deck_freeze = Some((id, anchor, 81));
                }
            }
            packet => {
                remote.sublevels.handle(packet);
                remote.deck_rider.support = remote
                    .deck_rider
                    .support
                    .filter(|&id| remote.sublevels.get(id).is_some());
            }
        }
        self.sync_ride_view();
    }

    pub(super) fn move_entity_in_frame(
        &mut self,
        id: i32,
        path: Option<Path>,
        rotation: Option<(f32, f32)>,
        on_ground: bool,
        resident: bool,
    ) {
        let path = if let Some(path) = path {
            let update = self.frame_entity_position(id, path.end(), resident);
            if self.riding == Some(id as u32) {
                self.sync_ride_view();
            }
            match update {
                Some(EntityFrameUpdate::Terrain) => Some(path),
                Some(EntityFrameUpdate::PlotMob) => None,
                Some(EntityFrameUpdate::PlotObject(at)) => Some(Path::Linear(at)),
                None => return,
            }
        } else {
            None
        };
        self.move_entity(id, path, rotation, on_ground, false);
    }

    fn frame_entity_position(
        &mut self,
        id: i32,
        position: [f64; 3],
        resident: bool,
    ) -> Option<EntityFrameUpdate> {
        let remote = self.remote.as_mut()?;
        if !remote.sublevels.is_enabled() {
            return Some(EntityFrameUpdate::Terrain);
        }
        let tracked = remote.entities.get(&id).map_or(position, |t| t.position);
        let (deck, same_frame) = EntityDeck::retarget(
            remote.entity_decks.remove(&id),
            &remote.sublevels,
            position,
            resident,
            || {
                self.mobs
                    .iter()
                    .find(|m| m.id == id as u32)
                    .map(|m| m.position)
                    .or_else(|| {
                        self.boats
                            .iter()
                            .find(|b| b.id == id as u32)
                            .map(|b| b.position)
                    })
                    .or_else(|| {
                        self.carts
                            .iter()
                            .find(|c| c.id == id as u32)
                            .map(|c| c.position)
                    })
                    .map_or_else(|| tracked.into(), |world| world.components().into())
            },
        );
        if !same_frame {
            remote.stepped.remove(&id);
            remote.lerps.remove(&id);
        }
        let Some(deck) = deck else {
            return Some(EntityFrameUpdate::Terrain);
        };
        let ship = deck.id;
        remote.entity_decks.insert(id, deck);
        let level = remote.sublevels.get(ship?)?;
        if self.mobs.iter().any(|m| m.id == id as u32) {
            Some(EntityFrameUpdate::PlotMob)
        } else {
            Some(EntityFrameUpdate::PlotObject(
                level.logical_pose().to_world(position.into()).to_array(),
            ))
        }
    }

    pub(in crate::game) fn plot_mob_render(
        &self,
        mob: &horizon_client::mobs::Mob,
        alpha: f64,
    ) -> Option<(DVec3, DVec3, DQuat)> {
        let remote = self.remote.as_ref()?;
        let (level, eye_height, rotates) = if let Some(vehicle) = mob.extra.mount {
            (self.resident_deck(vehicle)?, mob.eye_height(), true)
        } else {
            let deck = remote.entity_decks.get(&(mob.id as i32))?;
            let level = remote.sublevels.get(deck.id?)?;
            let eye_height = if deck.resident { mob.eye_height() } else { 0.0 };
            (level, eye_height, deck.resident)
        };
        Some(mob_render_transform(
            level,
            [
                mob.previous.components().into(),
                mob.position.components().into(),
            ],
            eye_height,
            rotates,
            alpha,
        ))
    }

    pub(in crate::game) fn correct_plot_vehicle(
        &mut self,
        id: u32,
        position: [f64; 3],
    ) -> Option<[f64; 3]> {
        let remote = self.remote.as_mut()?;
        let point = DVec3::from(position);
        let Some((plot, ship)) = remote.sublevels.plot_at(point) else {
            remote.entity_decks.remove(&(id as i32));
            return Some(position);
        };
        remote.entity_decks.insert(
            id as i32,
            EntityDeck::new(position, plot, ship, true, Some(point)),
        );
        Some(
            remote
                .sublevels
                .get(ship?)?
                .logical_pose()
                .to_world(point)
                .to_array(),
        )
    }

    pub(in crate::game) fn sync_ride_view(&mut self) {
        let next = self.riding.and_then(|vehicle| {
            let id = self
                .remote
                .as_ref()?
                .entity_decks
                .get(&(vehicle as i32))?
                .id?;
            Some((
                id,
                self.resident_deck(vehicle)?.logical_pose().orientation(),
            ))
        });
        let Some(remote) = self.remote.as_mut() else {
            return;
        };
        let old = std::mem::replace(&mut remote.ride_frame, next);
        if let Some(rotation) = ride_view_rotation(old, next) {
            remote.ride_position_reset = next.is_some();
            if next.is_some() {
                remote.deck_rider = Default::default();
            }
            self.rotate_ride_view(rotation);
        }
    }

    /// LocalPlayer.startRiding resets the old seat position in the vehicle's
    /// frame so the first rendered frame does not interpolate from terrain.
    pub(super) fn finish_plot_mount(&mut self) {
        if !self
            .remote
            .as_mut()
            .is_some_and(|remote| std::mem::take(&mut remote.ride_position_reset))
        {
            return;
        }
        if let Some((level, player)) = self.player_deck().zip(self.player.as_ref()) {
            let eye = DVec3::Y * f64::from(player.dimensions.eye_height);
            let at = DVec3::from(player.position.components()) + eye;
            self.previous_position = world_vec(
                level
                    .previous_pose()
                    .to_world(level.logical_pose().to_plot(at))
                    - eye,
            );
        }
    }

    fn rotate_ride_view(&mut self, rotation: glam::DQuat) {
        let view = DVec3::from(Vec3::direction_from_rotation(self.pitch, self.yaw).components());
        (self.yaw, self.pitch) = super::look_at_rotation([0.0; 3], (rotation * view).to_array());
    }

    pub(in crate::game) fn resident_deck(&self, vehicle: u32) -> Option<&SubLevel> {
        let remote = self.remote.as_ref()?;
        let deck = remote
            .entity_decks
            .get(&(vehicle as i32))
            .filter(|deck| deck.resident)?;
        remote.sublevels.get(deck.id?)
    }

    pub(in crate::game) fn waiting_for_plot_vehicle(&self, id: u32) -> bool {
        self.remote
            .as_ref()
            .and_then(|remote| remote.entity_decks.get(&(id as i32)))
            .is_some_and(EntityDeck::waiting_for_vehicle)
    }

    pub(in crate::game) fn player_deck(&self) -> Option<&SubLevel> {
        if let Some(vehicle) = self.riding {
            return self.resident_deck(vehicle);
        }
        let remote = self.remote.as_ref()?;
        remote.sublevels.get(remote.deck_rider.support?)
    }

    pub(super) fn resolve_deck_spawn(&mut self) -> bool {
        let Some(remote) = self.remote.as_mut() else {
            return false;
        };
        let Some((id, anchor, remaining)) = remote.deck_freeze.as_mut() else {
            let Some((point, remaining)) = remote.deck_correction.as_mut() else {
                return false;
            };
            if *remaining == 0 {
                remote.deck_correction = None;
                return false;
            }
            *remaining -= 1;
            let Some(level) = remote
                .sublevels
                .plot_at(*point)
                .and_then(|(_, id)| remote.sublevels.get(id?))
            else {
                return true;
            };
            let Some(player) = self.player.as_mut() else {
                return true;
            };
            player.position = world_vec(level.logical_pose().to_world(*point));
            self.previous_position = player.position;
            remote.deck_correction = None;
            return false;
        };
        if *remaining == 0 {
            remote.deck_freeze = None;
            return false;
        }
        *remaining -= 1;
        let Some(level) = remote.sublevels.get(*id).filter(|level| level.finalized()) else {
            return true;
        };
        let Some(player) = self.player.as_mut() else {
            return true;
        };
        player.position = world_vec(level.previous_pose().to_world(*anchor));
        player.velocity = Vec3::ZERO;
        self.previous_position = player.position;
        remote.deck_rider = Default::default();
        remote.deck_rider.support = Some(*id);
        remote.deck_freeze = None;
        remote.deck_correction = None;
        false
    }

    /// Publish the projected positions of plot-resident vehicles and custom
    /// seats before the existing passenger placement runs.
    pub(super) fn tick_plot_vehicles(&mut self) {
        let Some(remote) = self.remote.as_mut() else {
            return;
        };
        for (&id, deck) in &mut remote.entity_decks {
            if !deck.resident {
                continue;
            }
            let Some(tracked) = remote.entities.get_mut(&id) else {
                continue;
            };
            if self.mobs.iter().any(|m| m.id == id as u32) {
                continue;
            }
            let Some((level, [previous, current])) =
                deck.advance(&remote.sublevels, tracked.position.into())
            else {
                continue;
            };
            let at = level.logical_pose().to_world(current);
            tracked.position = at.to_array();
            let previous = world_vec(level.previous_pose().to_world(previous));
            if let Some(display) = self.text_displays.get_mut(&id)
                && display.vehicle.is_none()
            {
                display.ride(at);
            }
            for item in self.dropped.iter_mut().filter(|item| item.id == id as u32) {
                item.previous = previous;
                item.position = world_vec(at);
            }
            for orb in self.orbs.iter_mut().filter(|orb| orb.id == id as u32) {
                orb.previous = previous;
                orb.position = world_vec(at);
            }
            for boat in self.boats.iter_mut().filter(|b| b.id == id as u32) {
                boat.previous = previous;
                boat.position = world_vec(at);
            }
            for cart in self.carts.iter_mut().filter(|c| c.id == id as u32) {
                cart.previous = previous;
                cart.position = world_vec(at);
            }
            for seat in self.cushions.iter_mut().filter(|c| c.id == id as u32) {
                seat.position = world_vec(at);
            }
            for stand in self.armor_stands.iter_mut().filter(|a| a.id == id as u32) {
                stand.position = world_vec(at);
            }
        }
        self.sync_ride_view();
    }

    // Vanilla has already placed the passenger relative to its vehicle.
    // Rotate that local offset and the passenger's eye about the vehicle.
    pub(in crate::game) fn plot_passenger_feet(
        &self,
        vehicle: u32,
        seats: [Vec3; 2],
        origins: [Vec3; 2],
        eye_height: f64,
    ) -> [Vec3; 2] {
        let Some(deck) = self.resident_deck(vehicle) else {
            return seats;
        };
        let eye = DVec3::Y * eye_height;
        let poses = [deck.previous_pose(), deck.logical_pose()];
        std::array::from_fn(|i| {
            let offset = DVec3::from(seats[i].components()) - DVec3::from(origins[i].components());
            world_vec(passenger_feet(
                poses[i].orientation(),
                origins[i].components().into(),
                offset,
                eye,
            ))
        })
    }

    pub(in crate::game) fn plot_object_pose(
        &self,
        id: u32,
        alpha: f64,
    ) -> Option<(DVec3, glam::Quat)> {
        let remote = self.remote.as_ref()?;
        let deck = remote
            .entity_decks
            .get(&(id as i32))
            .filter(|deck| deck.resident)?;
        let level = remote.sublevels.get(deck.id?)?;
        let current = deck.local?;
        let previous = deck.previous_local.unwrap_or(current);
        let pose = level.render_pose(alpha);
        Some((
            pose.to_world(previous.lerp(current, alpha)),
            pose.orientation().as_quat(),
        ))
    }

    pub(in crate::game) fn plot_vehicle_seat(&self, id: u32) -> Option<(Vec3, Vec3)> {
        let remote = self.remote.as_ref()?;
        let deck = remote
            .entity_decks
            .get(&(id as i32))
            .filter(|deck| deck.resident)?;
        let level = remote.sublevels.get(deck.id?)?;
        let local = deck.local?;
        Some((
            world_vec(
                level
                    .previous_pose()
                    .to_world(deck.previous_local.unwrap_or(local)),
            ),
            world_vec(level.logical_pose().to_world(local)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::{
        sable::{Bounds, SnapshotInfo, StartTracking, plot_chunk_origin},
        sublevel::SubLevelPose,
    };
    use horizon_protocol::packets::play::VecDelta;
    use std::f64::consts::FRAC_1_SQRT_2;

    // Poses and points are inputs. The plot is in Sable's container, with
    // storage origin chosen by SubLevelContainer's backing grid.
    fn ship(plot: Plot, id: u128) -> StartTracking {
        let (x, z) = plot_chunk_origin(plot.0, plot.1).unwrap();
        let pivot = [f64::from(x * 16), 64.0, f64::from(z * 16)];
        let last_pose = SubLevelPose {
            position: [10.0, 64.0, 20.0],
            rotation_point: pivot,
            ..Default::default()
        };
        let half = FRAC_1_SQRT_2 as f32;
        StartTracking {
            id,
            plot,
            last_pose,
            pose: SubLevelPose {
                position: [14.0, 66.0, 18.0],
                rotation: [half, 0.0, 0.0, half],
                ..last_pose
            },
            bounds: Bounds {
                min: [x * 16, 64, z * 16],
                max: [x * 16 + 15, 67, z * 16 + 15],
            },
            name: None,
            tick: 2,
        }
    }

    fn settle_at_next_pose(levels: &mut SubLevelManager, plot: Plot) {
        levels.handle(Packet::StopMoving(plot));
        levels.handle(Packet::SnapshotInfo(SnapshotInfo {
            elapsed_ms: 50,
            tick: 3,
            stopped: true,
        }));
        levels.tick(20.0);
    }

    fn near(actual: DVec3, expected: [f64; 3]) {
        assert!(
            (actual - DVec3::from(expected)).length() < 1e-6,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn sable_phase_5c_living_mob_keeps_its_wire_base_and_renders_upright() {
        let start = ship((0, 0), 7);
        let plot = start.plot;
        let spawn = DVec3::from(start.pose.rotation_point) + DVec3::new(2.0, 0.0, 2.0);
        let mut levels = SubLevelManager::default();
        levels.enable();
        let world = DVec3::new(12.0, 64.0, 22.0);
        let (deck, _) = EntityDeck::retarget(None, &levels, spawn.to_array(), false, || world);
        let mut deck = deck.unwrap();
        assert!(deck.advance(&levels, world).is_none());
        levels.handle(Packet::Start(start.clone()));
        let (_, [_, current]) = deck.advance(&levels, world).unwrap();
        assert_eq!(current, spawn);

        // VecDeltaCodec's 1/4096 units and LivingEntityMixin's 1/3 first
        // step turn the chosen 3-block target into one block of local travel.
        let target = VecDelta::apply(deck.wire_position, [12288, 0, 0]);
        let (deck, same_frame) = EntityDeck::retarget(Some(deck), &levels, target, false, || world);
        assert!(same_frame);
        let mut deck = deck.unwrap();
        settle_at_next_pose(&mut levels, plot);
        let (level, [previous, current]) = deck.advance(&levels, world).unwrap();
        let feet = [
            level.previous_pose().to_world(previous),
            level.logical_pose().to_world(current),
        ];
        near(feet[0], [12.0, 64.0, 22.0]);
        near(feet[1], [17.0, 64.0, 18.0]);
        let (_, rendered_feet, rotation) = mob_render_transform(level, feet, 0.0, false, 0.5);
        near(
            rendered_feet,
            [14.5, 65.0 - 2.0 * FRAC_1_SQRT_2, 19.0 + 2.0 * FRAC_1_SQRT_2],
        );
        assert_eq!(rotation, DQuat::IDENTITY);
        assert_eq!(deck.wire_position, target);

        // Entering another plot carries the displayed position through world
        // space first. The new target is only the wire base until the lerp.
        let other = ship((1, 0), 8);
        let pivot = DVec3::from(other.last_pose.rotation_point);
        levels.handle(Packet::Start(other));
        let next = pivot + DVec3::new(1.0, 0.0, 1.0);
        let (transferred, same_frame) =
            EntityDeck::retarget(Some(deck), &levels, next.to_array(), false, || feet[1]);
        assert!(!same_frame);
        let transferred = transferred.unwrap();
        near(transferred.local.unwrap() - pivot, [7.0, 0.0, -2.0]);
        assert_eq!(transferred.wire_position, next.to_array());
    }

    #[test]
    fn sable_phase_5c_passenger_rotates_about_its_eye_and_detaches_to_terrain() {
        let start = ship((0, 0), 7);
        let plot = start.plot;
        let local = DVec3::from(start.pose.rotation_point) + DVec3::new(2.0, 0.0, 2.0);
        let mut levels = SubLevelManager::default();
        levels.handle(Packet::Start(start.clone()));
        let mut deck = EntityDeck::new(local.to_array(), plot, Some(7), true, Some(local));
        settle_at_next_pose(&mut levels, plot);
        let (level, [previous, current]) = deck.advance(&levels, DVec3::ZERO).unwrap();
        let poses = [level.previous_pose(), level.logical_pose()];
        let eye = DVec3::Y * 1.5;
        let offset = DVec3::new(0.75, 0.5, 0.25);
        // EntityRidingSubLevelVehicleHelper projects seat + eye and then
        // subtracts the vertical eye offset. Offset and eye height are inputs.
        let feet = std::array::from_fn(|i| {
            let pose = poses[i];
            let origin = pose.to_world([previous, current][i]);
            passenger_feet(pose.orientation(), origin, offset, eye)
        });
        near(feet[0], [12.75, 64.5, 22.25]);
        near(feet[1], [16.75, 62.25, 20.0]);
        let (_, render_eye, rotation) = mob_render_transform(level, feet, 1.5, true, 0.5);
        near(
            render_eye,
            [
                14.75,
                65.0 - 0.25 * FRAC_1_SQRT_2,
                19.0 + 4.25 * FRAC_1_SQRT_2,
            ],
        );
        near(rotation * DVec3::Y, [0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2]);

        // LocalPlayerMixin converts the view direction when boarding and
        // removing a vehicle; the latter uses the current pose only once.
        let old = Some((7, poses[0].orientation()));
        let new = Some((7, poses[1].orientation()));
        let view = ride_view_rotation(None, old).unwrap() * DVec3::Z;
        assert!(ride_view_rotation(old, new).is_none());
        near(
            ride_view_rotation(new, None).unwrap() * view,
            [0.0, -1.0, 0.0],
        );
        assert!(ride_view_rotation(None, None).is_none());

        let stop = Packet::StopTracking(plot);
        let pending = EntityDeck::new(local.to_array(), plot, None, true, None);
        assert!(pending.waiting_for_vehicle());
        assert!(deck.retire_for_tracking_change(&stop));
        assert!(!deck.waiting_for_vehicle());
        levels.handle(stop);
        levels.handle(Packet::Start(StartTracking { id: 8, ..start }));
        assert!(deck.advance(&levels, feet[1]).is_none());
        assert_eq!(deck.wire_position, local.to_array());
        // The following terrain TeleportEntity is absolute in 767. The
        // first terrain delta uses that position, independently of the seat.
        let terrain = [16.0, 63.0, 22.0];
        let (deck, same_frame) =
            EntityDeck::retarget(Some(deck), &levels, terrain, false, || feet[1]);
        assert!(deck.is_none());
        assert!(!same_frame);
        let base = deck.map_or(terrain, |deck| deck.wire_position);
        let next = VecDelta::apply(base, [2048, 0, -4096]);
        assert_eq!(next, [16.5, 63.0, 21.0]);
        assert!(levels.plot_at(next.into()).is_none());
        levels.clear_level();
        assert!(levels.get(8).is_none());
    }
}
