//! Per-state gameplay facts exported from vanilla's own block-state accessors
//! (`gameplay-facts.json`), indexed by registry state ID.
mod decode;
mod snapshot;
use crate::Result;
use horizon_world::{
    level::{Behaviours, GameplayStateSource},
    registry::{BlockRegistry, BlockStateId},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Seek},
    path::Path,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RenderShape {
    Model,
    Invisible,
    Animated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fluid {
    None,
    Water,
    Lava,
}
#[derive(Clone, Debug)]
pub struct Sounds {
    pub volume: f32,
    pub pitch: f32,
    pub break_sound: Arc<str>,
    pub step: Arc<str>,
    pub place: Arc<str>,
    pub hit: Arc<str>,
    pub fall: Arc<str>,
}
type SoundKey = (u32, u32, Arc<str>, Arc<str>, Arc<str>, Arc<str>, Arc<str>);
#[derive(Clone, Debug)]
pub struct StateFacts {
    pub block: Arc<BlockTypeFacts>,
    pub emission: u8,
    pub dampening: u8,
    pub skylight_down: bool,
    pub shape_occlusion: bool,
    pub can_occlude: bool,
    pub solid_render: bool,
    pub replaceable: bool,
    pub full_collision: bool,
    pub fluid: Fluid,
    /// Fluid amount 1..=8 (8 = source/falling surface), 0 when there is no fluid.
    pub fluid_level: u8,
    pub fluid_falling: bool,
    pub waterlogged: bool,
    /// `BlockState.isSolid()` (legacy solidity; fluid surfaces read it).
    pub legacy_solid: bool,
    /// `isFaceSturdy(.., SupportType.FULL)` by Direction ordinal.
    pub sturdy_full: u8,
    /// `getShape` (the outline used for picking) as boxes in block space,
    /// without the random offset; and its edges as LevelRenderer draws them.
    pub outline: Arc<[[f64; 6]]>,
    pub outline_edges: Arc<[[f64; 6]]>,
    /// `getMapColor(..).id` (0 = NONE).
    pub map_color: u8,
}
/// Facts usually repeated by every state of a block. Sharing uses exact values,
/// not a per-block assumption: a mod's state-specific exception gets its own record.
#[derive(Clone, Debug)]
pub struct BlockTypeFacts {
    pub name: Arc<str>,
    pub hardness: f32,
    pub requires_tool: bool,
    pub random_ticks: bool,
    pub block_entity: bool,
    pub render_shape: RenderShape,
    pub friction: f32,
    pub explosion_resistance: f32,
    pub item: Arc<str>,
    pub sounds: Arc<Sounds>,
    /// `hasOffsetFunction`: the model is jittered per column.
    pub random_offset: bool,
    pub outline_offset: bool,
    /// The class chain, most specific first.
    pub classes: Arc<[Arc<str>]>,
}
impl std::ops::Deref for StateFacts {
    type Target = BlockTypeFacts;
    fn deref(&self) -> &Self::Target {
        &self.block
    }
}
impl std::ops::DerefMut for StateFacts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Preserve independent per-state edits made by fixtures and callers.
        Arc::make_mut(&mut self.block)
    }
}
#[derive(Default)]
struct BlockTypes(HashMap<Arc<str>, Vec<Arc<BlockTypeFacts>>>);
impl BlockTypes {
    fn intern(&mut self, facts: BlockTypeFacts) -> Arc<BlockTypeFacts> {
        let variants = self.0.entry(facts.name.clone()).or_default();
        if let Some(found) = variants.iter().find(|existing| {
            existing.hardness.to_bits() == facts.hardness.to_bits()
                && existing.requires_tool == facts.requires_tool
                && existing.random_ticks == facts.random_ticks
                && existing.block_entity == facts.block_entity
                && existing.render_shape == facts.render_shape
                && existing.friction.to_bits() == facts.friction.to_bits()
                && existing.explosion_resistance.to_bits() == facts.explosion_resistance.to_bits()
                && existing.item == facts.item
                // Sound records are already interned by exact float bits and IDs.
                && Arc::ptr_eq(&existing.sounds, &facts.sounds)
                && existing.random_offset == facts.random_offset
                && existing.outline_offset == facts.outline_offset
                && existing.classes == facts.classes
        }) {
            return found.clone();
        }
        let facts = Arc::new(facts);
        variants.push(facts.clone());
        facts
    }
}
/// `MapColor.col` by id.
pub const MAP_COLORS: [u32; 62] = [
    0, 8368696, 16247203, 13092807, 16711680, 10526975, 10987431, 31744, 16777215, 10791096,
    9923917, 7368816, 4210943, 9402184, 16776437, 14188339, 11685080, 6724056, 15066419, 8375321,
    15892389, 5000268, 10066329, 5013401, 8339378, 3361970, 6704179, 6717235, 10040115, 1644825,
    16445005, 6085589, 4882687, 55610, 8476209, 7340544, 13742497, 10441252, 9787244, 7367818,
    12223780, 6780213, 10505550, 3746083, 8874850, 5725276, 8014168, 4996700, 4993571, 5001770,
    9321518, 2430480, 12398641, 9715553, 6035741, 1474182, 3837580, 5647422, 1356933, 6579300,
    14200723, 8365974,
];
/// The 12 edges of the unit cube (outline data predating the export).
pub fn cube_edges() -> Vec<[f64; 6]> {
    let mut edges = Vec::new();
    for a in 0..3 {
        for (u, v) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.)] {
            let mut from = [0.0f64; 3];
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            from[b] = u;
            from[c] = v;
            let mut to = from;
            to[a] = 1.0;
            edges.push([from[0], from[1], from[2], to[0], to[1], to[2]]);
        }
    }
    edges
}
impl StateFacts {
    pub fn has_class(&self, name: &str) -> bool {
        self.classes.iter().any(|c| &**c == name)
    }
}
/// Compact, owned behaviour inputs retained until the normal behaviour startup stage.
/// No borrowed gameplay JSON or process-wide cache survives the fact loader.
pub struct GameplayProjection {
    flags: u16,
    emission: u64,
    dampening: u64,
    hardness: f64,
    push: Arc<str>,
    volume: f64,
    pitch: f64,
    place: Arc<str>,
    break_sound: Arc<str>,
    fall: Arc<str>,
}

impl GameplayProjection {
    fn from_entry(entry: &decode::Entry<'_>, strings: &mut HashMap<String, Arc<str>>) -> Self {
        fn intern(strings: &mut HashMap<String, Arc<str>>, value: &str) -> Arc<str> {
            if let Some(existing) = strings.get(value) {
                return Arc::clone(existing);
            }
            let value_arc: Arc<str> = Arc::from(value);
            strings.insert(value.to_owned(), Arc::clone(&value_arc));
            value_arc
        }
        const FLAGS: [&str; 11] = [
            "replaceable",
            "random_ticks",
            "solid_render",
            "liquid",
            "block_entity",
            "ignited_by_lava",
            "can_occlude",
            "full_collision",
            "skylight_down",
            "requires_tool",
            "shape_occlusion",
        ];
        Self {
            flags: FLAGS.iter().enumerate().fold(0, |flags, (i, name)| {
                flags | ((u16::from(entry.flag(name))) << i)
            }),
            emission: entry.unsigned("emission"),
            dampening: entry.unsigned("dampening"),
            hardness: entry.number("hardness"),
            push: intern(strings, entry.text("push")),
            volume: entry.sound_number("volume"),
            pitch: entry.sound_number("pitch"),
            place: intern(strings, entry.sound_id("place")),
            break_sound: intern(strings, entry.sound_id("break")),
            fall: intern(strings, entry.sound_id("fall")),
        }
    }
}

impl GameplayStateSource for GameplayProjection {
    fn flag(&self, name: &str) -> bool {
        const FLAGS: [&str; 11] = [
            "replaceable",
            "random_ticks",
            "solid_render",
            "liquid",
            "block_entity",
            "ignited_by_lava",
            "can_occlude",
            "full_collision",
            "skylight_down",
            "requires_tool",
            "shape_occlusion",
        ];
        FLAGS
            .iter()
            .position(|field| *field == name)
            .is_some_and(|index| self.flags & (1 << index) != 0)
    }
    fn unsigned(&self, name: &str) -> u64 {
        match name {
            "emission" => self.emission,
            "dampening" => self.dampening,
            _ => 0,
        }
    }
    fn number(&self, name: &str) -> f64 {
        if name == "hardness" {
            self.hardness
        } else {
            0.0
        }
    }
    fn text(&self, name: &str) -> &str {
        if name == "push" { &self.push } else { "" }
    }
    fn sound_number(&self, name: &str) -> f64 {
        match name {
            "volume" => self.volume,
            "pitch" => self.pitch,
            _ => 1.0,
        }
    }
    fn sound_id(&self, name: &str) -> &str {
        match name {
            "place" => &self.place,
            "break" => &self.break_sound,
            "fall" => &self.fall,
            _ => "",
        }
    }
}

pub struct BlockFacts {
    pub states: Vec<StateFacts>,
    pub air: BlockStateId,
}
impl BlockFacts {
    pub fn load(root: &Path, registry: &BlockRegistry) -> Result<Self> {
        let behaviours: Value = serde_json::from_reader(BufReader::new(File::open(
            root.join("block-behaviours.json"),
        )?))?;
        Self::load_with_behaviour_json(root, registry, &behaviours)
    }

    /// Load both tables from one resource root, sharing each JSON decode.
    /// Stamped roots reuse checked snapshots; editable roots load normally.
    pub fn load_pair(root: &Path, registry: &BlockRegistry) -> Result<(Self, Behaviours)> {
        let behaviours: Value = serde_json::from_reader(BufReader::new(File::open(
            root.join("block-behaviours.json"),
        )?))?;
        Self::load_with_behaviours(root, registry, &behaviours)
    }

    /// Decode state facts once, retaining only the small owned fields needed later.
    pub fn load_with_projection(
        root: &Path,
        registry: &BlockRegistry,
        behaviours: &Value,
    ) -> Result<(Self, Vec<GameplayProjection>)> {
        Self::load_projected(root, registry, behaviours, true)
    }

    pub fn load_with_behaviour_json(
        root: &Path,
        registry: &BlockRegistry,
        behaviours: &Value,
    ) -> Result<Self> {
        Ok(Self::load_projected(root, registry, behaviours, false)?.0)
    }

    /// Build both tables from the same behaviour document and gameplay decoder.
    pub fn load_with_behaviours(
        root: &Path,
        registry: &BlockRegistry,
        behaviours: &Value,
    ) -> Result<(Self, Behaviours)> {
        let (facts, gameplay) = Self::load_with_projection(root, registry, behaviours)?;
        let table = Behaviours::load_with_gameplay(registry, root, behaviours, &gameplay)?;
        Ok((facts, table))
    }

    /// Load block facts from a root without reading `block-behaviours.json` on a
    /// snapshot hit; on a miss, reads `block-behaviours.json` and returns it
    /// alongside the facts so downstream tables can reuse the parsed document.
    pub fn load_root_with_projection(
        root: &Path,
        registry: &BlockRegistry,
        project: bool,
    ) -> Result<(Self, Vec<GameplayProjection>, Option<Value>)> {
        let started = std::time::Instant::now();
        let mut identity = Vec::with_capacity(33);
        identity.extend_from_slice(registry.cache_identity());
        identity.push(u8::from(project));
        let cache = horizon_world::resource_cache::Cache::new(
            root,
            if project {
                "block-facts-root-v2"
            } else {
                "block-facts-root-v2-no-projection"
            },
            &[
                &root.join("gameplay-facts.json"),
                &root.join("block-behaviours.json"),
            ],
            &identity,
        );
        if let Some((facts, projection)) = cache
            .as_ref()
            .and_then(|cache| cache.read_value::<snapshot::Snapshot>())
            .and_then(|snapshot| snapshot.restore())
            && facts.states.len() == registry.len()
            && facts.air
                == registry
                    .default_state("minecraft:air")
                    .unwrap_or(BlockStateId(u32::MAX))
            && (!project || projection.len() == registry.len())
        {
            log::info!(
                "Horizon: stage=block_facts cache=hit elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return Ok((facts, projection, None));
        }
        let behaviour_started = std::time::Instant::now();
        let behaviours: Value = serde_json::from_reader(BufReader::new(File::open(
            root.join("block-behaviours.json"),
        )?))?;
        log::info!(
            "Horizon: stage=behaviour_document elapsed_ms={}",
            behaviour_started.elapsed().as_millis()
        );
        let loaded = Self::load_projected(root, registry, &behaviours, project)?;
        if let Some(cache) = cache.as_ref() {
            let snapshot = snapshot::Snapshot::capture(&loaded.0, &loaded.1);
            let _ = cache.write_value(&snapshot);
        }
        if project {
            let mut no_proj_identity = Vec::with_capacity(33);
            no_proj_identity.extend_from_slice(registry.cache_identity());
            no_proj_identity.push(0);
            if let Some(no_proj_cache) = horizon_world::resource_cache::Cache::new(
                root,
                "block-facts-root-v2-no-projection",
                &[
                    &root.join("gameplay-facts.json"),
                    &root.join("block-behaviours.json"),
                ],
                &no_proj_identity,
            ) {
                let snapshot = snapshot::Snapshot::capture(&loaded.0, &[]);
                let _ = no_proj_cache.write_value(&snapshot);
            }
        }
        Ok((loaded.0, loaded.1, Some(behaviours)))
    }

    fn load_projected(
        root: &Path,
        registry: &BlockRegistry,
        behaviours: &Value,
        project: bool,
    ) -> Result<(Self, Vec<GameplayProjection>)> {
        let started = std::time::Instant::now();
        let mut identity = Vec::with_capacity(65);
        identity.extend_from_slice(registry.cache_identity());
        identity.push(u8::from(project));
        if let Some(digest) = horizon_world::resource_cache::Cache::json_identity(behaviours) {
            identity.extend_from_slice(&digest);
        } else {
            return Self::load_projected_uncached(root, registry, behaviours, project);
        }
        let cache = horizon_world::resource_cache::Cache::new(
            root,
            if project {
                "block-facts-v3"
            } else {
                "block-facts-v3-no-projection"
            },
            &[
                &root.join("gameplay-facts.json"),
                &root.join("block-behaviours.json"),
            ],
            &identity,
        );
        let cache_started = std::time::Instant::now();
        if let Some((facts, projection)) = cache
            .as_ref()
            .and_then(|cache| cache.read_value::<snapshot::Snapshot>())
            .and_then(|snapshot| snapshot.restore())
        {
            if facts.states.len() == registry.len()
                && facts.air
                    == registry
                        .default_state("minecraft:air")
                        .unwrap_or(BlockStateId(u32::MAX))
                && (!project || projection.len() == registry.len())
            {
                log::info!(
                    "load assets: block facts snapshot hit {} ms",
                    cache_started.elapsed().as_millis()
                );
                log::info!(
                    "Horizon: stage=block_facts cache=hit elapsed_ms={}",
                    started.elapsed().as_millis()
                );
                return Ok((facts, projection));
            }
        }
        log::info!(
            "load assets: block facts snapshot miss {} ms",
            cache_started.elapsed().as_millis()
        );
        let load_started = std::time::Instant::now();
        let loaded = Self::load_projected_uncached(root, registry, behaviours, project)?;
        log::info!(
            "load assets: block facts source decode {} ms",
            load_started.elapsed().as_millis()
        );
        if let Some(cache) = cache.as_ref() {
            let snapshot_started = std::time::Instant::now();
            let snapshot = snapshot::Snapshot::capture(&loaded.0, &loaded.1);
            let _ = cache.write_value(&snapshot);
            log::info!(
                "load assets: block facts snapshot write {} ms",
                snapshot_started.elapsed().as_millis()
            );
        }
        log::info!(
            "Horizon: stage=block_facts cache=miss elapsed_ms={}",
            started.elapsed().as_millis()
        );
        Ok(loaded)
    }

    fn load_projected_uncached(
        root: &Path,
        registry: &BlockRegistry,
        behaviours: &Value,
        project: bool,
    ) -> Result<(Self, Vec<GameplayProjection>)> {
        let mut reader = BufReader::new(File::open(root.join("gameplay-facts.json"))?);
        // Keep the original full-document numeric/depth/syntax checks and error
        // precedence, without holding the source bytes or all decoded rows.
        serde_json::from_reader::<_, decode::Validate>(&mut reader)?;
        reader.rewind()?;
        // Class chains and per-state behaviour flags exported from the actual block objects.
        let behaviour_states = behaviours["states"]
            .as_array()
            .filter(|s| s.len() == registry.len());
        let mut class_names: HashMap<String, Arc<str>> = HashMap::new();
        let classes: Option<HashMap<&str, Arc<[Arc<str>]>>> =
            behaviours["blocks"].as_array().map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| {
                        let chain =
                            b["class"]
                                .as_array()?
                                .iter()
                                .filter_map(Value::as_str)
                                .map(|c| {
                                    class_names
                                        .entry(c.to_owned())
                                        .or_insert_with(|| Arc::from(c))
                                        .clone()
                                });
                        Some((b["name"].as_str()?, chain.collect()))
                    })
                    .collect()
            });
        let mut names: HashMap<String, Arc<str>> = HashMap::new();
        let mut sounds_by_value: HashMap<SoundKey, Arc<Sounds>> = HashMap::new();
        let mut block_types = BlockTypes::default();
        let mut shapes = decode::Shapes::default();
        let empty_shape: Arc<[[f64; 6]]> = Arc::from([]);
        let unit_cube_shape: Arc<[[f64; 6]]> = Arc::from([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
        let unit_cube_edges: Arc<[[f64; 6]]> = cube_edges().into();
        let mut states = Vec::with_capacity(registry.len());
        let mut projection = Vec::new();
        let mut projection_names = HashMap::new();
        let mut count = None;
        let mut typed_error = None;
        let mut row_error = None;
        decode::stream(reader, |event| {
            let raw = match event {
                decode::Row::Reset => {
                    count = None;
                    states.clear();
                    projection.clear();
                    names.clear();
                    projection_names.clear();
                    sounds_by_value.clear();
                    block_types = BlockTypes::default();
                    shapes = decode::Shapes::default();
                    row_error = None;
                    return;
                }
                decode::Row::Array => {
                    count = Some(0usize);
                    return;
                }
                decode::Row::Entry(raw) => raw,
            };
            let index = count.unwrap_or(0);
            count = Some(index + 1);
            let entry: decode::Entry<'_> = match serde_json::from_str(raw.get()) {
                Ok(entry) => entry,
                Err(error) => {
                    if typed_error.is_none() {
                        typed_error = Some(error);
                    }
                    return;
                }
            };
            // Even a wrong-length/overwritten array still gets fully decoded.
            // Domain failures are reported only after the final array's length
            // and the behaviour document have passed their original checks.
            let (Some(behaviour_states), Some(classes)) = (behaviour_states, classes.as_ref())
            else {
                return;
            };
            if index >= registry.len() || row_error.is_some() {
                return;
            }
            let built = (|| -> Result<()> {
                let mut intern = |s: &str| -> Arc<str> {
                    if let Some(name) = names.get(s) {
                        return name.clone();
                    }
                    let name: Arc<str> = Arc::from(s);
                    names.insert(s.to_owned(), name.clone());
                    name
                };
                let id = BlockStateId(index as u32);
                if entry.id != Some(index as u64) {
                    return Err("gameplay facts are not in registry order".into());
                }
                let state = registry.state(id).ok_or("missing registry state")?;
                let sound = &entry.sound;
                let prop = |k: &str| state.properties.get(k).map(String::as_str);
                let (fluid, fluid_level, fluid_falling) = match state.name.as_str() {
                    "minecraft:water" | "minecraft:lava" => {
                        let level: u8 = prop("level").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let kind = if state.name == "minecraft:water" {
                            Fluid::Water
                        } else {
                            Fluid::Lava
                        };
                        // LiquidBlock: level 0 is the source; 1..7 flowing; >=8 falling.
                        if level == 0 {
                            (kind, 8, false)
                        } else if level >= 8 {
                            (kind, 8, true)
                        } else {
                            (kind, 8 - level, false)
                        }
                    }
                    "minecraft:bubble_column"
                    | "minecraft:kelp"
                    | "minecraft:kelp_plant"
                    | "minecraft:seagrass"
                    | "minecraft:tall_seagrass" => (Fluid::Water, 8, false),
                    _ if prop("waterlogged") == Some("true") => (Fluid::Water, 8, false),
                    _ => (Fluid::None, 0, false),
                };
                let air = matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                );
                let cube = move || {
                    if air {
                        Vec::new()
                    } else {
                        vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
                    }
                };
                let edges = move || if air { Vec::new() } else { cube_edges() };
                let outline = match entry.outline {
                    Some(raw) => shapes.intern(Some(raw), cube)?,
                    None => {
                        if air {
                            empty_shape.clone()
                        } else {
                            unit_cube_shape.clone()
                        }
                    }
                };
                let outline_edges = match entry.outline_edges {
                    Some(raw) => shapes.intern(Some(raw), edges)?,
                    None => {
                        if air {
                            empty_shape.clone()
                        } else {
                            unit_cube_edges.clone()
                        }
                    }
                };
                // The export holds MapColor.col; ids are positions in MAP_COLORS.
                let col = entry.map_color.unwrap_or(0) as u32;
                let map_color = if col == 0 {
                    0
                } else {
                    MAP_COLORS.iter().position(|c| *c == col).unwrap_or(0) as u8
                };
                states.push(StateFacts {
                    map_color,
                    outline,
                    outline_edges,
                    emission: entry.emission.as_f64().unwrap_or(0.0) as u8,
                    dampening: entry.dampening.as_f64().unwrap_or(0.0) as u8,
                    skylight_down: entry.skylight_down,
                    shape_occlusion: entry.shape_occlusion,
                    can_occlude: entry.can_occlude,
                    solid_render: entry.solid_render,
                    replaceable: entry.replaceable,
                    full_collision: entry.full_collision,
                    fluid,
                    fluid_level,
                    fluid_falling,
                    waterlogged: prop("waterlogged") == Some("true"),
                    legacy_solid: behaviour_states[index][1].as_u64().unwrap_or(0) & 1 != 0,
                    sturdy_full: (behaviour_states[index][0].as_u64().unwrap_or(0) & 63) as u8,
                    block: block_types.intern(BlockTypeFacts {
                        name: intern(&state.name),
                        hardness: entry.hardness.as_f64().unwrap_or(0.0) as f32,
                        requires_tool: entry.requires_tool,
                        random_ticks: entry.random_ticks,
                        block_entity: entry.block_entity,
                        render_shape: match entry.render_shape.as_deref() {
                            Some("INVISIBLE") => RenderShape::Invisible,
                            Some("ENTITYBLOCK_ANIMATED") => RenderShape::Animated,
                            _ => RenderShape::Model,
                        },
                        friction: entry.friction as f32,
                        explosion_resistance: entry.explosion_resistance as f32,
                        item: intern(entry.item.as_deref().unwrap_or("minecraft:air")),
                        sounds: {
                            let volume = sound.volume as f32;
                            let pitch = sound.pitch as f32;
                            let break_sound = intern(sound.r#break.id.as_deref().unwrap_or(""));
                            let step = intern(sound.step.id.as_deref().unwrap_or(""));
                            let place = intern(sound.place.id.as_deref().unwrap_or(""));
                            let hit = intern(sound.hit.id.as_deref().unwrap_or(""));
                            let fall = intern(sound.fall.id.as_deref().unwrap_or(""));
                            sounds_by_value
                                .entry((
                                    volume.to_bits(),
                                    pitch.to_bits(),
                                    break_sound.clone(),
                                    step.clone(),
                                    place.clone(),
                                    hit.clone(),
                                    fall.clone(),
                                ))
                                .or_insert_with(|| {
                                    Arc::new(Sounds {
                                        volume,
                                        pitch,
                                        break_sound,
                                        step,
                                        place,
                                        hit,
                                        fall,
                                    })
                                })
                                .clone()
                        },
                        random_offset: entry.offset,
                        outline_offset: entry.outline_offset.unwrap_or(entry.offset),
                        classes: classes
                            .get(state.name.as_str())
                            .cloned()
                            .ok_or("missing block behaviour class chain")?,
                    }),
                });
                if project {
                    projection.push(GameplayProjection::from_entry(
                        &entry,
                        &mut projection_names,
                    ));
                }
                Ok(())
            })();
            row_error = built.err();
        })?;
        if let Some(error) = typed_error {
            return Err(error.into());
        }
        let count = count.ok_or("missing gameplay state facts")?;
        if count != registry.len() {
            return Err("gameplay facts do not match the block registry".into());
        }
        behaviour_states.ok_or("block behaviour facts do not match the block registry")?;
        classes.ok_or("missing block behaviour classes")?;
        if let Some(error) = row_error {
            return Err(error);
        }
        Ok((
            Self {
                states,
                air: registry
                    .default_state("minecraft:air")
                    .ok_or("missing air")?,
            },
            projection,
        ))
    }
    /// A state past the table (a modded server's id that reached here
    /// unmapped) reads as state 0, air, instead of ending the game.
    pub fn get(&self, id: BlockStateId) -> &StateFacts {
        self.states.get(id.0 as usize).unwrap_or(&self.states[0])
    }
    pub fn is_air(&self, id: BlockStateId) -> bool {
        matches!(
            &*self.get(id).name,
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    }
}

impl GameplayStateSource for decode::Entry<'_> {
    fn flag(&self, name: &str) -> bool {
        match name {
            "replaceable" => self.replaceable,
            "random_ticks" => self.random_ticks,
            "solid_render" => self.solid_render,
            "liquid" => self.liquid,
            "block_entity" => self.block_entity,
            "ignited_by_lava" => self.ignited_by_lava,
            "can_occlude" => self.can_occlude,
            "full_collision" => self.full_collision,
            "skylight_down" => self.skylight_down,
            "requires_tool" => self.requires_tool,
            "shape_occlusion" => self.shape_occlusion,
            _ => false,
        }
    }
    fn unsigned(&self, name: &str) -> u64 {
        match name {
            "emission" => self.emission.as_u64().unwrap_or(0),
            "dampening" => self.dampening.as_u64().unwrap_or(0),
            _ => 0,
        }
    }
    fn number(&self, name: &str) -> f64 {
        match name {
            "hardness" => self.hardness.as_f64().unwrap_or(0.0),
            _ => 0.0,
        }
    }
    fn text(&self, name: &str) -> &str {
        match name {
            "push" => self.push.as_deref().unwrap_or(""),
            _ => "",
        }
    }
    fn sound_number(&self, name: &str) -> f64 {
        match name {
            "volume" => self.sound.volume,
            "pitch" => self.sound.pitch,
            _ => 1.0,
        }
    }
    fn sound_id(&self, name: &str) -> &str {
        match name {
            "place" => self.sound.place.id.as_deref().unwrap_or(""),
            "break" => self.sound.r#break.id.as_deref().unwrap_or(""),
            "fall" => self.sound.fall.id.as_deref().unwrap_or(""),
            _ => "",
        }
    }
}

#[cfg(test)]
#[path = "blocks/stream_tests.rs"]
mod stream_tests;
