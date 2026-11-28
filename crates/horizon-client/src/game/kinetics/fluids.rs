//! Client visuals for the fluid containers and glass pipes of Create 6.0.10.
//!
//! Tank and pipe tags follow the public Create block-entity formats. Geometry
//! uses the installed pack's sprites and Horizon's world translucency pass.
mod geometry;
#[cfg(test)]
pub(in crate::game::kinetics) mod scene_fixture;

use super::Game;
use glam::{DVec3, Vec3};
use horizon_client::{
    block_entity_render::vertex_light,
    hud::Rect,
    mesher::Vertex,
    world::{
        CreateFluid, CreateFluidFlow, CreateFluidState, CreateFluidTank, CreateVesselKind,
        LerpedValue, World,
    },
};
use horizon_world::nbt::{Compound, JavaString, Tag};
use std::collections::HashMap;

pub(super) fn spout_parts(
    state: Option<&CreateFluidState>,
    alpha: f64,
) -> [super::machines::MachinePart; 3] {
    let ticks = match state {
        Some(CreateFluidState::Vessel { processing, .. }) => *processing,
        _ => -1,
    };
    let remaining = ticks as f32 - alpha as f32;
    let lift = if remaining < 0.0 {
        0.0
    } else if remaining < 2.0 {
        remaining / 2.0
    } else if remaining < 10.0 {
        1.0
    } else {
        let phase = ((15.0 - remaining) / 10.0).clamp(0.0, 1.0);
        4.0 * phase * (1.0 - phase)
    };
    let models = [
        "create:block/spout/top",
        "create:block/spout/middle",
        "create:block/spout/bottom",
    ];
    std::array::from_fn(|i| {
        super::machines::part(
            models[i],
            glam::Mat4::from_translation(Vec3::Y * (lift * i as f32 * 3.0 / 32.0)),
        )
    })
}

const TANK_BLOCK_CAPACITY: u32 = 8_000;
const SIDES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
const AXES: [usize; 6] = [1, 1, 2, 2, 0, 0];

fn vessel_kind(block: &str) -> Option<CreateVesselKind> {
    Some(match block {
        "create:spout" => CreateVesselKind::Spout,
        "create:item_drain" => CreateVesselKind::Drain,
        "create_dragons_plus:fragile_fluid_tank" => CreateVesselKind::FragileTank,
        "create_dragons_plus:levitite_fragile_fluid_tank" => CreateVesselKind::LevititeTank,
        _ => return None,
    })
}

pub(in crate::game) fn is_container(block: &str) -> bool {
    matches!(
        block,
        "create:fluid_tank"
            | "create:creative_fluid_tank"
            | "create:basin"
            | "create:glass_fluid_pipe"
    ) || vessel_kind(block).is_some()
}

fn field<'a>(tag: &'a Compound, name: &str) -> Option<&'a Tag> {
    tag.get(&JavaString::from(name))
}

fn child<'a>(tag: &'a Compound, name: &str) -> Option<&'a Compound> {
    match field(tag, name)? {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    }
}

fn int(tag: &Compound, name: &str) -> Option<i32> {
    match field(tag, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

fn float(tag: &Tag) -> Option<f32> {
    let value = match tag {
        Tag::Float(bits) => f32::from_bits(*bits),
        Tag::Double(bits) => f64::from_bits(*bits) as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn read_fluid(tag: &Compound) -> Option<CreateFluid> {
    let Tag::String(id) = field(tag, "id")? else {
        return None;
    };
    let id = id.to_string_lossy();
    let amount = u32::try_from(int(tag, "amount")?).ok()?;
    if amount == 0 || id.is_empty() || id == "minecraft:empty" || id.len() > 256 {
        return None;
    }
    let tint = match id.as_str() {
        "minecraft:water" | "minecraft:flowing_water" => 0x3f76e4,
        "create:potion" | "create:flowing_potion" => {
            let potion = child(tag, "components")
                .and_then(|components| child(components, "minecraft:potion_contents"));
            potion
                .and_then(|p| int(p, "custom_color").map(|color| color as u32 & 0xffffff))
                .or_else(|| {
                    let Tag::String(potion_id) = field(potion?, "potion")? else {
                        return None;
                    };
                    Some(horizon_client::brewing::potion_color(Some(
                        &potion_id.to_string_lossy(),
                    )))
                })
                .unwrap_or(0xffffff)
        }
        _ => 0xffffff,
    };
    Some(CreateFluid { id, amount, tint })
}

fn tank_fluid(tag: &Compound) -> Option<CreateFluid> {
    read_fluid(child(child(tag, "TankContent")?, "Fluid")?)
}

fn bucket_fluid(tag: &Compound) -> Option<CreateFluid> {
    let held = super::logistics::parse_transported_item(field(tag, "HeldItem")?)?;
    let (namespace, bucket) = held.stack.item.split_once(':')?;
    let name = bucket.strip_suffix("_bucket")?;
    let tint = match (namespace, name) {
        (
            "minecraft",
            "water" | "cod" | "salmon" | "pufferfish" | "tropical_fish" | "axolotl" | "tadpole",
        ) => 0x3f76e4,
        _ => 0xffffff,
    };
    let id = if tint == 0x3f76e4 {
        "minecraft:water".to_owned()
    } else {
        format!("{namespace}:{name}")
    };
    Some(CreateFluid {
        id,
        amount: 1_000,
        tint,
    })
}

fn segments<'a>(tag: &'a Compound, name: &str) -> impl Iterator<Item = &'a Compound> {
    match field(tag, name) {
        Some(Tag::List(items)) => Some(items.iter()),
        _ => None,
    }
    .into_iter()
    .flatten()
    .filter_map(|item| match item {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    })
}

fn level(
    old: Option<(&CreateFluid, LerpedValue)>,
    fluid: Option<&CreateFluid>,
    target: f32,
    snap: bool,
) -> LerpedValue {
    let mut value = if !snap
        && let (Some((previous, level)), Some(fluid)) = (old, fluid)
        && previous.id == fluid.id
    {
        level
    } else {
        LerpedValue::start_with(target)
    };
    value.chase_exp(target, 0.5);
    value
}

fn tank_update(
    block: &str,
    old: Option<&CreateFluidState>,
    tag: &Compound,
) -> Option<CreateFluidState> {
    if !["Controller", "TankContent", "Window", "Boiler"]
        .iter()
        .any(|name| field(tag, name).is_some())
    {
        return None;
    }
    let controller = match field(tag, "Controller") {
        Some(Tag::IntArray(pos)) => <[i32; 3]>::try_from(pos.as_slice()).ok(),
        _ => None,
    };
    let width = int(tag, "Size").unwrap_or(1).clamp(1, 3) as u8;
    let height = int(tag, "Height").unwrap_or(1).clamp(1, 32) as u8;
    let fluid = if controller.is_none() {
        tank_fluid(tag)
    } else {
        None
    };
    let creative = block == "create:creative_fluid_tank";
    let target = fluid.as_ref().map_or(0.0, |fluid| {
        if creative {
            1.0
        } else {
            let capacity = TANK_BLOCK_CAPACITY * u32::from(width).pow(2) * u32::from(height);
            (fluid.amount as f32 / capacity as f32).clamp(0.0, 1.0)
        }
    });
    let prior = match old {
        Some(CreateFluidState::Tank(tank)) => tank.fluid.as_ref().map(|f| (f, tank.level)),
        _ => None,
    };
    let level = level(
        prior,
        fluid.as_ref(),
        target,
        field(tag, "ForceFluidLevel").is_some(),
    );
    let boiler_active = child(tag, "Boiler").is_some_and(|boiler| {
        (int(boiler, "Whistles").unwrap_or(0) > 0 || int(boiler, "Engines").unwrap_or(0) > 0)
            && (int(boiler, "PassiveHeat").unwrap_or(0) != 0
                || int(boiler, "ActiveHeat").unwrap_or(0) > 0)
    });
    Some(CreateFluidState::Tank(CreateFluidTank {
        fluid,
        level,
        width,
        height,
        window: int(tag, "Window").unwrap_or(0) != 0,
        creative,
        controller,
        boiler_active,
    }))
}

fn pipe_update(tag: &Compound) -> Option<CreateFluidState> {
    if !SIDES.iter().any(|name| field(tag, name).is_some()) {
        return None;
    }
    Some(CreateFluidState::Pipe(Box::new(std::array::from_fn(|i| {
        let connection = child(tag, SIDES[i])?;
        let flow = child(connection, "Flow")?;
        let fluid = read_fluid(child(flow, "Fluid")?)?;
        let inward = int(flow, "In").unwrap_or(0) != 0;
        let progress = child(flow, "Progress")
            .and_then(|p| field(p, "Value"))
            .and_then(float)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let pressure = match field(connection, "Pressure") {
            Some(Tag::List(list)) => list
                .iter()
                .nth(usize::from(!inward))
                .and_then(float)
                .unwrap_or(0.0),
            _ => 0.0,
        };
        let mut progress = LerpedValue::start_with(progress);
        progress.chase_linear(
            1.0,
            (1.0 + 31.0 * (pressure / 128.0).clamp(0.0, 1.0)) / 32.0,
        );
        Some(CreateFluidFlow {
            fluid,
            inward,
            progress,
        })
    }))))
}

fn update(block: &str, old: Option<&CreateFluidState>, tag: &Tag) -> Option<CreateFluidState> {
    let Tag::Compound(tag) = tag else {
        return None;
    };
    match block {
        "create:fluid_tank" | "create:creative_fluid_tank" => tank_update(block, old, tag),
        "create:basin"
            if field(tag, "InputTanks").is_some() || field(tag, "OutputTanks").is_some() =>
        {
            let fluids = segments(tag, "InputTanks")
                .take(2)
                .chain(segments(tag, "OutputTanks").take(2))
                .filter_map(tank_fluid)
                .collect();
            Some(CreateFluidState::Basin(fluids))
        }
        "create:glass_fluid_pipe" => pipe_update(tag),
        _ if field(tag, "Tanks").is_some() => {
            let kind = vessel_kind(block)?;
            let fluid = segments(tag, "Tanks").next().and_then(tank_fluid);
            let capacity = if kind == CreateVesselKind::Drain {
                1_500.0
            } else {
                1_000.0
            };
            let target = fluid
                .as_ref()
                .map_or(0.0, |f| (f.amount as f32 / capacity).clamp(0.0, 1.0));
            let prior = match old {
                Some(CreateFluidState::Vessel {
                    fluid: Some(fluid),
                    level,
                    ..
                }) => Some((fluid, *level)),
                _ => None,
            };
            let level = level(prior, fluid.as_ref(), target, false);
            Some(CreateFluidState::Vessel {
                fluid,
                level,
                processing: int(tag, "ProcessingTicks").unwrap_or(-1),
                kind,
                stream_fluid: if kind == CreateVesselKind::Drain {
                    bucket_fluid(tag)
                } else {
                    None
                },
            })
        }
        _ => None,
    }
}

fn apply_fluid_tag(
    world: &mut World,
    pos: (i32, i32, i32),
    block: &str,
    kind: &str,
    tag: &Tag,
) -> bool {
    let type_matches = kind == block
        || (kind == "create_dragons_plus:fragile_fluid_tank"
            && block == "create_dragons_plus:levitite_fragile_fluid_tank");
    if !is_container(block) || !type_matches {
        return false;
    }
    if let Some(state) = update(block, world.create_fluids.get(&pos), tag) {
        world.create_fluids.insert(pos, state);
    }
    true
}

struct FluidDraw<'frame, 'state> {
    atlas: &'frame HashMap<String, Rect>,
    sprites: HashMap<&'state str, Option<[Rect; 2]>>,
    alpha: f64,
    out: &'frame mut Vec<Vertex>,
}

impl<'frame, 'state> FluidDraw<'frame, 'state> {
    fn new(atlas: &'frame HashMap<String, Rect>, alpha: f64, out: &'frame mut Vec<Vertex>) -> Self {
        Self {
            atlas,
            sprites: HashMap::new(),
            alpha,
            out,
        }
    }

    fn emit(
        &mut self,
        block: &str,
        axis: Option<&str>,
        state: &'state CreateFluidState,
        origin: Vec3,
        light: u32,
    ) {
        geometry::volumes(block, axis, state, self.alpha, |volume| {
            let id = volume.fluid.id.as_str();
            let material = *self
                .sprites
                .entry(id)
                .or_insert_with(|| geometry::sprites(self.atlas, id));
            if let Some(material) = material {
                geometry::emit_volume(volume, origin, material, light, self.out);
            }
        });
    }
}

impl Game<'_> {
    pub(in crate::game) fn apply_create_fluid_tag(
        &mut self,
        pos: (i32, i32, i32),
        kind: &str,
        tag: &Tag,
    ) -> bool {
        let state = self.world.block(pos.0, pos.1, pos.2);
        let block = self.res.facts().get(state).name.as_ref();
        apply_fluid_tag(&mut self.world, pos, block, kind, tag)
    }

    pub(in crate::game) fn tick_create_fluids(&mut self) {
        for state in self.world.create_fluids.values_mut() {
            match state {
                CreateFluidState::Tank(tank) => tank.level.tick(),
                CreateFluidState::Vessel {
                    level, processing, ..
                } => {
                    level.tick();
                    *processing = processing.saturating_sub(1).max(-1);
                }
                CreateFluidState::Pipe(flows) => {
                    for flow in flows.iter_mut().flatten() {
                        flow.progress.tick();
                    }
                }
                CreateFluidState::Basin(_) => {}
            }
        }
    }

    pub(in crate::game) fn create_fluid_geometry(
        &self,
        camera: DVec3,
        alpha: f64,
        out: &mut Vec<Vertex>,
    ) {
        let mut draw = FluidDraw::new(&self.gui.atlas.sprites, alpha, out);
        let range = self.settings.render_distance as f32 * 16.0;
        for (&(x, y, z), data) in &self.world.create_fluids {
            let min = (DVec3::new(f64::from(x), f64::from(y), f64::from(z)) - camera).as_vec3();
            let extent = match data {
                CreateFluidState::Tank(tank) => Vec3::new(
                    f32::from(tank.width),
                    f32::from(tank.height),
                    f32::from(tank.width),
                ),
                _ => Vec3::ONE,
            };
            // A spout can extend down into the space between itself and a depot.
            let lower = min - Vec3::Y * 2.0;
            let upper = min + extent;
            if Vec3::ZERO.clamp(lower, upper).length_squared() > range * range
                || self
                    .view_frustum
                    .as_ref()
                    .is_some_and(|view| !view.intersects(lower, upper))
            {
                continue;
            }
            let state = self.world.block(x, y, z);
            let block = self.res.facts().get(state).name.as_ref();
            let axis = self
                .pack
                .assets
                .registry
                .state(state)
                .and_then(|s| s.properties.get("axis"))
                .map(String::as_str);
            let light = vertex_light(self.world.light(x, y, z));
            draw.emit(block, axis, data, min, light);
        }
    }
}
