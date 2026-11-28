//! Block targets keep Sable's backing coordinates alongside their projected hit.

use super::{Game, Hit};
use glam::DVec3;
use horizon_client::sublevel::{SubLevel, SubLevelPose};
use horizon_core::{
    geometry::{Aabb, BlockHit, Direction, Vec3, traverse_blocks},
    position::BlockPos,
};
use horizon_world::registry::BlockStateId;

#[derive(Clone, Copy)]
pub(super) struct ShipHit {
    id: u128,
    world_point: [f64; 3],
    inside: bool,
}

impl Hit {
    pub(super) fn world_point(self) -> [f64; 3] {
        self.ship.map_or(self.point, |ship| ship.world_point)
    }

    pub(super) fn inside(self) -> bool {
        self.ship.map_or(self.face == [0; 3], |ship| ship.inside)
    }
}

impl Game<'_> {
    pub(super) fn plot_level(&self, pos: [i32; 3]) -> Option<&SubLevel> {
        let plots = &self.remote.as_ref()?.sublevels;
        plots.get(plots.plot_at(DVec3::from(pos.map(f64::from)))?.1?)
    }

    pub(super) fn target_block(&self, [x, y, z]: [i32; 3]) -> BlockStateId {
        self.plot_level([x, y, z])
            .map_or_else(|| self.world.block(x, y, z), |level| level.block(x, y, z))
    }

    pub(super) fn hit_level(&self, hit: &Hit) -> Option<&SubLevel> {
        self.remote.as_ref()?.sublevels.get(hit.ship?.id)
    }

    pub(super) fn block_world_center(&self, pos: [i32; 3]) -> [f64; 3] {
        let center = DVec3::from(pos.map(|v| f64::from(v) + 0.5));
        self.plot_level(pos)
            .map_or(center, |level| level.logical_pose().to_world(center))
            .to_array()
    }

    pub(super) fn interaction_angles(&self, hit: &Hit) -> (f32, f32) {
        let Some(level) = self.hit_level(hit) else {
            return (self.yaw, self.pitch);
        };
        use horizon_core::geometry::trig::atan2_degrees;
        let view = DVec3::from(self.aim().map_or_else(
            || super::view_vector(self.yaw, self.pitch),
            |(_, direction)| direction,
        ));
        let p = level.logical_pose().orientation().conjugate() * view;
        let yaw = atan2_degrees(p.z, p.x) - 90.0;
        let pitch = -atan2_degrees(p.y, p.x.hypot(p.z));
        ((yaw + 180.0).rem_euclid(360.0) - 180.0, pitch)
    }

    pub(super) fn pick_at(&self, alpha: f64) -> Option<Hit> {
        let (origin, direction) = self.aim()?;
        let reach = self.reach();
        let terrain = self.clip_ray(origin, direction, reach);
        let Some(remote) = &self.remote else {
            return terrain;
        };
        let start = DVec3::from(origin);
        let end = start + DVec3::from(direction) * reach;
        let ships = remote.sublevels.tracked_levels().filter_map(|(id, level)| {
            if !level.finalized() {
                return None;
            }
            let pose = level.render_pose(alpha);
            let ray = PlotRay::new(pose, start, end);
            let [x, _, z] = ray.start.floor().as_ivec3().to_array();
            // Sable only clips a projected ray whose start belongs to this plot.
            if horizon_client::sable::plot_from_chunk(x >> 4, z >> 4) != Some(level.plot())
                || !ray.intersects(level.bounds())
            {
                return None;
            }
            ray.clip(id, pose, |pos| {
                let [x, y, z] = pos;
                let state = level.block(x, y, z);
                let facts = self.res.facts();
                if facts.is_air(state)
                    || matches!(
                        &*facts.get(state).name,
                        "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column"
                    )
                {
                    return Vec::new();
                }
                let outline = self.neighbour_outline_in_plot(pos, state, Some(level));
                self.outline_boxes(pos, state, outline)
            })
        });
        nearest_hit(start, terrain, ships)
    }
}

struct PlotRay {
    start: DVec3,
    end: DVec3,
}

impl PlotRay {
    fn new(pose: SubLevelPose, start: DVec3, end: DVec3) -> Self {
        Self {
            start: pose.to_plot(start),
            end: pose.to_plot(end),
        }
    }

    fn intersects(&self, bounds: horizon_client::sable::Bounds) -> bool {
        let min = DVec3::from(bounds.min.map(f64::from));
        let max = DVec3::from(bounds.max.map(f64::from)) + DVec3::ONE;
        self.start.min(self.end).cmple(max).all() && self.start.max(self.end).cmpge(min).all()
    }

    fn clip(
        &self,
        id: u128,
        pose: SubLevelPose,
        mut outline_boxes: impl FnMut([i32; 3]) -> Vec<[f64; 6]>,
    ) -> Option<Hit> {
        let start = Vec3::new(self.start.x, self.start.y, self.start.z);
        let end = Vec3::new(self.end.x, self.end.y, self.end.z);
        let delta = start.vector_to(end);
        let inside_probe = start.add_vector(delta.scale(0.001));
        let hit = traverse_blocks(start, end, 32, |block| {
            let boxes = outline_boxes([block.x, block.y, block.z]);
            let boxes = boxes.iter().copied().map(|[x0, y0, z0, x1, y1, z1]| {
                Aabb::new(Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1))
            });
            if boxes.clone().any(|b| b.contains(inside_probe)) {
                Some(BlockHit {
                    location: inside_probe,
                    direction: Direction::approximate_nearest(delta).opposite(),
                    block,
                    inside: true,
                })
            } else {
                Aabb::clip_boxes(boxes, start, end, BlockPos::new(0, 0, 0))
                    .map(|hit| BlockHit { block, ..hit })
            }
        })
        .ok()??;
        let point = hit.location.components();
        Some(Hit {
            pos: [hit.block.x, hit.block.y, hit.block.z],
            face: hit.direction.normal(),
            point,
            ship: Some(ShipHit {
                id,
                world_point: pose.to_world(DVec3::from(point)).to_array(),
                inside: hit.inside,
            }),
        })
    }
}

fn nearest_hit(
    origin: DVec3,
    terrain: Option<Hit>,
    ships: impl IntoIterator<Item = Hit>,
) -> Option<Hit> {
    let distance = |hit: Hit| origin.distance_squared(DVec3::from(hit.world_point()));
    ships.into_iter().fold(terrain, |best, hit| {
        if best.is_none_or(|best| distance(hit) < distance(best)) {
            Some(hit)
        } else {
            best
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const POS: [i32; 3] = [20_482_064, 64, 20_484_128];
    const START: DVec3 = DVec3::new(10.75, 65.25, 8.0);
    const END: DVec3 = DVec3::new(10.75, 65.25, 13.0);

    fn rotated_plot_hit() -> Hit {
        // With a 90 degree Y rotation Java's inverse maps world +Z to plot -X.
        // At world z = 9 the ray crosses the block's east face at (1, .25, .75).
        let pose = SubLevelPose {
            position: [10.0, 65.0, 10.0],
            rotation: [
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            rotation_point: POS.map(f64::from),
        };
        let [x, y, z] = POS.map(f64::from);
        PlotRay::new(pose, START, END)
            .clip(42, pose, |pos| {
                if pos == POS {
                    vec![[x, y, z, x + 1.0, y + 1.0, z + 1.0]]
                } else {
                    Vec::new()
                }
            })
            .unwrap()
    }

    #[test]
    fn rotated_ship_returns_the_plot_pos_and_face() {
        let hit = rotated_plot_hit();
        assert_eq!(hit.pos, POS);
        assert_eq!(hit.face, Direction::East.normal());
        assert_eq!(hit.point, [20_482_065.0, 64.25, 20_484_128.75]);
        assert!(DVec3::from(hit.world_point()).distance(DVec3::new(10.75, 65.25, 9.0)) < 1e-8);
        assert!(!hit.inside());
    }

    #[test]
    fn terrain_and_ships_compete_in_world_space() {
        let ship = rotated_plot_hit();
        let terrain = |z| Hit::world([10, 65, 8], Direction::North.normal(), [10.75, 65.25, z]);
        assert!(
            nearest_hit(START, Some(terrain(8.5)), [ship])
                .unwrap()
                .ship
                .is_none()
        );
        assert!(
            nearest_hit(START, Some(terrain(10.0)), [ship])
                .unwrap()
                .ship
                .is_some()
        );
        let tie = Hit::world(ship.pos, ship.face, ship.world_point());
        assert!(
            nearest_hit(START, Some(tie), [ship])
                .unwrap()
                .ship
                .is_none()
        );
    }

    #[test]
    fn break_and_use_packets_keep_the_plot_hit() {
        use horizon_protocol::{
            buf::{Reader, unpack_block_pos},
            ids::play::serverbound::{PLAYER_ACTION, USE_ITEM_ON},
            packets::play::Serverbound,
        };
        let hit = rotated_plot_hit();
        for action in [0, 2, 3] {
            let bytes = hit.destroy_packet(action, 37).encode();
            let mut wire = Reader::new(&bytes);
            assert_eq!(wire.varint().unwrap(), PLAYER_ACTION);
            assert_eq!(wire.varint().unwrap(), action);
            assert_eq!(unpack_block_pos(wire.i64().unwrap()), POS);
            assert_eq!(wire.u8().unwrap(), 5);
            assert_eq!(wire.varint().unwrap(), 37);
            assert_eq!(wire.remaining(), 0);
        }
        let bytes = Serverbound::UseItemOn(hit.use_packet(1, 38)).encode();
        let mut wire = Reader::new(&bytes);
        assert_eq!(wire.varint().unwrap(), USE_ITEM_ON);
        assert_eq!(wire.varint().unwrap(), 1);
        assert_eq!(unpack_block_pos(wire.i64().unwrap()), POS);
        assert_eq!(wire.varint().unwrap(), 5);
        assert_eq!(
            std::array::from_fn::<_, 3, _>(|_| wire.f32().unwrap()),
            [1.0, 0.25, 0.75]
        );
        assert_eq!(wire.u8().unwrap(), 0);
        assert_eq!(wire.u8().unwrap(), 0);
        assert_eq!(wire.varint().unwrap(), 38);
        assert_eq!(wire.remaining(), 0);
    }
}
