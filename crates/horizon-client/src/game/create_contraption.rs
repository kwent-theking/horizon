//! Visuals for remote Create pistons and block-mounted Big Cannons.

pub(super) mod burst;
mod cannon;
mod ejector;
mod packet;
mod projectile;
#[cfg(test)]
mod scene_test;
mod shot;

use super::Game;
use ejector::SpringEjector;
use glam::{DVec3, Mat4, Vec3};
use horizon_client::{block_entity_render::vertex_light, mesher::Vertex};
use horizon_client_core::nbt_value::text;
use horizon_core::position::BlockPos;
use horizon_protocol::{buf::Reader, packets::play};
use horizon_world::{
    chunk::BlockState,
    nbt::Tag,
    registry::BlockStateId,
    stepped_interpolation::{Path, PosRot, Start, SteppedInterpolation},
};
use shot::{CannonKind, Recoil};
use std::collections::{HashMap, HashSet};

const STATIONARY_CONTRAPTION: &str = "create:stationary_contraption";
const MAX_TRACKED: usize = 512;
const MAX_ACTIVE_BLOCKS: usize = 65_536;
/// `CBCCfgClient.enableEmissiveTracers` (default false): the tracers'
/// `RenderType.eyes` layer.
const ENABLE_EMISSIVE_TRACERS: bool = false;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stationary,
    Cannon,
}

fn entity_kind(kind: &str) -> Option<Kind> {
    match kind {
        STATIONARY_CONTRAPTION => Some(Kind::Stationary),
        cannon::PITCH_CONTRAPTION => Some(Kind::Cannon),
        _ => None,
    }
}

pub(super) fn handles_entity(kind: &str) -> bool {
    entity_kind(kind).is_some()
}

pub(super) struct PlacedBlock {
    pub(super) local: [i32; 3],
    pub(super) state: BlockStateId,
}

#[derive(Default)]
pub(super) struct ControllerPose {
    pub(super) piston_offset: Option<f64>,
    pub(super) cannon_angles: Option<(f32, f32)>,
}

pub(super) struct VisualPose<'a> {
    pub(super) anchor: DVec3,
    pub(super) transform: Mat4,
    pub(super) blocks: &'a [PlacedBlock],
    moving_blocks: &'a [PlacedBlock],
    moving_transform: Mat4,
    ejectors: &'a [SpringEjector],
    ejector_scale: f32,
}

enum Drive {
    Piston {
        controller: [i32; 3],
        anchor: DVec3,
        direction: DVec3,
    },
    Cannon(cannon::PitchContraptionPose, CannonKind),
}

impl Drive {
    fn from_spawn(kind: Kind, origin: DVec3, tag: &Tag) -> Option<Self> {
        let contraption = tag.get("Contraption")?;
        let kind_id = text(contraption.get("Type"))?;
        match kind {
            Kind::Stationary if kind_id == "create:piston" => {
                let Tag::Int(orientation) = contraption.get("Orientation")? else {
                    return None;
                };
                let direction = match orientation {
                    0 => DVec3::NEG_Y,
                    1 => DVec3::Y,
                    2 => DVec3::NEG_Z,
                    3 => DVec3::Z,
                    4 => DVec3::NEG_X,
                    5 => DVec3::X,
                    _ => return None,
                };
                let relative = packet::decode_pos(tag.get("ControllerRelative")?)?;
                let absolute = origin.floor() + DVec3::from_array(relative.map(f64::from));
                if !valid_position(absolute.to_array()) {
                    return None;
                }
                let controller = absolute.as_ivec3().to_array();
                let anchor = DVec3::from_array(
                    packet::decode_pos(contraption.get("Anchor")?)?.map(f64::from),
                );
                if !valid_position(anchor.to_array()) {
                    return None;
                }
                Some(Self::Piston {
                    controller,
                    anchor,
                    direction,
                })
            }
            Kind::Cannon
                if matches!(
                    kind_id.as_str(),
                    "createbigcannons:mounted_cannon" | "createbigcannons:mounted_autocannon"
                ) =>
            {
                let kind = if kind_id == "createbigcannons:mounted_cannon" {
                    CannonKind::Big
                } else {
                    CannonKind::Auto
                };
                cannon::PitchContraptionPose::from_spawn(tag, origin)
                    .map(|pose| Self::Cannon(pose, kind))
            }
            _ => None,
        }
    }

    fn controller(&self) -> Option<[i32; 3]> {
        match self {
            Self::Piston { controller, .. } => Some(*controller),
            Self::Cannon(cannon, _) => cannon.controller,
        }
    }
}

struct Body {
    drive: Drive,
    blocks: Vec<PlacedBlock>,
    moving_blocks: Vec<PlacedBlock>,
    ejectors: Vec<SpringEjector>,
    radius: f64,
    piston_head: bool,
}

struct Entity {
    kind: Kind,
    origin: DVec3,
    previous: PosRot,
    current: PosRot,
    interpolation: SteppedInterpolation,
    body: Option<Body>,
    stalled: bool,
    stall_position: Option<DVec3>,
    recoil: Option<Recoil>,
}

#[derive(Default)]
pub(super) struct State {
    entities: HashMap<i32, Entity>,
    pub(super) projectiles: projectile::State,
    pub(super) bursts: burst::State,
}

fn valid_position(position: [f64; 3]) -> bool {
    // A generous bound for legal world coordinates and safe block sampling.
    position
        .into_iter()
        .all(|value| value.abs() <= 1_000_000_000.0)
}

fn angular_lerp(from: f32, to: f32, alpha: f32) -> f32 {
    from + ((to - from + 180.0).rem_euclid(360.0) - 180.0) * alpha
}

impl State {
    pub(super) fn add(
        &mut self,
        id: i32,
        kind: &str,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) -> bool {
        self.remove(id);
        let Some(kind) = entity_kind(kind) else {
            return false;
        };
        if self.entities.len() >= MAX_TRACKED
            || !valid_position(position)
            || !yaw.is_finite()
            || !pitch.is_finite()
        {
            return false;
        }
        let current = PosRot {
            position,
            yaw,
            pitch,
        };
        self.entities.insert(
            id,
            Entity {
                kind,
                origin: position.into(),
                previous: current,
                current,
                interpolation: SteppedInterpolation::new(3),
                body: None,
                stalled: false,
                stall_position: None,
                recoil: None,
            },
        );
        true
    }

    pub(super) fn contains(&self, id: i32) -> bool {
        self.entities.contains_key(&id)
    }

    pub(super) fn remove(&mut self, id: i32) {
        self.entities.remove(&id);
        self.projectiles.remove(id);
        self.bursts.remove(id);
    }

    pub(super) fn apply_spawn(
        &mut self,
        data: &[u8],
        resolve: impl Fn(&BlockState) -> Option<BlockStateId>,
    ) -> bool {
        let Ok(id) = Reader::new(data).varint() else {
            return false;
        };
        let Some(entity) = self.entities.get(&id) else {
            return false;
        };
        let Some((_, tag)) = packet::decode_advanced_spawn(data) else {
            return false;
        };
        let Some(drive) = Drive::from_spawn(entity.kind, entity.origin, &tag) else {
            return false;
        };
        let Some(packed) = packet::decode_blocks(&tag) else {
            return false;
        };
        let already_active: usize = self
            .entities
            .iter()
            .filter(|(other, _)| **other != id)
            .filter_map(|(_, e)| e.body.as_ref())
            .map(|b| b.blocks.len() + b.moving_blocks.len())
            .sum();
        if packed.len() > MAX_ACTIVE_BLOCKS.saturating_sub(already_active) {
            return false;
        }
        let piston = matches!(&drive, Drive::Piston { .. });
        let mut moving_blocks = Vec::new();
        let mut ejectors = Vec::new();
        if matches!(&drive, Drive::Cannon(_, CannonKind::Auto)) {
            let cap = (MAX_ACTIVE_BLOCKS - already_active - packed.len()).min(packed.len());
            let packed_positions: HashSet<_> = packed.iter().map(|block| block.local).collect();
            let mut seen = HashSet::new();
            for spring in &packed {
                let Some(ejector) = SpringEjector::new(spring.local, &spring.state) else {
                    continue;
                };
                ejectors.push(ejector);
                let Some(data) = spring.data else { continue };
                for (offset, state) in shot::spring_blocks(data) {
                    if moving_blocks.len() == cap {
                        break;
                    }
                    let local = std::array::from_fn(|i| spring.local[i].saturating_add(offset[i]));
                    if !packed_positions.contains(&local) || !seen.insert(local) {
                        continue;
                    }
                    if let Some(state) =
                        packet::decode_state(state).and_then(|state| resolve(&state))
                    {
                        moving_blocks.push(PlacedBlock { local, state });
                    }
                }
            }
        }
        let mut piston_head = false;
        let mut radius: f64 = 0.0;
        let mut blocks = Vec::with_capacity(packed.len());
        let mut poles = Vec::new();
        for block in packed {
            let Some(state) = resolve(&block.state) else {
                continue;
            };
            piston_head |= piston && block.state.name == "create:mechanical_piston_head";
            radius = radius.max(DVec3::from_array(block.local.map(f64::from)).length() + 3.0);
            let placed = PlacedBlock {
                local: block.local,
                state,
            };
            if piston && block.state.name == "create:piston_extension_pole" {
                poles.push(placed);
            } else {
                blocks.push(placed);
            }
        }
        // If the registry could not supply the head, the controller still
        // supplies the whole head-and-pole fallback. Draw only its cargo here.
        if piston_head {
            blocks.extend(poles);
        }
        if blocks.is_empty() {
            return false;
        }
        let entity = self
            .entities
            .get_mut(&id)
            .expect("entity was just resolved");
        if let Drive::Cannon(cannon, _) = &drive {
            entity.current.yaw = cannon.yaw;
            entity.current.pitch = cannon.pitch;
            entity.previous = entity.current;
        }
        if piston {
            entity.stalled |= matches!(
                tag.get("Contraption").and_then(|body| body.get("Stalled")),
                Some(Tag::Byte(value)) if *value != 0
            );
        }
        entity.body = Some(Body {
            drive,
            blocks,
            moving_blocks,
            ejectors,
            radius,
            piston_head,
        });
        true
    }

    pub(super) fn add_projectile(&mut self, kind: &str, p: &play::AddEntity) {
        self.projectiles.add(kind, p);
    }

    pub(super) fn apply_projectile_spawn(&mut self, data: &[u8]) -> bool {
        self.projectiles.apply_spawn(data)
    }

    pub(super) fn apply_projectile_motion(&mut self, data: &[u8]) {
        self.projectiles.apply_motion(data);
    }

    /// A shell's `ID_FLAGS` (entity data 8); `false` when `id` is no shell.
    pub(super) fn set_projectile_flags(&mut self, id: i32, flags: i8) -> bool {
        self.projectiles.set_flags(id, flags)
    }

    pub(super) fn set_projectile_tracer(&mut self, id: i32, tracer: bool) {
        self.projectiles.set_tracer(id, tracer);
    }

    pub(super) fn shell_ticks(
        &mut self,
        player: Option<glam::DVec3>,
    ) -> Vec<projectile::ShellTick> {
        self.projectiles.shell_ticks(player)
    }

    pub(super) fn shell_sounds(&self) -> Vec<horizon_client::sound_engine::ShellSound> {
        self.projectiles.shell_sounds()
    }

    pub(super) fn set_shell_sound_cooldown(&mut self, id: i32, cooldown: i32) {
        self.projectiles.set_sound_cooldown(id, cooldown);
    }

    pub(super) fn apply_cannon_animation(&mut self, data: &[u8]) {
        if let Some(id) = shot::animation_entity(data)
            && let Some(entity) = self.entities.get_mut(&id)
            && entity
                .body
                .as_ref()
                .is_some_and(|body| matches!(body.drive, Drive::Cannon(_, CannonKind::Auto)))
        {
            entity.recoil = Some(Recoil::default());
        }
    }

    pub(super) fn apply_stall(&mut self, data: &[u8]) {
        let Some((id, position)) = packet::decode_stall(data) else {
            return;
        };
        let Some(entity) = self.entities.get_mut(&id) else {
            return;
        };
        if entity.kind != Kind::Stationary || !valid_position(position) {
            return;
        }
        entity.stalled = true;
        entity.stall_position = Some(position.into());
        entity.current.position = position;
        entity.previous = entity.current;
        entity.interpolation.cancel();
    }

    pub(super) fn set_stalled(&mut self, id: i32, stalled: bool) {
        if let Some(entity) = self.entities.get_mut(&id)
            && entity.kind == Kind::Stationary
        {
            entity.stalled = stalled;
            if !stalled {
                entity.stall_position = None;
            }
        }
    }

    pub(super) fn move_to(
        &mut self,
        id: i32,
        path: Option<Path>,
        rotation: Option<(f32, f32)>,
        far_snaps: bool,
    ) -> Option<[f64; 3]> {
        let entity = self.entities.get_mut(&id)?;
        if !path.as_ref().is_none_or(|path| match path {
            Path::Linear(position) => valid_position(*position),
            Path::Stepped(steps) => {
                !steps.is_empty()
                    && steps
                        .iter()
                        .all(|(position, ticks)| *ticks > 0 && valid_position(*position))
            }
        }) || !rotation.is_none_or(|(yaw, pitch)| yaw.is_finite() && pitch.is_finite())
        {
            return None;
        }
        let heading = entity.interpolation.heading(entity.current);
        let position = path.as_ref().map_or(heading.position, Path::end);
        if far_snaps
            && DVec3::from_array(position).distance_squared(entity.current.position.into()) > 4096.0
        {
            let (yaw, pitch) = rotation.unwrap_or((heading.yaw, heading.pitch));
            entity.current = PosRot {
                position,
                yaw,
                pitch,
            };
            entity.previous = entity.current;
            entity.interpolation.cancel();
        } else if let Start::Snap(at) =
            entity
                .interpolation
                .interpolate_to(entity.current, path, rotation)
        {
            entity.current = at;
            entity.previous = at;
        }
        Some(position)
    }

    pub(super) fn sync(&mut self, packet: &play::EntityPositionSync) -> Option<[f64; 3]> {
        let path = match &packet.path {
            play::PositionPath::Linear(at) => Path::Linear(*at),
            play::PositionPath::Stepped(steps) => Path::Stepped(steps.clone()),
        };
        self.move_to(
            packet.id,
            Some(path),
            Some((packet.yaw, packet.pitch)),
            true,
        )
    }

    pub(super) fn tick(&mut self) {
        for entity in self.entities.values_mut() {
            if entity.recoil.as_mut().is_some_and(|recoil| !recoil.tick()) {
                entity.recoil = None;
            }
            entity.previous = entity.current;
            if let Some(at) = entity.interpolation.interpolate() {
                entity.current = at;
            }
        }
    }

    fn controller(&self, id: i32) -> Option<[i32; 3]> {
        self.entities.get(&id)?.body.as_ref()?.drive.controller()
    }

    pub(super) fn has_piston_head(&self, at: [i32; 3]) -> bool {
        self.entities.values().any(|entity| {
            entity.body.as_ref().is_some_and(|body| {
                body.piston_head
                    && matches!(&body.drive, Drive::Piston { controller, .. } if *controller == at)
            })
        })
    }

    pub(super) fn visual(
        &self,
        id: i32,
        alpha: f64,
        control: ControllerPose,
    ) -> Option<VisualPose<'_>> {
        let entity = self.entities.get(&id)?;
        let body = entity.body.as_ref()?;
        let alpha = alpha.clamp(0.0, 1.0);
        let mut anchor =
            DVec3::from_array(entity.previous.position).lerp(entity.current.position.into(), alpha);
        let mut moving_transform = Mat4::IDENTITY;
        let mut ejector_scale = 1.0;
        let transform = match &body.drive {
            Drive::Piston {
                anchor: rest,
                direction,
                ..
            } => {
                if let Some(stalled) = entity.stall_position {
                    anchor = stalled;
                } else if let Some(offset) = control.piston_offset.filter(|value| value.is_finite())
                {
                    anchor = *rest + *direction * offset;
                }
                Mat4::IDENTITY
            }
            Drive::Cannon(cannon, kind) => {
                let rest = cannon_transform(entity, *cannon, alpha, control.cannon_angles);
                // Only the autocannon has a recoil animation; a big cannon
                // keeps its pose when firing.
                let recoil = entity.recoil.filter(|_| *kind == CannonKind::Auto);
                let offset = recoil.map_or(0.0, |shot| shot.offset(alpha));
                moving_transform = rest * Mat4::from_translation(cannon.local_forward() * -offset);
                ejector_scale = recoil.map_or(1.0, |shot| shot.scale(alpha));
                rest
            }
        };
        Some(VisualPose {
            anchor,
            transform,
            blocks: &body.blocks,
            moving_blocks: &body.moving_blocks,
            moving_transform,
            ejectors: &body.ejectors,
            ejector_scale,
        })
    }
}

fn cannon_transform(
    entity: &Entity,
    cannon: cannon::PitchContraptionPose,
    alpha: f64,
    angles: Option<(f32, f32)>,
) -> Mat4 {
    let (yaw, pitch) = angles
        .map(|(yaw, pitch)| cannon.controller_angles(yaw, pitch))
        .unwrap_or_else(|| {
            (
                angular_lerp(entity.previous.yaw, entity.current.yaw, alpha as f32),
                angular_lerp(entity.previous.pitch, entity.current.pitch, alpha as f32),
            )
        });
    cannon.transform(yaw, pitch)
}

pub(super) fn handles_projectile(kind: &str) -> bool {
    projectile::handles(kind)
}

impl Game<'_> {
    pub(super) fn create_contraption_geometry(
        &self,
        camera: DVec3,
        alpha: f64,
        out: &mut Vec<Vertex>,
    ) {
        let Some(remote) = &self.remote else { return };
        let state = &remote.create_contraptions;
        let range = f64::from(self.settings.render_distance) * 16.0;
        for (&id, entity) in &state.entities {
            let Some(body) = &entity.body else { continue };
            if let &Drive::Piston {
                controller: [x, y, z],
                ..
            } = &body.drive
                && self.world.loaded(x, z)
                && !self
                    .pack
                    .assets
                    .registry
                    .state(self.world.block(x, y, z))
                    .is_some_and(|block| {
                        matches!(
                            block.name.as_str(),
                            "create:mechanical_piston" | "create:sticky_mechanical_piston"
                        ) && block
                            .properties
                            .get("state")
                            .is_some_and(|state| state == "moving")
                    })
            {
                continue;
            }
            let control = state
                .controller(id)
                .map_or_else(ControllerPose::default, |[x, y, z]| {
                    let pos = (x, y, z);
                    ControllerPose {
                        piston_offset: self
                            .world
                            .create_pistons
                            .get(&pos)
                            .filter(|piston| piston.running)
                            .map(|piston| {
                                if entity.stalled {
                                    piston.synchronized_offset
                                } else {
                                    piston.previous_offset
                                        + (piston.offset - piston.previous_offset) * alpha
                                }
                            }),
                        cannon_angles: self
                            .world
                            .create_cannon_mounts
                            .get(&pos)
                            .filter(|mount| mount.running)
                            .map(|mount| (mount.yaw.get(alpha), mount.pitch.get(alpha))),
                    }
                });
            let Some(visual) = state.visual(id, alpha, control) else {
                continue;
            };
            let relative = visual.anchor - camera;
            if relative.length_squared() > (range + body.radius).powi(2) {
                continue;
            }
            let relative = relative.as_vec3();
            let translation = Mat4::from_translation(relative);
            let fixed = visual.blocks.iter().map(|block| (block, visual.transform));
            let moving = visual
                .moving_blocks
                .iter()
                .map(|block| (block, visual.moving_transform));
            for (block, model_transform) in fixed.chain(moving) {
                let transform = translation * model_transform;
                let local = Vec3::from_array(block.local.map(|v| v as f32));
                let center = transform.transform_point3(local + Vec3::splat(0.5));
                if center.length_squared() > ((range + 2.0) as f32).powi(2)
                    || self.view_frustum.as_ref().is_some_and(|frustum| {
                        !frustum.intersects(center - Vec3::ONE, center + Vec3::ONE)
                    })
                {
                    continue;
                }
                let sample = (camera + center.as_dvec3()).floor().as_ivec3();
                let light = vertex_light(self.world.light(sample.x, sample.y, sample.z));
                let [x, y, z] = block.local;
                self.block_model_vertices(
                    block.state,
                    horizon_core::random::position_seed(BlockPos::new(x, y, z)),
                    transform * Mat4::from_translation(local),
                    light,
                    true,
                    out,
                );
            }
            for rod in visual.ejectors {
                let Some(quads) = ejector::baked(
                    self.pack.item_renderer.library(),
                    &self.gui.atlas.sprites,
                    self.gui.atlas.generation,
                    rod.model,
                ) else {
                    continue;
                };
                let transform = translation * visual.transform * rod.matrix(visual.ejector_scale);
                let local = Vec3::from_array(rod.local.map(|v| v as f32));
                let center =
                    (translation * visual.transform).transform_point3(local + Vec3::splat(0.5));
                let sample = (camera + center.as_dvec3()).floor().as_ivec3();
                let light = vertex_light(self.world.light(sample.x, sample.y, sample.z));
                ejector::append(&quads, transform, light, out);
            }
        }
        for visual in state.projectiles.visuals(alpha) {
            let relative = visual.pos - camera;
            if !visual.tracer && relative.length_squared() > range.powi(2) {
                continue;
            }
            let relative = relative.as_vec3();
            if !visual.tracer
                && self.view_frustum.as_ref().is_some_and(|frustum| {
                    !frustum.intersects(relative - Vec3::ONE, relative + Vec3::ONE)
                })
            {
                continue;
            }
            let Some(model) = visual.model else { continue };
            let sample = visual.pos.floor().as_ivec3();
            let light = if visual.tracer {
                vertex_light(0xff)
            } else {
                vertex_light(self.world.light(sample.x, sample.y, sample.z))
            };
            if let Some(partial) = model.partial() {
                if let Some(quads) = ejector::baked(
                    self.pack.item_renderer.library(),
                    &self.gui.atlas.sprites,
                    self.gui.atlas.generation,
                    partial,
                ) {
                    let matrix = projectile::drop_mortar_transform(relative, visual.direction);
                    ejector::append(&quads, matrix, light, out);
                }
            } else if let Some(block) = model.resolve(self.pack.assets.registry) {
                self.block_model_vertices(
                    block,
                    0,
                    projectile::transform(relative, visual.direction),
                    light,
                    true,
                    out,
                );
            }
        }
    }

    pub(super) fn cbc_projectile_geometry(
        &self,
        camera: DVec3,
        alpha: f64,
        camera_rotation: Mat4,
        out: &mut Vec<Vertex>,
    ) {
        let Some(remote) = &self.remote else { return };
        let sprites = &self.gui.atlas.sprites;
        let range = f64::from(self.settings.render_distance) * 16.0;
        let right = camera_rotation.transform_vector3(Vec3::X);
        let up = camera_rotation.transform_vector3(Vec3::Y);
        for visual in remote.create_contraptions.projectiles.visuals(alpha) {
            let relative = visual.pos - camera;
            let center = relative.as_vec3();
            let full_bright = vertex_light(0xff);
            if let Some(round) = visual.round {
                if visual.tracer {
                    if let Some(&rect) = sprites.get("createbigcannons:entity/color") {
                        projectile::round_boxes(&visual, center, round, rect, full_bright, out);
                    }
                    if ENABLE_EMISSIVE_TRACERS
                        && let Some(&rect) = sprites.get("createbigcannons:entity/color_s")
                    {
                        let start = out.len();
                        projectile::round_boxes(&visual, center, round, rect, full_bright, out);
                        projectile::mark_eyes(&mut out[start..]);
                    }
                } else if relative.length_squared() <= range.powi(2)
                    && self.view_frustum.as_ref().is_none_or(|frustum| {
                        frustum.intersects(center - Vec3::splat(0.5), center + Vec3::splat(0.5))
                    })
                    && let Some(&rect) = sprites.get(burst::SHRAPNEL_SPRITE)
                {
                    let sample = visual.pos.floor().as_ivec3();
                    let light = vertex_light(self.world.light(sample.x, sample.y, sample.z));
                    let half = if round == projectile::Round::MachineGun {
                        0.25
                    } else {
                        0.5
                    };
                    projectile::billboard(center, -right * half, up * half, rect, light, out);
                }
            } else if visual.tracer {
                let frame = projectile::tracer_frame(visual.id, self.world.game_time);
                if let Some(&rect) =
                    sprites.get(&format!("createbigcannons:entity/tracer_glow{frame}"))
                {
                    let start = out.len();
                    projectile::billboard(center, right * 0.75, up * 0.75, rect, full_bright, out);
                    if visual.model.and_then(projectile::Model::partial).is_none() {
                        for vertex in &mut out[start..] {
                            vertex.normal = horizon_client::mesher::CULL_BACK;
                        }
                    }
                }
                if ENABLE_EMISSIVE_TRACERS
                    && visual.model.and_then(projectile::Model::partial).is_none()
                    && let Some(&rect) =
                        sprites.get(&format!("createbigcannons:entity/tracer_glow{frame}_s"))
                {
                    let start = out.len();
                    projectile::billboard(center, right * 0.75, up * 0.75, rect, full_bright, out);
                    projectile::mark_eyes(&mut out[start..]);
                }
            }
        }
        self.cbc_burst_geometry(camera, alpha, right, up, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;
    use horizon_world::nbt;

    fn piston_packet(id: i32, structure_type: &str) -> Vec<u8> {
        let source = format!(
            r#"{{ControllerRelative:[I;0,0,-2],Contraption:{{Type:"{structure_type}",
            Anchor:[I;-3,64,6],Orientation:3,Blocks:{{
            Palette:[{{Name:"create:mechanical_piston_head",Properties:{{facing:"south",type:"sticky"}}}},
                     {{Name:"create:brass_casing"}}],
            BlockList:[{{Pos:[I;0,0,-1],State:0}},{{Pos:[I;0,0,0],State:1}}]}}}}}}"#,
        );
        let (tag, _) = nbt::parse_snbt(&source).unwrap();
        let payload = nbt::write_network(&tag, nbt::Limits::default()).unwrap();
        let mut wire = Writer::new();
        wire.varint(id);
        wire.byte_array(&payload);
        wire.data
    }

    #[test]
    fn the_piston_controller_carries_every_block_as_one_body() {
        let mut entities = State::default();
        let payload = piston_packet(42, "create:piston");
        let resolve = |state: &BlockState| match state.name.as_str() {
            "create:mechanical_piston_head" => {
                assert_eq!(state.properties["type"], "sticky");
                Some(BlockStateId(1))
            }
            "create:brass_casing" => Some(BlockStateId(2)),
            _ => None,
        };
        assert!(!entities.apply_spawn(&payload, resolve));
        assert!(entities.add(42, STATIONARY_CONTRAPTION, [-3.0, 64.0, 7.25], 0.0, 0.0));
        assert!(entities.apply_spawn(&payload, resolve));
        assert!(entities.has_piston_head([-3, 64, 5]));
        let pose = entities
            .visual(
                42,
                1.0,
                ControllerPose {
                    piston_offset: Some(2.25),
                    ..ControllerPose::default()
                },
            )
            .unwrap();
        assert_eq!(pose.anchor, DVec3::new(-3.0, 64.0, 8.25));
        assert_eq!(pose.transform, Mat4::IDENTITY);
        assert_eq!(pose.blocks[0].local, [0, 0, -1]);
        assert_eq!(pose.blocks[1].state, BlockStateId(2));

        let mut stall = Writer::new();
        stall.i32(42);
        for coordinate in [-3.0, 64.0, 7.5] {
            stall.f64(coordinate);
        }
        stall.f32(0.0);
        entities.apply_stall(&stall.data);
        let control = || ControllerPose {
            piston_offset: Some(2.25),
            ..ControllerPose::default()
        };
        assert_eq!(
            entities.visual(42, 1.0, control()).unwrap().anchor,
            DVec3::new(-3.0, 64.0, 7.5)
        );
        entities.set_stalled(42, false);
        assert_eq!(
            entities.visual(42, 1.0, control()).unwrap().anchor,
            DVec3::new(-3.0, 64.0, 8.25)
        );
        let unsupported = piston_packet(42, "create:bearing");
        assert!(!entities.apply_spawn(&unsupported, resolve));
        assert!(entities.has_piston_head([-3, 64, 5]));
        entities.remove(42);
        assert!(!entities.has_piston_head([-3, 64, 5]));
        assert!(
            entities
                .visual(42, 0.0, ControllerPose::default())
                .is_none()
        );
    }
}
