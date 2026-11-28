//! Player collision against the voxel boxes of moving plots.

use super::{SubLevel, SubLevelManager, SubLevelPose};
use crate::collisions::LookupCollisions;
use glam::DVec3;
use horizon_core::geometry::Aabb;
use horizon_world::block_collision::BlockCollisionRegistry;
use std::{collections::HashMap, io};

const SUBSTEPS: usize = 8;
const CONTACT_EPSILON: f64 = 1.0e-7;
const SUPPORT_DEPTH: f64 = 0.001;
const AXES: [DVec3; 3] = [DVec3::X, DVec3::Y, DVec3::Z];

#[derive(Default)]
pub struct DeckRider {
    pub support: Option<u128>,
    pub(crate) inherited: DVec3,
    pub(crate) advanced: bool,
}

impl DeckRider {
    pub fn begin_tick(&mut self) {
        self.advanced = false;
    }

    pub fn has_motion(&self) -> bool {
        self.support.is_some() || self.inherited != DVec3::ZERO
    }
}

pub(crate) struct DeckMovement {
    pub movement: DVec3,
    pub carry: DVec3,
    pub support: Option<u128>,
}

struct Collider<'a> {
    id: u128,
    level: &'a SubLevel,
    shapes: HashMap<[i32; 3], Vec<Aabb>>,
}

impl SubLevel {
    fn shape(&self, pos: [i32; 3], binding: &BlockCollisionRegistry<'_>) -> io::Result<Vec<Aabb>> {
        let state = self.collision_block(pos[0], pos[1], pos[2]);
        if state.0 == 0 {
            return Ok(Vec::new());
        }
        let block = |x, y, z| self.collision_block(x, y, z);
        let mut world = LookupCollisions {
            block: &block,
            binding,
        };
        let shape = binding
            .collision_shape(&mut world, state, pos)
            .map_err(io::Error::other)?;
        Ok(shape.boxes().to_vec())
    }

    fn swept_intersects(&self, min: DVec3, max: DVec3, advance: bool) -> bool {
        let lo = DVec3::from_array(self.bounds.min.map(f64::from)) - DVec3::ONE;
        let hi = DVec3::from_array(self.bounds.max.map(f64::from)) + DVec3::splat(2.0);
        let a = if advance {
            self.previous_pose
        } else {
            self.current_pose
        };
        let b = self.current_pose;
        let a_pivot = DVec3::from(a.rotation_point);
        let b_pivot = DVec3::from(b.rotation_point);
        let a_pos = DVec3::from(a.position);
        let b_pos = DVec3::from(b.position);
        // Bound the entire rotation, including points that swing outside the
        // union of the two endpoint boxes.
        let radius = (lo - a_pivot)
            .abs()
            .max((hi - a_pivot).abs())
            .max((lo - b_pivot).abs())
            .max((hi - b_pivot).abs())
            .length();
        (a_pos.min(b_pos) - radius).cmple(max).all() && (a_pos.max(b_pos) + radius).cmpge(min).all()
    }
}

impl SubLevelManager {
    pub(crate) fn collide_decks(
        &self,
        bounds: Aabb,
        movement: DVec3,
        rider: &DeckRider,
        max_step: f64,
        binding: &BlockCollisionRegistry<'_>,
    ) -> io::Result<DeckMovement> {
        self.resolve_decks(bounds, movement, rider, max_step, |level, pos| {
            level.shape(pos, binding)
        })
    }

    pub(crate) fn deck_intersects(
        &self,
        bounds: Aabb,
        advanced: bool,
        binding: &BlockCollisionRegistry<'_>,
    ) -> io::Result<bool> {
        let center = DVec3::from(bounds.center().components());
        let half = DVec3::from(bounds.size().components()) * 0.5;
        for (&id, level) in &self.levels {
            if !level.finalized || !level.swept_intersects(center - half, center + half, !advanced)
            {
                continue;
            }
            let pose = if advanced {
                level.current_pose
            } else {
                level.previous_pose
            };
            let mut collider = Collider {
                id,
                level,
                shapes: HashMap::new(),
            };
            if collider.push_out(center, half, pose, &mut |level, pos| {
                level.shape(pos, binding)
            })? != DVec3::ZERO
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn resolve_decks(
        &self,
        bounds: Aabb,
        movement: DVec3,
        rider: &DeckRider,
        max_step: f64,
        mut shape: impl FnMut(&SubLevel, [i32; 3]) -> io::Result<Vec<Aabb>>,
    ) -> io::Result<DeckMovement> {
        let advance = !rider.advanced;
        let start = DVec3::from(bounds.center().components());
        let half = DVec3::from(bounds.size().components()) * 0.5;
        let support = rider.support.filter(|id| self.levels.contains_key(id));
        let carried = support.and_then(|id| self.levels.get(&id));
        let mut colliders: Vec<_> = self
            .levels
            .iter()
            .filter(|&(id, level)| {
                level.finalized
                    && (support == Some(*id)
                        || carried.is_some()
                        || level.swept_intersects(
                            start.min(start + movement) - half - DVec3::ONE,
                            start.max(start + movement) + half + DVec3::ONE,
                            advance,
                        ))
            })
            .map(|(&id, level)| Collider {
                id,
                level,
                shapes: HashMap::new(),
            })
            .collect();
        let motion = movement;
        if colliders.is_empty() {
            return Ok(DeckMovement {
                movement: motion,
                carry: DVec3::ZERO,
                support: None,
            });
        }
        // Try the previous support first at seams between bodies.
        colliders.sort_unstable_by_key(|c| (Some(c.id) != support, c.id));
        let mut center = start;
        let mut carry = DVec3::ZERO;
        let mut correction = DVec3::ZERO;
        let mut ground = None;
        let step = motion / SUBSTEPS as f64;
        for i in 1..=SUBSTEPS {
            let alpha = i as f64 / SUBSTEPS as f64;
            if let Some(level) = carried.filter(|_| advance) {
                let previous = level.render_pose(alpha - 1.0 / SUBSTEPS as f64);
                let current = level.render_pose(alpha);
                let anchor = center - DVec3::Y * half.y;
                let delta = current.to_world(previous.to_plot(anchor)) - anchor;
                center += delta;
                carry += delta;
            }
            center += step;
            for collider in &mut colliders {
                let pose = if advance {
                    collider.level.render_pose(alpha)
                } else {
                    collider.level.logical_pose()
                };
                for _ in 0..4 {
                    let mut push = collider.push_out(center, half, pose, &mut shape)?;
                    if push.y.abs() < push.length() * 0.6
                        && push.dot(step) < 0.0
                        && let Some(rise) =
                            collider.step_up(center, half, pose, max_step, &mut shape)?
                    {
                        push = DVec3::Y * rise;
                    }
                    if push == DVec3::ZERO {
                        break;
                    }
                    center += push;
                    correction += push;
                }
            }
        }
        if motion.y <= CONTACT_EPSILON {
            for collider in &mut colliders {
                let push = collider.push_out(
                    center - DVec3::Y * SUPPORT_DEPTH,
                    half,
                    collider.level.logical_pose(),
                    &mut shape,
                )?;
                if push.try_normalize().is_some_and(|n| n.y > 0.6) {
                    ground = Some(collider.id);
                    break;
                }
            }
        }
        let movement = motion + correction;
        Ok(DeckMovement {
            movement,
            carry,
            support: ground,
        })
    }
}

impl Collider<'_> {
    fn step_up(
        &mut self,
        center: DVec3,
        half: DVec3,
        pose: SubLevelPose,
        limit: f64,
        shape: &mut impl FnMut(&SubLevel, [i32; 3]) -> io::Result<Vec<Aabb>>,
    ) -> io::Result<Option<f64>> {
        const INCREMENT: f64 = 1.0 / 16.0;
        let mut lower = 0.0;
        while lower < limit {
            let mut upper = (lower + INCREMENT).min(limit);
            let below = self.push_out(center + DVec3::Y * lower, half, pose, shape)?;
            if self.push_out(center + DVec3::Y * upper, half, pose, shape)? == DVec3::ZERO
                && below.try_normalize().is_some_and(|n| n.y > 0.8)
            {
                // Settle close enough to the surface for the support probe.
                while upper - lower > SUPPORT_DEPTH / 2.0 {
                    let mid = (lower + upper) / 2.0;
                    if self.push_out(center + DVec3::Y * mid, half, pose, shape)? == DVec3::ZERO {
                        upper = mid;
                    } else {
                        lower = mid;
                    }
                }
                return Ok(Some(upper));
            }
            lower = upper;
        }
        Ok(None)
    }

    fn push_out(
        &mut self,
        center: DVec3,
        half: DVec3,
        pose: SubLevelPose,
        shape: &mut impl FnMut(&SubLevel, [i32; 3]) -> io::Result<Vec<Aabb>>,
    ) -> io::Result<DVec3> {
        let rotation = pose.orientation();
        let axes = AXES.map(|axis| rotation.conjugate() * axis);
        let center = pose.to_plot(center);
        let extent = axes[0].abs() * half.x + axes[1].abs() * half.y + axes[2].abs() * half.z;
        let min = (center - extent).floor().as_ivec3() - glam::IVec3::ONE;
        let max = (center + extent).floor().as_ivec3() + glam::IVec3::ONE;
        let [min_x, min_y, min_z] = min.max(self.level.bounds.min.into()).to_array();
        let [max_x, max_y, max_z] = max.min(self.level.bounds.max.into()).to_array();
        let mut deepest = DVec3::ZERO;
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    let pos = [x, y, z];
                    let boxes = match self.shapes.entry(pos) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(shape(self.level, pos)?)
                        }
                    };
                    let block = DVec3::from_array(pos.map(f64::from));
                    for aabb in boxes {
                        let offset = center - (block + DVec3::from(aabb.center().components()));
                        let box_half = DVec3::from(aabb.size().components()) * 0.5;
                        if let Some(local) = separation(offset, half, axes, box_half)
                            && local.length_squared() > deepest.length_squared()
                        {
                            deepest = local;
                        }
                    }
                }
            }
        }
        let push = rotation * deepest;
        if push.y > 0.8 * push.length() {
            Ok(DVec3::Y * (push.length_squared() / push.y))
        } else {
            Ok(push)
        }
    }
}

/// The three box axes, the three player axes and their cross products are
/// the separating axes of these two boxes. Return the shortest escape vector.
fn separation(offset: DVec3, half: DVec3, axes: [DVec3; 3], box_half: DVec3) -> Option<DVec3> {
    let mut distance = f64::INFINITY;
    let mut shortest = DVec3::ZERO;
    let cross = AXES.into_iter().flat_map(|a| axes.map(|b| a.cross(b)));
    for axis in AXES.into_iter().chain(axes).chain(cross) {
        if axis.length_squared() < 1.0e-12 {
            continue;
        }
        let axis = axis.normalize();
        let radius =
            box_half.dot(axis.abs()) + half.dot(DVec3::from_array(axes.map(|a| a.dot(axis).abs())));
        let center = offset.dot(axis);
        let overlap = radius - center.abs();
        if overlap <= CONTACT_EPSILON {
            return None;
        }
        if overlap < distance {
            distance = overlap;
            shortest = axis * overlap.copysign(center);
        }
    }
    Some(shortest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chunk::Chunk,
        collisions::SceneCollisions,
        sable::{Bounds, Packet, StartTracking, plot_chunk_origin},
        world::World,
    };
    use horizon_core::geometry::Vec3;
    use horizon_world::{
        DATA_VERSION,
        collision::CachedCollisionRegistry,
        nbt::Tag,
        registry::{BlockRegistry, BlockStateId},
    };
    use std::sync::Arc;

    fn deck() -> SubLevelManager {
        let plot = (0, 0);
        let (x, z) = plot_chunk_origin(0, 0).unwrap();
        let pose = SubLevelPose {
            rotation_point: [f64::from(x * 16), 0.0, f64::from(z * 16)],
            ..SubLevelPose::default()
        };
        let mut plots = SubLevelManager::default();
        plots.handle(Packet::Start(StartTracking {
            plot,
            id: 1,
            last_pose: pose,
            pose,
            bounds: Bounds {
                min: [x * 16, 0, z * 16],
                max: [x * 16 + 2, 0, z * 16],
            },
            name: None,
            tick: 10,
        }));
        plots.handle(Packet::Finalize(plot));
        plots
    }

    fn player(feet: DVec3) -> Aabb {
        Aabb::new(
            Vec3::new(feet.x - 0.3, feet.y, feet.z - 0.3),
            Vec3::new(feet.x + 0.3, feet.y + 1.8, feet.z + 0.3),
        )
    }

    fn sweep(
        plots: &SubLevelManager,
        feet: DVec3,
        movement: DVec3,
        rider: &DeckRider,
    ) -> DeckMovement {
        plots
            .resolve_decks(player(feet), movement, rider, 0.0, |_, _| {
                Ok(vec![Aabb::unit_cube(Vec3::ZERO)])
            })
            .unwrap()
    }

    fn close(actual: DVec3, expected: DVec3) {
        assert!(
            (actual - expected).length() < 1.0e-7,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn player_box_lands_on_world_floor_and_tracked_deck() {
        let [air, stone] = [BlockStateId(0), BlockStateId(1)];
        let states = Tag::List(vec![
            Tag::String("minecraft:air".into()),
            Tag::String("minecraft:stone".into()),
        ]);
        let registry = BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(DATA_VERSION)),
            ("states", states.clone()),
            (
                "defaults",
                Tag::compound([
                    ("minecraft:air", Tag::Int(0)),
                    ("minecraft:stone", Tag::Int(1)),
                ]),
            ),
        ]))
        .unwrap();
        let shapes = [false, true].map(|full| {
            let size = usize::from(full);
            let mut coords = vec![0];
            if full {
                coords.push(1.0_f64.to_bits() as i64);
            }
            let coords = Tag::LongArray(coords);
            Tag::compound([
                ("x", coords.clone()),
                ("y", coords.clone()),
                ("z", coords),
                ("occupied", Tag::ByteArray(vec![1; size])),
                ("cube", Tag::Byte(i8::from(full))),
                ("cube_axes", Tag::Byte(if full { 7 } else { 0 })),
                ("discrete_min", Tag::IntArray(vec![0; 3])),
                ("discrete_max", Tag::IntArray(vec![size as i32; 3])),
                ("canonical", Tag::Byte(1 + i8::from(full))),
            ])
        });
        let cached = CachedCollisionRegistry::from_nbt(
            &Tag::compound([
                ("DataVersion", Tag::Int(DATA_VERSION)),
                ("states", states),
                ("shapes", Tag::List(shapes.into())),
                ("cached_shapes", Tag::IntArray(vec![0, 1])),
            ]),
            &registry,
        )
        .unwrap();
        let binding = BlockCollisionRegistry::new(Arc::new(cached)).unwrap();

        let mut world = World::new(air, 0);
        let mut floor = Chunk::empty(0, 0);
        for x in [1, 4] {
            floor.set(x, 299, 0, stone.0);
        }
        world.chunks.insert((0, 0), Arc::new(floor));

        let mut plots = deck();
        let level = plots.levels.get_mut(&1).unwrap();
        level.previous_pose.position[1] = 301.0;
        level.current_pose.position[1] = 301.0;
        let (cx, cz) = plot_chunk_origin(0, 0).unwrap();
        let pos = [cx * 16 + 1, 0, cz * 16];
        let mut column = Chunk::empty(cx, cz);
        column.set(pos[0], pos[1], pos[2], stone.0);
        assert!(plots.route_column(column, []).is_none());
        plots.predict_block(pos, air, 1);

        let land = |plots: &SubLevelManager, feet: DVec3, top, support| {
            let bounds = player(feet);
            let ship = plots
                .collide_decks(
                    bounds,
                    -4.0 * DVec3::Y,
                    &DeckRider::default(),
                    0.0,
                    &binding,
                )
                .unwrap();
            let movement = SceneCollisions {
                world: &world,
                binding: &binding,
                subject: None,
            }
            .resolve(
                bounds,
                Vec3::new(ship.movement.x, ship.movement.y, ship.movement.z),
                0.0,
                false,
            )
            .unwrap();
            close(
                feet + DVec3::from(movement.components()),
                DVec3::new(feet.x, top, feet.z),
            );
            assert_eq!(ship.support, support);
        };
        land(&plots, DVec3::new(4.5, 301.2, 0.5), 300.0, None);
        land(&plots, DVec3::new(1.5, 303.2, 0.5), 302.0, Some(1));

        plots.set_block(pos, air.0, |_| false);
        land(&plots, DVec3::new(1.5, 303.2, 0.5), 300.0, None);
    }

    #[test]
    fn feet_stay_on_the_deck_through_translation_rotation_and_walking() {
        let mut plots = deck();
        let start = DVec3::new(1.5, 1.0, 0.5);
        let gravity = DVec3::new(0.0, -0.08, 0.0);
        let stationary = sweep(&plots, start, gravity, &DeckRider::default());
        assert_eq!(stationary.support, Some(1));
        close(stationary.movement, DVec3::ZERO);
        let rider = DeckRider {
            support: stationary.support,
            ..DeckRider::default()
        };

        let shift = DVec3::new(4.0, 2.0, -3.0);
        plots.levels.get_mut(&1).unwrap().current_pose.position = shift.to_array();
        let moved = sweep(&plots, start, gravity, &rider);
        assert_eq!(moved.support, Some(1));
        close(start + moved.carry + moved.movement, start + shift);

        let ship = plots.levels.get_mut(&1).unwrap();
        ship.previous_pose = ship.current_pose;
        ship.current_pose.rotation =
            glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2).to_array();
        let turned = sweep(&plots, start + shift, gravity, &rider);
        assert_eq!(turned.support, Some(1));
        let end = DVec3::new(0.5, 1.0, -1.5) + shift;
        close(start + shift + turned.carry + turned.movement, end);
        let ship = plots.levels.get_mut(&1).unwrap();
        let halfway = ship.render_point(start + shift, end, 0.5) - shift;
        close(
            halfway,
            DVec3::new(2.0_f64.sqrt(), 1.0, -0.5 * 2.0_f64.sqrt()),
        );

        ship.previous_pose = ship.current_pose;
        let walk = DVec3::new(0.0, 0.0, -0.25);
        let walked = sweep(&plots, end, walk + gravity, &rider);
        assert_eq!(walked.support, Some(1));
        close(walked.movement, walk);
        let jumped = sweep(&plots, end, DVec3::Y * 0.42, &rider);
        assert_eq!(jumped.support, None);
    }

    #[test]
    fn moving_side_pushes_an_adjacent_player_without_mounting_them() {
        let mut plots = deck();
        // The player is outside the end of the three-block deck.
        let feet = DVec3::new(3.4, 0.25, 0.5);
        plots.levels.get_mut(&1).unwrap().current_pose.position[0] = 0.3;
        let result = sweep(&plots, feet, DVec3::ZERO, &DeckRider::default());
        close(result.movement, DVec3::X * 0.2);
        assert_eq!(result.carry, DVec3::ZERO);
        assert_eq!(result.support, None);
    }

    #[test]
    fn empty_and_distant_plots_leave_world_movement_untouched() {
        let movement = DVec3::new(-0.8, -0.08, 0.1);
        for plots in [SubLevelManager::default(), deck()] {
            let result = plots
                .resolve_decks(
                    player(DVec3::splat(100.0)),
                    movement,
                    &DeckRider::default(),
                    0.0,
                    |_, _| panic!("queried a faraway plot"),
                )
                .unwrap();
            assert_eq!(result.movement, movement);
            assert_eq!(result.carry, DVec3::ZERO);
            assert_eq!(result.support, None);
        }
    }
}
