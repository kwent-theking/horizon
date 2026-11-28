//! Client visuals for Big Cannons' block-model projectiles.

use crate::game::{Dim, cbc_effects::TracerKind, creatures::clip_projectile_blocks};
use glam::{DVec3, Mat4, Vec3};
use horizon_client::{
    blocks::{BlockFacts, Fluid},
    collisions::SceneCollisions,
    hud::Rect,
    mesher::Vertex,
};
use horizon_core::{
    geometry::Vec3 as CoreVec3,
    random::{LegacyRandom, RandomSource},
};
use horizon_protocol::{buf::Reader, packets::play};
use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::collections::HashMap;

#[cfg(test)]
pub(super) use horizon_client::net::RPL_PRECISE_MOTION_CHANNEL as PRECISE_MOTION_CHANNEL;
const MAX_PROJECTILES: usize = 512;
const MAX_NETWORK_VALUE: f64 = 1_000_000_000.0;
const ADVANCED_SPAWN_BYTES: usize = 56;
pub(super) const DROP_MORTAR_PARTIAL: &str = "createbigcannons:block/drop_mortar_shell_flying";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Model {
    block: &'static str,
    north: bool,
}

impl Model {
    pub(super) fn for_entity(kind: &str) -> Option<Self> {
        // CBCEntityTypes and each projectile's getRenderedBlockState in CBC 5.11.7.
        let block = match kind {
            "createbigcannons:shot" => "createbigcannons:solid_shot",
            "createbigcannons:he_shell" => "createbigcannons:he_shell",
            "createbigcannons:ap_shot" => "createbigcannons:ap_shot",
            "createbigcannons:ap_shell" => "createbigcannons:ap_shell",
            "createbigcannons:shrapnel_shell" => "createbigcannons:shrapnel_shell",
            "createbigcannons:fluid_shell" => "createbigcannons:fluid_shell",
            "createbigcannons:smoke_shell" => "createbigcannons:smoke_shell",
            "createbigcannons:bag_of_grapeshot" => "createbigcannons:bag_of_grapeshot",
            "createbigcannons:traffic_cone" => "createbigcannons:traffic_cone",
            "createbigcannons:drop_mortar_shell" => "createbigcannons:drop_mortar_shell",
            "createbigcannons:mortar_stone" => {
                return Some(Self {
                    block: "createbigcannons:mortar_stone_projectile",
                    north: false,
                });
            }
            _ => return None,
        };
        Some(Self { block, north: true })
    }

    pub(super) fn partial(self) -> Option<&'static str> {
        (self.block == "createbigcannons:drop_mortar_shell").then_some(DROP_MORTAR_PARTIAL)
    }

    fn acceleration(self, velocity: DVec3, dim: Dim, fluid: Fluid, no_gravity: bool) -> DVec3 {
        // CBC 5.11.7's bundled properties, from its jar: data/createbigcannons/munition_properties/
        // projectiles/*.json (gravity -0.05, drag 0.01, quadratic_drag false; mortar_stone -0.025),
        // data/minecraft/dimension_munition_properties/*.json and data/minecraft/fluid_drag/*.json.
        let gravity = match self.block {
            _ if no_gravity => 0.0,
            "createbigcannons:bag_of_grapeshot" => 0.0,
            "createbigcannons:mortar_stone_projectile" => -0.025,
            _ => -0.05,
        };
        let drag = if self.block == "createbigcannons:bag_of_grapeshot" {
            0.0
        } else {
            0.01
        };
        let (gravity_multiplier, air_density) = match dim {
            Dim::Overworld => (1.0, 1.0),
            Dim::Nether => (1.1, 1.1),
            Dim::End => (0.8, 0.0),
        };
        let fluid_density = match fluid {
            Fluid::Water => 30.0,
            Fluid::Lava => 60.0,
            _ => 0.0,
        };
        let speed = velocity.length();
        let drag_force = (drag * (air_density + fluid_density) * speed).min(speed);
        gravity * gravity_multiplier * DVec3::Y - normalize(velocity) * drag_force
    }

    pub(super) fn resolve(self, registry: &BlockRegistry) -> Option<BlockStateId> {
        let default = registry.default_state(self.block)?;
        if !self.north {
            return Some(default);
        }
        let state = registry.state(default)?;
        if state.properties.get("facing")?.as_str() == "north" {
            return Some(default);
        }
        let mut north = state.clone();
        north.properties.insert("facing".into(), "north".into());
        registry.id(&north)
    }
}

/// CBC bakes the rendered block north and rotates it about its center in flight.
pub(super) fn transform(position: Vec3, direction: Vec3) -> Mat4 {
    let direction = direction.try_normalize().unwrap_or(Vec3::NEG_Y);
    let heading = if direction.x == 0.0 && direction.z == 0.0 {
        0.0
    } else {
        (-direction.x).atan2(-direction.z)
    };
    let elevation = direction.y.clamp(-1.0, 1.0).asin();
    Mat4::from_translation(position)
        * Mat4::from_rotation_y(heading)
        * Mat4::from_rotation_x(elevation)
        * Mat4::from_translation(Vec3::splat(-0.5))
}

/// CBCUtils.mat4x4fFacing rotates the negative source axis to the destination.
fn facing(dest: DVec3, source: DVec3) -> Mat4 {
    let [x, y, z] = source.cross(dest).as_vec3().to_array();
    let cosine = -source.dot(dest) as f32;
    if f64::from(cosine) > -0.9999 {
        let k = 1.0 / (1.0 + cosine);
        Mat4::from_cols_array(&[
            x * x * k + cosine,
            y * x * k - z,
            z * x * k + y,
            0.0,
            x * y * k + z,
            y * y * k + cosine,
            z * y * k - x,
            0.0,
            x * z * k - y,
            y * z * k + x,
            z * z * k + cosine,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    } else if (1.0 - source.z).abs() < 1e-4 {
        Mat4::from_scale(Vec3::new(-1.0, 1.0, -1.0))
    } else {
        facing(dest, DVec3::Z) * facing(source, DVec3::Z).transpose()
    }
}

pub(super) fn drop_mortar_transform(position: Vec3, direction: Vec3) -> Mat4 {
    let mut direction = direction.as_dvec3();
    if direction.length_squared() < 1e-4 {
        direction = DVec3::NEG_Y;
    }
    let horizontal = DVec3::new(direction.x, 0.0, direction.z);
    let rotation = if horizontal.length_squared() > 1e-4 && direction.y.abs() > 1e-2 {
        let horizontal = horizontal.normalize();
        facing(-direction.normalize(), horizontal) * facing(horizontal, DVec3::Z)
    } else {
        facing(direction.normalize(), DVec3::Z)
    };
    Mat4::from_translation(position) * rotation
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum Round {
    Autocannon,
    MachineGun,
}

impl Round {
    fn for_entity(kind: &str) -> Option<Self> {
        match kind {
            "createbigcannons:ap_autocannon" | "createbigcannons:flak_autocannon" => {
                Some(Self::Autocannon)
            }
            "createbigcannons:machine_gun_bullet" => Some(Self::MachineGun),
            _ => None,
        }
    }

    pub(in crate::game) fn flyby_event(self) -> &'static str {
        match self {
            Self::Autocannon => "createbigcannons:autocannon_round_flyby",
            Self::MachineGun => "createbigcannons:machine_gun_round_flyby",
        }
    }
}

pub(super) fn handles(kind: &str) -> bool {
    Model::for_entity(kind).is_some() || Round::for_entity(kind).is_some()
}

pub(super) fn tracer_frame(id: i32, game_time: i64) -> i64 {
    game_time.wrapping_add(i64::from(id)).rem_euclid(4)
}

pub(super) fn billboard(
    center: Vec3,
    right: Vec3,
    up: Vec3,
    rect: Rect,
    light: u32,
    out: &mut Vec<Vertex>,
) {
    let corners = [
        center - right - up,
        center + right - up,
        center + right + up,
        center - right + up,
    ];
    let uvs = [
        [rect.u0, rect.v1],
        [rect.u1, rect.v1],
        [rect.u1, rect.v0],
        [rect.u0, rect.v0],
    ];
    out.extend(
        corners
            .into_iter()
            .zip(uvs)
            .map(|(pos, uv)| vertex(pos, uv, u32::MAX, light)),
    );
}

pub(super) fn mark_eyes(vertices: &mut [Vertex]) {
    for vertex in vertices {
        vertex.layer = horizon_client::gfx::CBC_EYES_OVERLAY;
        vertex.normal = horizon_client::mesher::CULL_BACK;
    }
}

fn vertex(pos: Vec3, uv: [f32; 2], color: u32, light: u32) -> Vertex {
    Vertex {
        pos: pos.to_array(),
        uv,
        layer: 0,
        color,
        light,
        normal: [0, 127, 0, 0],
        glint_uv: [0.0; 2],
    }
}

/// CBC's facing matrix rotates the negative of its source onto the destination.
fn round_facing(dest: Vec3, source: Vec3) -> Mat4 {
    let cross = source.cross(dest);
    let cos = -source.dot(dest);
    if cos <= -0.9999 {
        return if (1.0 - source.z).abs() < 1e-4 {
            Mat4::from_diagonal(glam::Vec4::new(-1.0, 1.0, -1.0, 1.0))
        } else {
            round_facing(dest, Vec3::Z) * round_facing(source, Vec3::Z)
        };
    }
    let Vec3 { x, y, z } = cross;
    let k = 1.0 / (1.0 + cos);
    Mat4::from_cols_array(&[
        x * x * k + cos,
        y * x * k - z,
        z * x * k + y,
        0.0,
        x * y * k + z,
        y * y * k + cos,
        z * y * k - x,
        0.0,
        x * z * k - y,
        y * z * k + x,
        z * z * k + cos,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ])
}

pub(super) fn round_boxes(
    visual: &Visual,
    center: Vec3,
    round: Round,
    rect: Rect,
    light: u32,
    out: &mut Vec<Vertex>,
) {
    let direction = if visual.direction.length_squared() < 1e-4 {
        Vec3::NEG_Y
    } else {
        visual.direction
    };
    let horizontal = Vec3::new(direction.x, 0.0, direction.z);
    let direction_normal = direction.normalize();
    let facing = if horizontal.length_squared() > 1e-4 && direction.y.abs() > 1e-2 {
        let horizontal = horizontal.normalize();
        round_facing(-direction_normal, horizontal) * round_facing(horizontal, Vec3::NEG_Z)
    } else {
        round_facing(direction_normal, Vec3::NEG_Z)
    };
    let thickness = if round == Round::MachineGun {
        1.0 / 32.0
    } else {
        2.0 / 32.0
    };
    for (t, inverted, color) in [
        (thickness, false, 0xff00d8ff),
        (thickness * 1.5, true, 0xff0050ff),
    ] {
        let front = -t - visual.length;
        let points = [
            [-t, -t, front],
            [-t, t, front],
            [t, t, front],
            [t, -t, front],
            [-t, -t, t],
            [-t, t, t],
            [t, t, t],
            [t, -t, t],
        ]
        .map(|p| center + facing.transform_vector3(Vec3::from_array(p)));
        for mut corners in [
            [0, 1, 2, 3],
            [0, 4, 5, 1],
            [4, 7, 6, 5],
            [3, 2, 6, 7],
            [7, 4, 0, 3],
            [1, 5, 6, 2],
        ] {
            if inverted {
                corners.swap(1, 3);
            }
            out.extend(corners.map(|i| {
                let mut vertex = vertex(points[i], [rect.u0, rect.v0], color, light);
                vertex.normal = horizon_client::mesher::CULL_BACK;
                vertex
            }));
        }
    }
}

struct Motion {
    id: i32,
    position: DVec3,
    velocity: DVec3,
    lerp_steps: i32,
}

fn read_vector(input: &mut Reader<'_>) -> Option<DVec3> {
    let vector = DVec3::new(input.f64().ok()?, input.f64().ok()?, input.f64().ok()?);
    valid_vector(vector).then_some(vector)
}

fn valid_vector(vector: DVec3) -> bool {
    vector.is_finite() && vector.abs().max_element() <= MAX_NETWORK_VALUE
}

fn decode_motion(data: &[u8]) -> Option<Motion> {
    // RPL 2.1.2's NeoForge root codec, then ClientboundPreciseMotionSyncPacket.
    let mut input = Reader::new(data);
    if input.varint().ok()? != 1 {
        return None;
    }
    let id = input.varint().ok()?;
    let position = read_vector(&mut input)?;
    let velocity = read_vector(&mut input)?;
    let yaw = input.f32().ok()?;
    let pitch = input.f32().ok()?;
    input.bool().ok()?;
    let lerp_steps = match input.varint().ok()? {
        i32::MIN..=0 => 3,
        steps => steps.min(20),
    };
    (input.remaining() == 0 && yaw.is_finite() && pitch.is_finite()).then_some(Motion {
        id,
        position,
        velocity,
        lerp_steps,
    })
}

struct Flight {
    tracer_kind: Option<TracerKind>,
    model: Option<Model>,
    round: Option<Round>,
    tracer: bool,
    displacement: f64,
    round_sound_cooldown: u8,
    random: LegacyRandom,
    previous: DVec3,
    position: DVec3,
    /// Vanilla's position delta base, independent of RPL and local movement.
    packet_position: DVec3,
    velocity: DVec3,
    orientation: DVec3,
    prediction_ticks: u8,
    target: Option<(DVec3, i32)>,
    precise_position: Option<DVec3>,
    landing: Landing,
    no_gravity: bool,
    /// `localSoundCooldown`: -1 while its flyby sound plays.
    sound_cooldown: i32,
}

#[derive(Clone, Copy)]
enum Landing {
    Flying,
    /// `ID_FLAGS` confirms the stop, but the last position packet was in flight.
    AwaitingImpact,
    Predicted,
    Confirmed,
}

impl Landing {
    fn is_in_ground(self) -> bool {
        !matches!(self, Self::Flying)
    }
}

/// One shell's client-side effects of a tick (`AbstractBigCannonProjectile.tick`).
pub(in crate::game) struct ShellTick {
    pub(in crate::game) id: i32,
    pub(in crate::game) from: [f64; 3],
    pub(in crate::game) to: [f64; 3],
    /// The radius of the flyby sound to start, when the shell is heading at
    /// the player and its sound cooldown has run out.
    pub(in crate::game) flyby_radius: Option<f64>,
    pub(in crate::game) round: Option<Round>,
    pub(in crate::game) round_sound: Option<(f32, f32)>,
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

fn core_vec(v: DVec3) -> CoreVec3 {
    CoreVec3::new(v.x, v.y, v.z)
}

impl Flight {
    fn snap_to_target(&mut self) {
        if let Some((position, _)) = self.target.take() {
            self.previous = position;
            self.position = position;
        }
    }

    fn sync_position(&mut self, position: DVec3, steps: i32, resume_flight: bool) {
        self.prediction_ticks = 0;
        self.target = Some((position, steps));
        if self.landing.is_in_ground() {
            self.snap_to_target();
            self.landing = match self.landing {
                Landing::Predicted if resume_flight => Landing::Flying,
                Landing::AwaitingImpact => Landing::Confirmed,
                landing => landing,
            };
        }
    }

    fn sync_velocity(&mut self, velocity: DVec3) {
        self.velocity = velocity;
        if velocity.length_squared() > 1e-4 {
            self.orientation = velocity;
        }
    }

    fn tick(
        &mut self,
        dim: Dim,
        clip: &mut impl FnMut(DVec3, DVec3) -> Option<DVec3>,
        fluid_at: &impl Fn(DVec3) -> Fluid,
    ) {
        if self.sound_cooldown > 0 {
            self.sound_cooldown -= 1;
        }
        self.previous = self.position;
        if let Some((target, steps)) = self.target {
            // Server interpolation takes priority while a target is available.
            self.position = self.position.lerp(target, 1.0 / f64::from(steps));
            self.target = (steps > 1).then_some((target, steps - 1));
        } else if let Some(model) = self.model {
            if matches!(self.landing, Landing::Flying | Landing::AwaitingImpact) {
                let accel = model.acceleration(
                    self.velocity,
                    dim,
                    fluid_at(self.position),
                    self.no_gravity,
                );
                let trajectory = self.velocity + accel;
                // CBC traces through the accelerated velocity and uses half
                // the acceleration for displacement when the trace misses.
                if trajectory.length_squared() >= 1e-4
                    && let Some(impact) = clip(self.position, self.position + trajectory)
                {
                    self.position = impact;
                    self.velocity = DVec3::ZERO;
                    self.landing = match self.landing {
                        Landing::AwaitingImpact => Landing::Confirmed,
                        _ => Landing::Predicted,
                    };
                } else {
                    self.position += self.velocity + 0.5 * accel;
                    self.velocity = trajectory;
                    self.orientation = trajectory;
                }
            }
        } else if self.prediction_ticks > 0 {
            // A newly spawned round can travel before its first exact position arrives.
            self.prediction_ticks -= 1;
            self.position += self.velocity;
        }
        if self.round.is_some() {
            self.displacement += self.position.distance(self.previous);
        }
    }

    fn visual(&self, id: i32, alpha: f64) -> Visual {
        let alpha = alpha.clamp(0.0, 1.0);
        let d2 = self.previous.distance_squared(self.position);
        let diff = if (1e-4..=self.velocity.length_squared() * 4.0).contains(&d2) {
            d2.sqrt()
        } else {
            0.0
        };
        Visual {
            id,
            tracer_kind: self.tracer_kind,
            pos: self.previous.lerp(self.position, alpha),
            direction: self.orientation.as_vec3(),
            model: self.model,
            round: self.round,
            tracer: self.tracer && (self.round.is_some() || !self.landing.is_in_ground()),
            length: diff.min(self.displacement - diff * (1.0 - alpha)) as f32,
        }
    }

    fn round_sound(&mut self, player: Option<DVec3>) -> Option<(f32, f32)> {
        self.round_sound_cooldown = self.round_sound_cooldown.saturating_sub(1);
        let player = player?;
        let radius = DVec3::new(
            3.0 + f64::from(0.2f32) / 2.0,
            3.0,
            3.0 + f64::from(0.2f32) / 2.0,
        );
        let min = self.previous.min(self.position) - radius;
        let max = self.previous.max(self.position) + radius + DVec3::Y * f64::from(0.2f32);
        if !player.cmpge(min).all() || !player.cmplt(max).all() {
            return None;
        }
        let was_audible = self.round_sound_cooldown != 0;
        self.round_sound_cooldown = 4;
        if was_audible {
            return None;
        }
        let volume = if (self.position - self.previous).dot(player - self.previous) < 0.0 {
            0.25
        } else {
            1.0
        };
        Some((volume, 0.95 + self.random.next_float() * 0.2))
    }

    /// `CBCClientCommon.playShellFlyingSoundOnClient`'s test: the player is
    /// within thirty ticks' travel (at most 200 blocks) of the tick's midpoint
    /// and not behind the shell.
    fn flyby_radius(&self, player: Option<DVec3>) -> Option<f64> {
        let player = player?;
        let displacement = self.position - self.previous;
        let origin = self.position - displacement * 0.5;
        let radius = (displacement.length() * 30.0).min(200.0);
        if player.distance_squared(origin) > radius * radius {
            return None;
        }
        let toward = normalize(displacement).dot(normalize(player - origin));
        (toward >= 0.0).then_some(radius)
    }
}

pub(super) struct Visual {
    pub(super) id: i32,
    pub(super) tracer_kind: Option<TracerKind>,
    pub(super) pos: DVec3,
    pub(super) direction: Vec3,
    pub(super) model: Option<Model>,
    pub(super) round: Option<Round>,
    pub(super) tracer: bool,
    pub(super) length: f32,
}

#[derive(Default)]
pub(in crate::game) struct State {
    flights: HashMap<i32, Flight>,
}

impl State {
    pub(in crate::game) fn add(&mut self, kind: &str, packet: &play::AddEntity) -> bool {
        self.remove(packet.id);
        let model = Model::for_entity(kind);
        let round = Round::for_entity(kind);
        if model.is_none() && round.is_none() {
            return false;
        }
        let position = DVec3::from_array(packet.position);
        let velocity = DVec3::from_array(packet.velocity);
        if self.flights.len() >= MAX_PROJECTILES
            || !valid_vector(position)
            || !valid_vector(velocity)
        {
            return false;
        }
        self.flights.insert(
            packet.id,
            Flight {
                tracer_kind: match kind {
                    "createbigcannons:shot" => Some(TracerKind::Shot),
                    "createbigcannons:machine_gun_bullet" => Some(TracerKind::MachineGun),
                    "createbigcannons:ap_autocannon" => Some(TracerKind::Autocannon),
                    _ => None,
                },
                model,
                round,
                tracer: false,
                displacement: 0.0,
                round_sound_cooldown: 0,
                random: LegacyRandom::new(horizon_client::sound_engine::unique_seed()),
                previous: position,
                position,
                packet_position: position,
                velocity,
                orientation: velocity,
                prediction_ticks: 3,
                target: None,
                precise_position: None,
                landing: Landing::Flying,
                no_gravity: false,
                sound_cooldown: 0,
            },
        );
        true
    }

    /// `ID_FLAGS` of a tracked shell; `false` for any other entity.
    pub(in crate::game) fn set_flags(&mut self, id: i32, flags: i8) -> bool {
        let Some(flight) = self.flights.get_mut(&id) else {
            return false;
        };
        if flags & 1 != 0 {
            flight.prediction_ticks = 0;
            flight.snap_to_target();
            // A moving sync can predate the impact. Keep moving toward a
            // local collision until we have an impact or a new position.
            flight.landing = if flight.model.is_some()
                && matches!(flight.landing, Landing::Flying | Landing::AwaitingImpact)
                && flight.velocity.length_squared() >= 1e-4
            {
                Landing::AwaitingImpact
            } else {
                Landing::Confirmed
            };
        } else {
            flight.landing = Landing::Flying;
        }
        if flight.round.is_some() {
            flight.tracer = flags & 2 != 0;
        }
        true
    }

    pub(in crate::game) fn set_no_gravity(&mut self, id: i32, no_gravity: bool) {
        if let Some(flight) = self.flights.get_mut(&id) {
            flight.no_gravity = no_gravity;
        }
    }

    pub(in crate::game) fn set_tracer(&mut self, id: i32, tracer: bool) {
        if let Some(flight) = self.flights.get_mut(&id)
            && flight.model.is_some()
        {
            flight.tracer = tracer;
        }
    }

    pub(in crate::game) fn set_sound_cooldown(&mut self, id: i32, cooldown: i32) {
        if let Some(flight) = self.flights.get_mut(&id) {
            flight.sound_cooldown = cooldown;
        }
    }

    /// After `tick`: the trail of every shell in flight, oldest shell first,
    /// and the flyby sounds due.
    pub(in crate::game) fn shell_ticks(&mut self, player: Option<DVec3>) -> Vec<ShellTick> {
        let mut ticks: Vec<ShellTick> = self
            .flights
            .iter_mut()
            .filter(|(_, flight)| !flight.landing.is_in_ground())
            .map(|(&id, flight)| ShellTick {
                id,
                from: flight.previous.to_array(),
                to: flight.position.to_array(),
                round: flight.round,
                round_sound: flight.round.and_then(|_| flight.round_sound(player)),
                flyby_radius: if flight.round.is_none() && flight.sound_cooldown == 0 {
                    flight.flyby_radius(player)
                } else {
                    None
                },
            })
            .collect();
        ticks.sort_by_key(|tick| tick.id);
        ticks
    }

    /// What the flyby sounds read about the shells.
    pub(in crate::game) fn shell_sounds(&self) -> Vec<horizon_client::sound_engine::ShellSound> {
        self.flights
            .iter()
            .filter(|(_, flight)| flight.round.is_none())
            .map(|(&id, flight)| horizon_client::sound_engine::ShellSound {
                id,
                position: flight.position.to_array(),
                velocity: flight.velocity.to_array(),
                cooldown: flight.sound_cooldown,
            })
            .collect()
    }

    #[cfg(test)]
    fn contains(&self, id: i32) -> bool {
        self.flights.contains_key(&id)
    }

    pub(in crate::game) fn remove(&mut self, id: i32) {
        self.flights.remove(&id);
    }

    pub(in crate::game) fn packet_position(&self, id: i32) -> Option<[f64; 3]> {
        Some(self.flights.get(&id)?.packet_position.to_array())
    }

    pub(in crate::game) fn move_to(&mut self, id: i32, position: Option<[f64; 3]>) -> bool {
        let Some(flight) = self.flights.get_mut(&id) else {
            return false;
        };
        if let Some(position) = position.map(DVec3::from_array).filter(|p| valid_vector(*p)) {
            flight.packet_position = position;
            // A vanilla delta can follow the RPL packet for the same position.
            // Advance the delta base and keep the higher-precision target.
            if flight
                .precise_position
                .is_none_or(|precise| !position.abs_diff_eq(precise, 1.0 / 4096.0))
            {
                flight.precise_position = None;
                flight.sync_position(position, 3, position != flight.position);
            }
        }
        true
    }

    pub(in crate::game) fn set_velocity(&mut self, id: i32, velocity: [f64; 3]) -> bool {
        let Some(flight) = self.flights.get_mut(&id) else {
            return false;
        };
        let velocity = DVec3::from_array(velocity);
        if valid_vector(velocity) {
            flight.sync_velocity(velocity);
            if matches!(flight.landing, Landing::Predicted) && velocity.length_squared() >= 1e-4 {
                flight.landing = Landing::Flying;
            }
        }
        true
    }

    pub(in crate::game) fn apply_spawn(&mut self, data: &[u8]) -> bool {
        self.read_spawn(data).is_some()
    }

    fn read_spawn(&mut self, data: &[u8]) -> Option<()> {
        // AdvancedAddEntityPayload contains CBC's AbstractCannonProjectile spawn data.
        let mut input = Reader::new(data);
        let id = input.varint().ok()?;
        let flight = self.flights.get_mut(&id)?;
        let bytes = ADVANCED_SPAWN_BYTES + if flight.round.is_some() { 8 } else { 0 };
        if input.count(bytes).ok()? != bytes || input.remaining() != bytes {
            return None;
        }
        let pitch = input.f32().ok()?;
        let yaw = input.f32().ok()?;
        let velocity = read_vector(&mut input)?;
        let direction = read_vector(&mut input)?;
        let displacement = if flight.round.is_some() {
            input.f64().ok()?
        } else {
            0.0
        };
        if !pitch.is_finite() || !yaw.is_finite() || !displacement.is_finite() || displacement < 0.0
        {
            return None;
        }
        flight.displacement = displacement;
        flight.velocity = velocity;
        flight.orientation = direction;
        Some(())
    }

    pub(in crate::game) fn apply_motion(&mut self, data: &[u8]) -> bool {
        let Some(motion) = decode_motion(data) else {
            return false;
        };
        let Some(flight) = self.flights.get_mut(&motion.id) else {
            return false;
        };
        // ClientboundPreciseMotionSyncPacket sets the delta movement too.
        flight.sync_velocity(motion.velocity);
        flight.precise_position = Some(motion.position);
        flight.sync_position(
            motion.position,
            motion.lerp_steps,
            motion.velocity.length_squared() >= 1e-4,
        );
        true
    }

    pub(in crate::game) fn tick(
        &mut self,
        mut scene: SceneCollisions<'_, '_>,
        facts: &BlockFacts,
        dim: Dim,
    ) {
        let world = scene.world;
        self.step(
            dim,
            |from, to| {
                clip_projectile_blocks(&mut scene, facts, core_vec(from), core_vec(to))
                    .map(|hit| DVec3::from_array(hit.location.components()))
            },
            |at| {
                facts
                    .get(world.block(
                        at.x.floor() as i32,
                        at.y.floor() as i32,
                        at.z.floor() as i32,
                    ))
                    .fluid
            },
        );
    }

    fn step(
        &mut self,
        dim: Dim,
        mut clip: impl FnMut(DVec3, DVec3) -> Option<DVec3>,
        fluid_at: impl Fn(DVec3) -> Fluid,
    ) {
        for flight in self.flights.values_mut() {
            flight.tick(dim, &mut clip, &fluid_at);
        }
    }

    #[cfg(test)]
    pub(super) fn tick_in_air(&mut self) {
        self.step(Dim::Overworld, |_, _| None, |_| Fluid::None);
    }

    #[cfg(test)]
    pub(super) fn visual(&self, id: i32, alpha: f64) -> Option<Visual> {
        self.flights.get(&id).map(|flight| flight.visual(id, alpha))
    }

    pub(super) fn visuals(&self, alpha: f64) -> impl Iterator<Item = Visual> + '_ {
        self.flights
            .iter()
            .map(move |(&id, flight)| flight.visual(id, alpha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;
    use horizon_world::chunk::BlockState;
    use std::collections::BTreeMap;

    fn add_packet() -> play::AddEntity {
        play::AddEntity {
            id: 140,
            uuid: 0,
            kind: 37,
            position: [0.0, 1.0, 0.0],
            velocity: [1.0, 0.0, 0.0],
            pitch: 0,
            yaw: 0,
            head_yaw: 0,
            data: 0,
        }
    }

    fn motion(id: i32, position: [f64; 3], discriminator: i32) -> Vec<u8> {
        motion_with_velocity(id, position, [0.0, 0.0, -25.0], discriminator)
    }

    fn motion_with_velocity(
        id: i32,
        position: [f64; 3],
        velocity: [f64; 3],
        discriminator: i32,
    ) -> Vec<u8> {
        let mut out = Writer::new();
        out.varint(discriminator);
        out.varint(id);
        for value in position.into_iter().chain(velocity) {
            out.f64(value);
        }
        out.f32(180.0);
        out.f32(0.0);
        out.bool(false);
        out.varint(3);
        out.data
    }

    #[test]
    fn drop_mortar_shell_uses_its_partial_and_the_big_cannon_flight() {
        let mut state = State::default();
        let packet = add_packet();
        assert!(state.add("createbigcannons:drop_mortar_shell", &packet));
        state.set_tracer(packet.id, true);
        state.tick_in_air();
        let visual = state.visual(packet.id, 1.0).unwrap();
        assert!(visual.tracer);
        assert_eq!(visual.model.unwrap().partial(), Some(DROP_MORTAR_PARTIAL));
        assert!(
            state.shell_ticks(Some(DVec3::new(10.0, 1.0, 0.0)))[0]
                .flyby_radius
                .is_some()
        );
        state.set_flags(packet.id, 1);
        assert!(state.shell_ticks(None).is_empty());
    }

    #[test]
    fn cbc_drop_mortar_partial_rotates_about_the_entity_origin() {
        let origin = Vec3::new(19.0, -7.0, -10.0);
        for flight in [Vec3::ZERO, Vec3::NEG_Y, Vec3::Z, Vec3::new(1.0, 2.0, 3.0)] {
            let matrix = drop_mortar_transform(origin, flight);
            let expected = flight.try_normalize().unwrap_or(Vec3::NEG_Y);
            assert_eq!(matrix.transform_point3(Vec3::ZERO), origin);
            assert!(
                matrix
                    .transform_vector3(Vec3::NEG_Z)
                    .abs_diff_eq(expected, 1e-5)
            );
        }
    }

    #[test]
    fn model_resolves_real_north_variant_and_aliases() {
        let block = "createbigcannons:solid_shot";
        let registry = BlockRegistry::from_ordered_states(
            ["up", "north"]
                .map(|facing| BlockState {
                    name: block.into(),
                    properties: [
                        ("facing".into(), facing.into()),
                        ("waterlogged".into(), "false".into()),
                    ]
                    .into(),
                })
                .to_vec(),
            BTreeMap::from([(block.into(), 0)]),
        )
        .unwrap();
        let model = Model::for_entity("createbigcannons:shot").unwrap();
        assert_eq!(model.resolve(&registry), Some(BlockStateId(1)));
        assert!(Model::for_entity("minecraft:arrow").is_none());
    }

    #[test]
    fn orientation_preserves_center_and_aims_the_north_facing_model() {
        let position = Vec3::new(4.0, 2.0, 1.0);
        for direction in [Vec3::Z, Vec3::NEG_Y, Vec3::Y, Vec3::new(1.0, 2.0, 3.0)] {
            let matrix = transform(position, direction);
            assert!(
                matrix
                    .transform_point3(Vec3::splat(0.5))
                    .abs_diff_eq(position, 1e-5)
            );
            assert!(
                matrix
                    .transform_vector3(Vec3::NEG_Z)
                    .abs_diff_eq(direction.normalize(), 1e-5)
            );
        }
    }

    #[test]
    fn spawn_precision_motion_and_remove_update_only_the_tracked_shell() {
        let mut state = State::default();
        let entity = add_packet();
        assert!(state.add("createbigcannons:he_shell", &entity));
        let mut spawn = Writer::new();
        spawn.varint(entity.id);
        spawn.varint(ADVANCED_SPAWN_BYTES as i32);
        spawn.f32(0.0);
        spawn.f32(90.0);
        for value in [12.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            spawn.f64(value);
        }
        let spawn = spawn.data;
        assert!(!state.apply_spawn(&spawn[..spawn.len() - 1]));
        assert!(state.apply_spawn(&spawn));
        assert_eq!(state.visual(entity.id, 1.0).unwrap().direction, Vec3::Y);
        state.tick_in_air();
        let spawn_at = DVec3::from_array(entity.position);
        let first_step = spawn_at + DVec3::new(12.0 - 0.01 * 12.0 / 2.0, -0.05 / 2.0, 0.0);
        assert!(
            state
                .visual(entity.id, 0.5)
                .unwrap()
                .pos
                .abs_diff_eq(spawn_at.lerp(first_step, 0.5), 1e-9)
        );
        assert!(!state.apply_motion(&motion(141, [15.0, 1.0, -15.0], 1)));
        assert!(!state.apply_motion(&motion(entity.id, [f64::NAN, 1.0, -15.0], 1)));
        assert!(!state.apply_motion(&motion(entity.id, [15.0, 1.0, -15.0], 2)));
        assert!(state.apply_motion(&motion(entity.id, [15.0, 1.0, -15.0], 1)));
        let target = DVec3::new(15.0, 1.0, -15.0);
        state.tick_in_air();
        assert!(
            state
                .visual(entity.id, 1.0)
                .unwrap()
                .pos
                .abs_diff_eq(first_step.lerp(target, 1.0 / 3.0), 1e-9)
        );
        assert_eq!(
            state.visual(entity.id, 1.0).unwrap().direction,
            Vec3::NEG_Z * 25.0
        );
        for _ in 0..2 {
            state.tick_in_air();
        }
        assert!(
            state
                .visual(entity.id, 1.0)
                .unwrap()
                .pos
                .abs_diff_eq(target, 1e-9)
        );
        state.remove(entity.id);
        assert!(state.visual(entity.id, 1.0).is_none());
    }

    #[test]
    fn dud_shell_draws_at_the_server_position_on_landing() {
        let mut state = State::default();
        let entity = play::AddEntity {
            position: [0.5, 72.0, 0.5],
            velocity: [0.0, -1.5, 0.0],
            ..add_packet()
        };
        assert!(state.add("createbigcannons:he_shell", &entity));
        let landed = DVec3::new(0.5, 64.0, 0.5);
        // Rounded CBC flight samples, ending with the clipped impact movement.
        for (y, dy) in [
            (70.4825, -1.535),
            (68.93018, -1.56965),
            (67.34337, -1.60395),
            (65.72244, -1.63791),
            (64.06772, -1.67153),
            (64.0, -0.06772),
        ] {
            assert!(state.apply_motion(&motion_with_velocity(
                entity.id,
                [landed.x, y, landed.z],
                [0.0, dy, 0.0],
                1,
            )));
            state.tick_in_air();
        }
        assert!(state.visual(entity.id, 1.0).unwrap().pos.y > landed.y);

        // On the stop tick RPL sends the exact position and zero velocity
        // before ServerEntity sends CBC's dirty ID_FLAGS (index 8).
        let stopped = motion_with_velocity(entity.id, landed.to_array(), [0.0; 3], 1);
        assert!(state.apply_motion(&stopped));
        assert!(state.set_flags(entity.id, 1));
        assert_eq!(state.visual(entity.id, 0.5).unwrap().pos, landed);

        // The tracker keeps sending precise updates for the resting shell.
        for _ in 0..4 {
            assert!(state.apply_motion(&stopped));
            assert!(state.flights[&entity.id].target.is_none());
            state.tick_in_air();
            assert_eq!(state.visual(entity.id, 0.5).unwrap().pos, landed);
        }
    }

    #[test]
    fn dud_shell_lands_on_stone_after_flight_syncs_stop() {
        use horizon_core::{geometry::VoxelShape, position::BlockPos};

        for receives_flags in [false, true] {
            let mut state = State::default();
            let landed = DVec3::new(0.5, 64.0, 0.5);
            let entity = play::AddEntity {
                position: (landed + 8.0 * DVec3::Y).to_array(),
                velocity: [0.0, -1.5, 0.0],
                ..add_packet()
            };
            let stone = BlockPos::new(0, 63, 0);
            let clip = |from: DVec3, to: DVec3| {
                VoxelShape::block()
                    .clip(core_vec(from), core_vec(to), stone)
                    .map(|hit| DVec3::from_array(hit.location.components()))
            };
            assert!(state.add("createbigcannons:he_shell", &entity));
            // First two CBC air steps from y=72 and dy=-1.5. The last
            // server target is still almost five blocks above the floor.
            for (y, dy) in [(70.4825, -1.535), (68.930175, -1.56965)] {
                assert!(state.apply_motion(&motion_with_velocity(
                    entity.id,
                    [landed.x, y, landed.z],
                    [0.0, dy, 0.0],
                    1,
                )));
                state.step(Dim::Overworld, clip, |_| Fluid::None);
            }
            if receives_flags {
                assert!(state.set_flags(entity.id, 1));
            }
            for _ in 0..8 {
                state.step(Dim::Overworld, clip, |_| Fluid::None);
            }
            assert!(state.flights[&entity.id].landing.is_in_ground());
            assert!(
                state
                    .visual(entity.id, 0.5)
                    .unwrap()
                    .pos
                    .abs_diff_eq(landed, 1e-9)
            );

            // A later exact position can correct the locally clipped position.
            let corrected = landed + DVec3::X;
            if receives_flags {
                assert!(state.move_to(entity.id, Some(corrected.to_array())));
            } else {
                // A rest packet can arrive before the dirty in-ground bit.
                let stopped = motion_with_velocity(entity.id, landed.to_array(), [0.0; 3], 1);
                assert!(state.apply_motion(&stopped));
                state.step(Dim::Overworld, clip, |_| Fluid::None);
                assert_eq!(state.visual(entity.id, 0.5).unwrap().pos, landed);
                assert!(state.apply_motion(&motion_with_velocity(
                    entity.id,
                    corrected.to_array(),
                    entity.velocity,
                    1,
                )));
                assert!(!state.flights[&entity.id].landing.is_in_ground());
            }
            assert_eq!(state.visual(entity.id, 0.5).unwrap().pos, corrected);
        }
    }

    #[test]
    fn shell_over_open_air_keeps_the_three_tick_sync_interpolation() {
        let mut state = State::default();
        let entity = play::AddEntity {
            position: [0.5, 72.0, 0.5],
            velocity: [0.0, -1.5, 0.0],
            ..add_packet()
        };
        assert!(state.add("createbigcannons:he_shell", &entity));
        let mut target = DVec3::from_array(entity.position);
        let mut velocity = DVec3::from_array(entity.velocity);
        let mut drawn = target;
        for tick in 0..12 {
            // CBC's packaged HE shell uses -0.05 gravity and 0.01 linear drag.
            let accel = -0.05 * DVec3::Y - 0.01 * velocity;
            target += velocity + 0.5 * accel;
            velocity += accel;
            assert!(state.apply_motion(&motion_with_velocity(
                entity.id,
                target.to_array(),
                velocity.to_array(),
                1,
            )));
            let vanilla = [target.x, (target.y * 4096.0).round() / 4096.0, target.z];
            if tick % 2 == 0 {
                state.tick_in_air();
            }
            assert!(state.move_to(entity.id, Some(vanilla)));
            assert_eq!(state.packet_position(entity.id), Some(vanilla));
            assert_eq!(state.flights[&entity.id].target.unwrap().0, target);
            if tick % 2 != 0 {
                state.tick_in_air();
            }
            let previous = drawn;
            drawn = drawn.lerp(target, 1.0 / 3.0);
            assert_eq!(state.visual(entity.id, 1.0).unwrap().pos, drawn);
            assert_eq!(
                state.visual(entity.id, 0.5).unwrap().pos,
                previous.lerp(drawn, 0.5)
            );
            assert_eq!(state.flights[&entity.id].velocity, velocity);
        }
    }

    #[test]
    fn big_cannon_tracer_tracks_the_stack_and_animation_tick() {
        let mut state = State::default();
        let packet = add_packet();
        assert!(state.add("createbigcannons:he_shell", &packet));
        assert!(!state.visual(packet.id, 1.0).unwrap().tracer);
        state.set_tracer(packet.id, true);
        assert!(state.visual(packet.id, 1.0).unwrap().tracer);
        for time in 45..50 {
            assert_eq!(
                tracer_frame(packet.id, time),
                (i64::from(packet.id) + time) % 4
            );
        }
        state.set_flags(packet.id, 1);
        assert!(!state.visual(packet.id, 1.0).unwrap().tracer);
        state.set_flags(packet.id, 0);
        assert!(state.visual(packet.id, 1.0).unwrap().tracer);
        state.set_tracer(packet.id, false);
        assert!(!state.visual(packet.id, 1.0).unwrap().tracer);
    }

    #[test]
    fn autocannon_rounds_have_their_own_spawn_body_and_tracer_bit() {
        for (name, kind) in [
            ("ap_autocannon", Round::Autocannon),
            ("flak_autocannon", Round::Autocannon),
            ("machine_gun_bullet", Round::MachineGun),
        ] {
            let mut state = State::default();
            let packet = add_packet();
            assert!(state.add(&format!("createbigcannons:{name}"), &packet));
            let mut spawn = Writer::new();
            spawn.varint(packet.id);
            spawn.varint((ADVANCED_SPAWN_BYTES + 8) as i32);
            spawn.f32(0.0);
            spawn.f32(0.0);
            for value in [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5] {
                spawn.f64(value);
            }
            assert!(!state.apply_spawn(&spawn.data[..spawn.data.len() - 1]));
            assert!(state.apply_spawn(&spawn.data));
            state.set_tracer(packet.id, true); // Stack metadata only applies to big shells.
            assert!(!state.visual(packet.id, 0.0).unwrap().tracer);
            state.set_flags(packet.id, 2);
            state.tick_in_air();
            let visual = state.visual(packet.id, 1.0).unwrap();
            assert!(visual.tracer && visual.model.is_none());
            assert_eq!(visual.round, Some(kind));
            assert_eq!(visual.length, 1.0);
            state.set_flags(packet.id, 3);
            assert!(state.visual(packet.id, 1.0).unwrap().tracer);
            assert!(state.shell_ticks(None).is_empty());
        }
    }

    #[test]
    fn round_flyby_uses_entity_random_and_resets_while_the_listener_is_nearby() {
        let mut state = State::default();
        let packet = add_packet();
        assert!(state.add("createbigcannons:ap_autocannon", &packet));
        state.flights.get_mut(&packet.id).unwrap().random = LegacyRandom::new(6);
        state.tick_in_air();
        let mut replay = LegacyRandom::new(6);
        let behind = Some(DVec3::new(-1.0, 1.0, 0.0));
        let first = state.shell_ticks(behind);
        assert_eq!(first[0].flyby_radius, None);
        assert_eq!(
            first[0].round_sound,
            Some((0.25, 0.95 + replay.next_float() * 0.2))
        );
        assert_eq!(state.shell_ticks(behind)[0].round_sound, None);
        for _ in 0..3 {
            assert_eq!(state.shell_ticks(None)[0].round_sound, None);
        }
        let ahead = Some(DVec3::new(2.0, 1.0, 0.0));
        assert_eq!(
            state.shell_ticks(ahead)[0].round_sound,
            Some((1.0, 0.95 + replay.next_float() * 0.2))
        );
    }

    #[test]
    fn flying_shells_trail_and_start_a_flyby_toward_a_player_ahead() {
        fn radius(state: &mut State, player: Option<DVec3>) -> Option<f64> {
            state.shell_ticks(player)[0].flyby_radius
        }
        let mut state = State::default();
        let entity = add_packet();
        assert!(state.add("createbigcannons:shot", &entity));
        // The three ticks after a flyby stopped itself (localSoundCooldown 3).
        state.set_sound_cooldown(entity.id, 3);
        for cooldown in [2, 1, 0] {
            assert!(state.apply_motion(&motion_with_velocity(
                entity.id,
                [f64::from(5 - cooldown), 1.0, 0.0],
                entity.velocity,
                1,
            )));
            state.tick_in_air();
            assert_eq!(state.shell_sounds()[0].cooldown, cooldown);
        }
        let ahead = DVec3::new(12.0, 1.0, 0.0);
        let ticks = state.shell_ticks(Some(ahead));
        assert_eq!(ticks.len(), 1);
        assert_eq!(
            (ticks[0].from, ticks[0].to),
            ([2.0, 1.0, 0.0], [3.0, 1.0, 0.0])
        );
        // One block a tick: thirty blocks' reach from the tick's midpoint,
        // toward a player ahead only.
        assert_eq!(ticks[0].flyby_radius, Some(30.0));
        assert_eq!(radius(&mut state, Some(DVec3::new(-10.0, 1.0, 0.0))), None);
        assert_eq!(radius(&mut state, Some(DVec3::new(100.0, 1.0, 0.0))), None);
        assert_eq!(radius(&mut state, None), None);
        // Not while its flyby plays.
        state.set_sound_cooldown(entity.id, -1);
        assert_eq!(radius(&mut state, Some(ahead)), None);
        // The sounds read the precise motion's delta movement.
        assert!(state.apply_motion(&motion(entity.id, [15.0, 1.0, -15.0], 1)));
        assert_eq!(state.shell_sounds()[0].velocity, [0.0, 0.0, -25.0]);
        // A shell in the ground leaves no trail.
        assert!(!state.set_flags(entity.id + 1, 1));
        assert!(state.set_flags(entity.id, 1));
        assert!(state.shell_ticks(None).is_empty());
        assert!(state.set_flags(entity.id, 0));
        assert_eq!(state.shell_ticks(None).len(), 1);
    }
}
