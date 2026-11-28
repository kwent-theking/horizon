//! Client-owned Sable plots, separate from the ordinary world and its streamer.

use crate::{
    chunk::{Chunk, MIN_Y, SECTIONS},
    mesher::{MeshScratch, Mesher, SectionMesh},
    sable::{self, Bounds, Packet, Plot, SableSnapshot, SnapshotInfo, StartTracking},
    streaming::{Streaming, WorkerContext},
};
use glam::{DMat4, DQuat, DVec3};
use horizon_world::nbt::Tag;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

const INTERPOLATION_DELAY: f64 = 1.5;
const HISTORY_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubLevelPose {
    pub position: [f64; 3],
    /// Quaternion in XYZW order.
    pub rotation: [f32; 4],
    /// Pivot in absolute backing block coordinates.
    pub rotation_point: [f64; 3],
}

impl Default for SubLevelPose {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_point: [0.0; 3],
        }
    }
}

impl SubLevelPose {
    fn orientation(self) -> DQuat {
        let [x, y, z, w] = self.rotation.map(f64::from);
        DQuat::from_xyzw(x, y, z, w).normalize()
    }

    fn interpolate(self, to: Self, t: f64) -> Self {
        Self {
            position: DVec3::from(self.position)
                .lerp(DVec3::from(to.position), t)
                .to_array(),
            rotation: self
                .orientation()
                .slerp(to.orientation(), t)
                .to_array()
                .map(|v| v as f32),
            rotation_point: DVec3::from(self.rotation_point)
                .lerp(DVec3::from(to.rotation_point), t)
                .to_array(),
        }
    }

    /// Map section-local vertices to camera-relative world space. Subtract the
    /// absolute pivot before rotation, while both coordinates are still f64.
    pub fn model_matrix(self, section_origin: DVec3, camera: DVec3) -> DMat4 {
        DMat4::from_rotation_translation(self.orientation(), DVec3::from(self.position) - camera)
            * DMat4::from_translation(section_origin - DVec3::from(self.rotation_point))
    }
}

struct PlotColumn {
    chunk: Chunk,
    block_entities: HashMap<[i32; 3], (i32, Tag)>,
    meshes: Vec<(i32, SectionMesh)>,
}

pub struct SubLevel {
    plot: Plot,
    origin: (i32, i32),
    bounds: Bounds,
    pub name: Option<String>,
    finalized: bool,
    stopped: bool,
    history: VecDeque<(i32, SubLevelPose)>,
    previous_pose: SubLevelPose,
    current_pose: SubLevelPose,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    columns: HashMap<(i32, i32), PlotColumn>,
    dirty: HashSet<(i32, i32)>,
}

impl SubLevel {
    fn new(start: StartTracking, origin: (i32, i32)) -> Self {
        let mut history = VecDeque::new();
        if let Some(previous) = start.tick.checked_sub(1) {
            history.push_back((previous, start.last_pose));
        }
        history.push_back((start.tick, start.pose));
        let initial_pose = SubLevelPose {
            rotation_point: start.pose.rotation_point,
            ..start.last_pose
        };
        Self {
            plot: start.plot,
            origin,
            bounds: start.bounds,
            name: start.name,
            finalized: false,
            stopped: false,
            history,
            previous_pose: initial_pose,
            current_pose: initial_pose,
            linear_velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            columns: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    fn sample(&self, tick: f64) -> SubLevelPose {
        let Some(&(first, first_pose)) = self.history.front() else {
            return self.current_pose;
        };
        let (mut before_tick, mut before) = (f64::from(first), first_pose);
        if tick <= before_tick {
            return before;
        }
        let mut prior = None;
        for &(after_tick, after) in self.history.iter().skip(1) {
            let after_tick = f64::from(after_tick);
            if tick <= after_tick {
                return before
                    .interpolate(after, (tick - before_tick) / (after_tick - before_tick));
            }
            prior = Some((before_tick, before));
            (before_tick, before) = (after_tick, after);
        }
        if !self.stopped
            && let Some((prior_tick, prior)) = prior
        {
            return prior.interpolate(
                before,
                1.0 + (tick - before_tick).min(1.0) / (before_tick - prior_tick),
            );
        }
        before
    }

    pub fn render_pose(&self, alpha: f64) -> SubLevelPose {
        self.previous_pose
            .interpolate(self.current_pose, alpha.clamp(0.0, 1.0))
    }

    pub fn meshes(&self) -> impl Iterator<Item = (DVec3, &SectionMesh)> {
        self.columns
            .values()
            .filter(move |_| self.finalized)
            .flat_map(|column| {
                column.meshes.iter().map(move |(sy, mesh)| {
                    let origin = DVec3::new(
                        f64::from(column.chunk.x * 16),
                        f64::from(MIN_Y + sy * 16),
                        f64::from(column.chunk.z * 16),
                    );
                    (origin, mesh)
                })
            })
    }

    fn local_column(&self, x: i32, z: i32) -> (i32, i32) {
        (x - self.origin.0, z - self.origin.1)
    }

    fn mark_neighbors(&mut self, (x, z): (i32, i32)) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbor = (x + dx, z + dz);
                if self.columns.contains_key(&neighbor) {
                    self.dirty.insert(neighbor);
                }
            }
        }
    }

    fn rebuild(&mut self, mesher: &mut Mesher<'_>, scratch: &mut MeshScratch) {
        if !self.finalized {
            return;
        }
        for (x, z) in self.dirty.drain() {
            if !self.columns.contains_key(&(x, z)) {
                continue;
            }
            let region = std::array::from_fn(|dz| {
                std::array::from_fn(|dx| {
                    self.columns
                        .get(&(x + dx as i32 - 1, z + dz as i32 - 1))
                        .map(|c| &c.chunk)
                })
            });
            let center = region[1][1].expect("plot column");
            let mut meshes = Vec::new();
            for sy in 0..SECTIONS {
                if !self
                    .bounds
                    .contains_section([center.x, (MIN_Y >> 4) + sy as i32, center.z])
                    || center.sections[sy].is_uniform(0)
                {
                    continue;
                }
                let mut mesh = SectionMesh::default();
                mesher.mesh_plot_into(&region, sy as i32, scratch, &mut mesh);
                if !mesh.solid.is_empty() || !mesh.translucent.is_empty() {
                    meshes.push((sy as i32, mesh));
                }
            }
            self.columns.get_mut(&(x, z)).expect("plot column").meshes = meshes;
        }
    }
}

impl Bounds {
    fn contains_section(self, section: [i32; 3]) -> bool {
        self.min
            .into_iter()
            .zip(self.max)
            .zip(section)
            .all(|((min, max), coord)| min <= max && (min >> 4..=max >> 4).contains(&coord))
    }
}

#[derive(Default)]
struct SnapshotClock {
    info: Option<SnapshotInfo>,
    tick: f64,
    speed: f64,
    last_sample: f64,
    sample: f64,
}

impl SnapshotClock {
    fn receive(&mut self, info: SnapshotInfo) {
        if self.info.is_some_and(|old| info.tick < old.tick) {
            return;
        }
        if self.info.is_none_or(|old| old.stopped && !info.stopped) {
            self.tick = f64::from(info.tick);
            self.speed = 1.0;
            self.sample = self.tick - INTERPOLATION_DELAY;
            self.last_sample = self.sample;
        }
        self.info = Some(info);
    }

    fn advance(&mut self, tick_rate: f32) -> Option<f64> {
        let info = self.info?;
        if !info.stopped {
            let measured = 1000.0 / f64::from(tick_rate) / f64::from(info.elapsed_ms.max(1));
            self.speed += (measured - self.speed) * 0.05;
        }
        let latest = f64::from(info.tick);
        self.tick = (self.tick + self.speed).clamp(latest - INTERPOLATION_DELAY, latest + 1.5);
        self.last_sample = self.sample;
        self.sample = self.tick - INTERPOLATION_DELAY;
        Some(self.sample)
    }
}

/// One live dimension's tracked plots. Negotiation applies to the connection.
#[derive(Default)]
pub struct SubLevelManager {
    enabled: bool,
    levels: HashMap<u128, SubLevel>,
    plot_index: HashMap<Plot, u128>,
    clock: SnapshotClock,
    mesh_inputs: Option<MeshInputs>,
}

struct MeshInputs {
    context: Arc<WorkerContext>,
    nether: bool,
    blend: i32,
    smooth: bool,
    cutout_leaves: bool,
}

impl SubLevelManager {
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn clear_level(&mut self) {
        self.levels.clear();
        self.plot_index.clear();
        self.clock = SnapshotClock::default();
        self.mesh_inputs = None;
    }

    pub fn levels(&self) -> impl Iterator<Item = &SubLevel> {
        self.levels.values()
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    pub fn is_backing_column(&self, x: i32, z: i32) -> bool {
        self.enabled && sable::is_plot_chunk(x, z)
    }

    fn get_by_plot_mut(&mut self, plot: Plot) -> Option<&mut SubLevel> {
        self.levels.get_mut(self.plot_index.get(&plot)?)
    }

    fn at_column(&mut self, x: i32, z: i32) -> Option<(&mut SubLevel, (i32, i32))> {
        let level = self.get_by_plot_mut(sable::plot_from_chunk(x, z)?)?;
        let local = level.local_column(x, z);
        Some((level, local))
    }

    /// The ordered packet consumer calls this for both worker-decoded and direct columns.
    /// An unowned backing column is discarded, including after its plot stops tracking.
    pub fn route_column(
        &mut self,
        chunk: Chunk,
        block_entities: impl IntoIterator<Item = ([i32; 3], (i32, Tag))>,
    ) -> Option<Chunk> {
        if !self.is_backing_column(chunk.x, chunk.z) {
            return Some(chunk);
        }
        if let Some((level, local)) = self.at_column(chunk.x, chunk.z) {
            level.columns.insert(
                local,
                PlotColumn {
                    chunk,
                    block_entities: block_entities.into_iter().collect(),
                    meshes: Vec::new(),
                },
            );
            level.mark_neighbors(local);
        }
        None
    }

    pub fn forget_column(&mut self, x: i32, z: i32) {
        if let Some((level, local)) = self.at_column(x, z) {
            level.columns.remove(&local);
            level.dirty.remove(&local);
            level.mark_neighbors(local);
        }
    }

    pub fn set_block(&mut self, pos: [i32; 3], state: u32, same_block: impl FnOnce(u32) -> bool) {
        let [x, y, z] = pos;
        if let Some((level, local)) = self.at_column(x >> 4, z >> 4)
            && let Some(column) = level.columns.get_mut(&local)
        {
            let old = column.chunk.set(x, y, z, state);
            if old != state {
                if column.block_entities.contains_key(&pos) && !same_block(old) {
                    column.block_entities.remove(&pos);
                }
                level.mark_neighbors(local);
            }
        }
    }

    pub fn set_block_entity(&mut self, pos: [i32; 3], kind: i32, tag: &Tag) {
        if let Some((level, local)) = self.at_column(pos[0] >> 4, pos[2] >> 4)
            && let Some(column) = level.columns.get_mut(&local)
            && matches!(tag, Tag::Compound(fields) if !fields.is_empty())
        {
            column.block_entities.insert(pos, (kind, tag.clone()));
        }
    }

    pub fn update_light(&mut self, x: i32, z: i32, update: impl FnOnce(&mut Chunk) -> bool) {
        if let Some((level, local)) = self.at_column(x, z)
            && let Some(column) = level.columns.get_mut(&local)
            && update(&mut column.chunk)
        {
            level.mark_neighbors(local);
        }
    }

    fn invalidate_meshes(&mut self) {
        for level in self.levels.values_mut() {
            level.dirty.extend(level.columns.keys().copied());
        }
    }

    pub fn rebuild_meshes(&mut self, context: &Arc<WorkerContext>, settings: &Streaming) {
        if self.levels.is_empty() {
            self.mesh_inputs = None;
            return;
        }
        if self.mesh_inputs.as_ref().is_none_or(|old| {
            !Arc::ptr_eq(&old.context, context)
                || old.nether != settings.nether_shading
                || old.blend != settings.biome_blend
                || old.smooth != settings.smooth_lighting
                || old.cutout_leaves != settings.cutout_leaves
        }) {
            self.mesh_inputs = Some(MeshInputs {
                context: context.clone(),
                nether: settings.nether_shading,
                blend: settings.biome_blend,
                smooth: settings.smooth_lighting,
                cutout_leaves: settings.cutout_leaves,
            });
            self.invalidate_meshes();
        }
        if !self
            .levels
            .values()
            .any(|level| level.finalized && !level.dirty.is_empty())
        {
            return;
        }
        let mut mesher = Mesher {
            assets: &context.assets,
            render: &context.render,
            tables: &context.tables,
            biomes: &context.biomes,
            visibility: context.tables.visibility.clone(),
            zoom: settings.biome_zoom,
            nether_shading: settings.nether_shading,
            blend_radius: settings.biome_blend,
            smooth_lighting: settings.smooth_lighting,
            cutout_leaves: settings.cutout_leaves,
        };
        let mut scratch = MeshScratch::default();
        for level in self.levels.values_mut() {
            level.rebuild(&mut mesher, &mut scratch);
        }
    }

    fn remove(&mut self, id: u128) {
        if let Some(level) = self.levels.remove(&id) {
            self.plot_index.remove(&level.plot);
        }
    }

    fn start(&mut self, start: StartTracking) {
        let Some(origin) = sable::plot_chunk_origin(start.plot.0, start.plot.1) else {
            return;
        };
        self.enable();
        if let Some(&old) = self.plot_index.get(&start.plot) {
            self.remove(old);
        }
        self.remove(start.id);
        let (plot, id) = (start.plot, start.id);
        let mut level = SubLevel::new(start, origin);
        if self.clock.info.is_some_and(|info| !info.stopped) {
            level.previous_pose = level.sample(self.clock.last_sample);
            level.current_pose = level.sample(self.clock.sample);
        }
        self.plot_index.insert(plot, id);
        self.levels.insert(id, level);
    }

    pub fn handle(&mut self, packet: Packet) {
        match packet {
            Packet::Start(start) => self.start(start),
            Packet::Finalize(plot) => {
                if let Some(level) = self.get_by_plot_mut(plot) {
                    level.finalized = true;
                    level.dirty.extend(level.columns.keys().copied());
                }
            }
            Packet::StopTracking(plot) => {
                if let Some(&id) = self.plot_index.get(&plot) {
                    self.remove(id);
                }
            }
            Packet::Bounds(plot, bounds) => {
                if let Some(level) = self.get_by_plot_mut(plot)
                    && level.bounds != bounds
                {
                    level.bounds = bounds;
                    level.dirty.extend(level.columns.keys().copied());
                }
            }
            Packet::Name(id, name) => {
                if let Some(level) = self.levels.get_mut(&id) {
                    level.name = name;
                }
            }
            Packet::Split { id, parent, pose } => self.split(id, parent, pose),
            Packet::StopMoving(plot) => {
                if let Some(level) = self.get_by_plot_mut(plot) {
                    level.stopped = true;
                    level.linear_velocity = [0.0; 3];
                    level.angular_velocity = [0.0; 3];
                }
            }
            Packet::SnapshotInfo(info) => self.clock.receive(info),
            Packet::Snapshot(snapshot) => self.apply_snapshot(snapshot),
            // Their player, debug, and physics consumers are outside terrain tracking.
            Packet::FreezePlayer(..)
            | Packet::PhysicsProperty(_)
            | Packet::DimensionPhysics(_)
            | Packet::FloatingMaterial(..)
            | Packet::UdpActivation(_)
            | Packet::EnterGizmo => {}
        }
    }

    fn split(&mut self, id: u128, parent: u128, relative: SubLevelPose) {
        let Some(first) = self
            .levels
            .get(&id)
            .and_then(|l| l.history.front())
            .map(|s| s.0)
        else {
            return;
        };
        let Some(parent) = self.levels.get(&parent) else {
            return;
        };
        let past: Vec<_> = parent
            .history
            .iter()
            .take_while(|(tick, _)| *tick < first)
            .map(|&(tick, pose)| {
                let point = pose
                    .model_matrix(DVec3::from(relative.position), DVec3::ZERO)
                    .transform_point3(DVec3::ZERO);
                (
                    tick,
                    SubLevelPose {
                        position: point.to_array(),
                        rotation: pose.rotation,
                        ..relative
                    },
                )
            })
            .collect();
        let level = self.levels.get_mut(&id).expect("split plot");
        for sample in past.into_iter().rev() {
            level.history.push_front(sample);
        }
        while level.history.len() > HISTORY_LIMIT {
            level.history.pop_front();
        }
        if self.clock.info.is_some_and(|info| !info.stopped) {
            level.previous_pose = level.sample(self.clock.last_sample);
            level.current_pose = level.sample(self.clock.sample);
        }
    }

    fn apply_snapshot(&mut self, snapshot: SableSnapshot) {
        let tick = snapshot.interpolation_tick;
        for entry in snapshot.entries {
            let Some(level) = self.get_by_plot_mut(sable::plot_coord_parts(entry.plot_coordinate))
            else {
                continue;
            };
            if level.history.back().is_some_and(|(last, _)| tick < *last) {
                continue;
            }
            if level.history.back().is_none_or(|(last, _)| tick > *last) {
                level.history.push_back((
                    tick,
                    SubLevelPose {
                        position: entry.position,
                        rotation: entry.rotation,
                        rotation_point: entry.rotation_point,
                    },
                ));
                if level.history.len() > HISTORY_LIMIT {
                    level.history.pop_front();
                }
            }
            level.linear_velocity = entry.linear_velocity;
            level.angular_velocity = entry.angular_velocity;
            level.stopped = false;
        }
    }

    pub fn tick(&mut self, tick_rate: f32) {
        let Some(tick) = self.clock.advance(tick_rate) else {
            return;
        };
        for level in self.levels.values_mut() {
            level.previous_pose = level.current_pose;
            level.current_pose = level.sample(tick);
            while level.history.len() > 2 && f64::from(level.history[1].0) < tick - 6.0 {
                level.history.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sable::SableSnapshotEntry, world::World};

    fn start(plot: Plot, id: u128) -> StartTracking {
        let (x, z) = sable::plot_chunk_origin(plot.0, plot.1).unwrap();
        let pose = SubLevelPose {
            position: [82.5, 66.0, 80.5],
            rotation_point: [f64::from(x * 16 + 15), 66.0, f64::from(z * 16)],
            ..SubLevelPose::default()
        };
        StartTracking {
            plot,
            id,
            last_pose: pose,
            pose,
            bounds: Bounds {
                min: [x * 16 + 15, 66, z * 16],
                max: [x * 16 + 17, 67, z * 16 + 2],
            },
            name: None,
            tick: 40,
        }
    }

    #[test]
    fn full_sync_keeps_columns_and_updates_out_of_the_world() {
        let mut plots = SubLevelManager::default();
        let mut world = World::new(horizon_world::registry::BlockStateId(0), 0);
        let plot = (1, 2);
        let (x, z) = sable::plot_chunk_origin(plot.0, plot.1).unwrap();
        let start = start(plot, 42);
        let mut insert = |plots: &mut SubLevelManager, chunk: Chunk| {
            if let Some(chunk) = plots.route_column(chunk, []) {
                world.chunks.insert((chunk.x, chunk.z), Arc::new(chunk));
            }
        };
        insert(&mut plots, Chunk::empty(0, 0));
        plots.handle(Packet::Start(start.clone()));
        insert(&mut plots, Chunk::empty(x, z));
        let initial = [x * 16 + 16, 66, z * 16];
        let tag = Tag::Compound([("test".into(), Tag::Int(7))].into());
        assert!(
            plots
                .route_column(Chunk::empty(x + 1, z), [(initial, (9, tag.clone()))])
                .is_none()
        );
        assert!(!plots.levels[&42].finalized);
        assert_eq!(plots.levels[&42].meshes().count(), 0);
        plots.handle(Packet::Finalize(plot));
        assert_eq!(
            plots.levels[&42].columns[&(1, 0)]
                .block_entities
                .get(&initial)
                .map(|(kind, tag)| (*kind, tag)),
            Some((9, &tag))
        );
        let pos = [x * 16 + 17, 66, z * 16];
        plots.set_block(pos, 1, |_| false);
        plots.set_block_entity(pos, 8, &tag);
        plots.update_light(x + 1, z, |chunk| {
            Arc::make_mut(&mut chunk.sections[8]).light = crate::chunk::Light::Uniform(0xF5);
            true
        });
        plots.handle(Packet::Bounds(plot, Bounds { min: pos, max: pos }));
        plots.handle(Packet::Name(42, Some("ship".into())));
        let level = &plots.levels[&42];
        assert!(level.finalized);
        assert_eq!(level.columns.len(), 2);
        let column = &level.columns[&(1, 0)];
        assert_eq!(column.chunk.get(pos[0], pos[1], pos[2]), 1);
        assert_eq!(column.block_entities[&pos], (8, tag));
        assert_eq!(column.chunk.sections[8].light.get(0), 0xF5);
        assert_eq!(level.dirty.len(), 2);
        assert_eq!(level.bounds.max, pos);
        assert!(!level.bounds.contains_section([x, 66 >> 4, z]));
        assert!(level.bounds.contains_section([x + 1, 66 >> 4, z]));
        assert_eq!(level.name.as_deref(), Some("ship"));

        let mut moved = start.pose.position;
        moved[0] += 4.0;
        plots.handle(Packet::SnapshotInfo(SnapshotInfo {
            elapsed_ms: 50,
            tick: 42,
            stopped: false,
        }));
        plots.handle(Packet::Snapshot(SableSnapshot {
            interpolation_tick: 42,
            entries: vec![SableSnapshotEntry {
                plot_coordinate: sable::plot_coord_long(plot.0, plot.1),
                position: moved,
                rotation: start.pose.rotation,
                rotation_point: start.pose.rotation_point,
                linear_velocity: [2.0, 0.0, 0.0],
                angular_velocity: [0.0; 3],
            }],
        }));
        plots.tick(20.0);
        let pose = plots.levels[&42].render_pose(0.5);
        assert_eq!(pose.position[0], start.pose.position[0] + 1.5);
        plots.handle(Packet::StopMoving(plot));
        for _ in 0..4 {
            plots.tick(20.0);
        }
        let level = &plots.levels[&42];
        assert_eq!(level.render_pose(1.0).position, moved);
        assert_eq!(level.linear_velocity, [0.0; 3]);

        plots
            .levels
            .get_mut(&42)
            .unwrap()
            .columns
            .get_mut(&(0, 0))
            .unwrap()
            .meshes
            .push((8, SectionMesh::default()));
        assert_eq!(plots.levels[&42].meshes().count(), 1);
        plots.handle(Packet::StopTracking(plot));
        insert(&mut plots, Chunk::empty(x, z));
        assert!(plots.is_empty());
        assert!(plots.plot_index.is_empty());
        assert_eq!(world.chunks.len(), 1);
        assert!(world.chunks.contains_key(&(0, 0)));
    }

    #[test]
    fn split_seeds_child_from_parent_and_reallocation_drops_old_data() {
        let mut plots = SubLevelManager::default();
        let parent = start((0, 0), 1);
        let mut child = start((1, 0), 2);
        child.tick += 3;
        plots.handle(Packet::Start(parent.clone()));
        plots.handle(Packet::Start(child.clone()));
        let relative = SubLevelPose {
            position: [
                parent.pose.rotation_point[0] + 2.0,
                68.0,
                parent.pose.rotation_point[2],
            ],
            ..child.pose
        };
        plots.handle(Packet::Split {
            id: 2,
            parent: 1,
            pose: relative,
        });
        assert_eq!(plots.levels[&2].sample(40.0).position, [84.5, 68.0, 80.5]);
        let (x, z) = sable::plot_chunk_origin(1, 0).unwrap();
        assert!(plots.route_column(Chunk::empty(x, z), []).is_none());
        plots.handle(Packet::Start(StartTracking { id: 3, ..child }));
        assert!(!plots.levels.contains_key(&2));
        assert!(plots.levels[&3].columns.is_empty());
        plots.clear_level();
        assert!(plots.plot_index.is_empty());
        assert!(plots.is_backing_column(x, z));
        let mut vanilla = SubLevelManager::default();
        assert!(vanilla.route_column(Chunk::empty(x, z), []).is_some());
    }

    #[test]
    fn start_seeds_the_pivot_without_a_running_snapshot_clock() {
        let mut start = start((1, 2), 42);
        start.last_pose.rotation_point = [0.0; 3];
        let expected = SubLevelPose {
            rotation_point: start.pose.rotation_point,
            ..start.last_pose
        };
        let mut plots = SubLevelManager::default();
        plots.handle(Packet::Start(start));
        assert_eq!(plots.levels[&42].render_pose(0.5), expected);
    }

    #[test]
    fn snapshot_clock_adjusts_to_send_interval_and_restarts() {
        let mut clock = SnapshotClock::default();
        clock.receive(SnapshotInfo {
            elapsed_ms: 100,
            tick: 100,
            stopped: false,
        });
        assert!((clock.advance(20.0).unwrap() - 99.475).abs() < 1e-9);
        clock.receive(SnapshotInfo {
            elapsed_ms: 50,
            tick: 101,
            stopped: true,
        });
        clock.receive(SnapshotInfo {
            elapsed_ms: 50,
            tick: 99,
            stopped: false,
        });
        for _ in 0..8 {
            clock.advance(20.0);
        }
        assert_eq!(clock.sample, 101.0);
        clock.receive(SnapshotInfo {
            elapsed_ms: 50,
            tick: 105,
            stopped: false,
        });
        assert_eq!(clock.advance(20.0), Some(104.5));
    }

    #[test]
    fn section_model_applies_java_pose_about_absolute_pivot() {
        let pose = SubLevelPose {
            position: [80.25, 69.0, -3.5],
            rotation: [
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            rotation_point: [20_480_001.25, 65.5, 20_480_001.75],
        };
        let origin = DVec3::new(20_480_000.0, 64.0, 20_480_000.0);
        let camera = DVec3::new(78.0, 67.0, -7.0);
        let result = pose
            .model_matrix(origin, camera)
            .transform_point3(DVec3::new(3.25, 2.5, 2.75));
        // Offset (2, 1, 1) becomes (1, 1, -2) under a positive Y rotation.
        assert!((result - DVec3::new(3.25, 3.0, 1.5)).length() < 1e-6);
    }
}
