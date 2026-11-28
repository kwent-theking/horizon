//! Private binary wire records. Shared allocations are indexed once, never
//! expanded once per block state. Every index is checked before publication.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
pub(super) struct Snapshot {
    strings: Vec<String>,
    shapes: Vec<Vec<[f64; 6]>>,
    classes: Vec<Vec<u32>>,
    sounds: Vec<Sound>,
    blocks: Vec<Block>,
    states: Vec<State>,
    projection: Vec<Projection>,
    air: u32,
}

#[derive(Serialize, Deserialize)]
struct State {
    block: u32,
    emission: u8,
    dampening: u8,
    skylight_down: bool,
    shape_occlusion: bool,
    can_occlude: bool,
    solid_render: bool,
    replaceable: bool,
    full_collision: bool,
    fluid: Fluid,
    fluid_level: u8,
    fluid_falling: bool,
    waterlogged: bool,
    legacy_solid: bool,
    sturdy_full: u8,
    outline: u32,
    outline_edges: u32,
    map_color: u8,
}

#[derive(Serialize, Deserialize)]
struct Block {
    name: u32,
    hardness: f32,
    requires_tool: bool,
    random_ticks: bool,
    block_entity: bool,
    render_shape: RenderShape,
    friction: f32,
    explosion_resistance: f32,
    item: u32,
    sounds: u32,
    random_offset: bool,
    outline_offset: bool,
    classes: u32,
}

#[derive(Serialize, Deserialize)]
struct Sound {
    volume: f32,
    pitch: f32,
    break_sound: u32,
    step: u32,
    place: u32,
    hit: u32,
    fall: u32,
}

#[derive(Serialize, Deserialize)]
struct Projection {
    flags: u16,
    emission: u64,
    dampening: u64,
    hardness: f64,
    push: u32,
    volume: f64,
    pitch: f64,
    place: u32,
    break_sound: u32,
    fall: u32,
}

#[derive(Default)]
struct Pools {
    snapshot: Snapshot,
    strings: HashMap<Arc<str>, u32>,
    shapes: HashMap<usize, u32>,
    shapes_by_bits: HashMap<Vec<[u64; 6]>, u32>,
    classes: HashMap<usize, u32>,
    sounds: HashMap<usize, u32>,
    blocks: HashMap<usize, u32>,
}
impl Pools {
    fn strings(&mut self, value: &Arc<str>) -> u32 {
        if let Some(&id) = self.strings.get(value) {
            return id;
        }
        let id = self.snapshot.strings.len() as u32;
        self.snapshot.strings.push(value.to_string());
        self.strings.insert(value.clone(), id);
        id
    }
    fn shapes(&mut self, value: &Arc<[[f64; 6]]>) -> u32 {
        let key = value.as_ptr() as usize;
        if let Some(&id) = self.shapes.get(&key) {
            return id;
        }
        let bits: Vec<[u64; 6]> = value.iter().map(|b| b.map(f64::to_bits)).collect();
        let id = match self.shapes_by_bits.entry(bits) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let id = self.snapshot.shapes.len() as u32;
                self.snapshot.shapes.push(value.to_vec());
                entry.insert(id);
                id
            }
        };
        self.shapes.insert(key, id);
        id
    }
    fn classes(&mut self, value: &Arc<[Arc<str>]>) -> u32 {
        let key = value.as_ptr() as usize;
        if let Some(&id) = self.classes.get(&key) {
            return id;
        }
        let record = value.iter().map(|v| self.strings(v)).collect();
        let id = self.snapshot.classes.len() as u32;
        self.snapshot.classes.push(record);
        self.classes.insert(key, id);
        id
    }
    fn sounds(&mut self, value: &Arc<Sounds>) -> u32 {
        let key = Arc::as_ptr(value) as usize;
        if let Some(&id) = self.sounds.get(&key) {
            return id;
        }
        let record = Sound {
            volume: value.volume,
            pitch: value.pitch,
            break_sound: self.strings(&value.break_sound),
            step: self.strings(&value.step),
            place: self.strings(&value.place),
            hit: self.strings(&value.hit),
            fall: self.strings(&value.fall),
        };
        let id = self.snapshot.sounds.len() as u32;
        self.snapshot.sounds.push(record);
        self.sounds.insert(key, id);
        id
    }
    fn blocks(&mut self, value: &Arc<BlockTypeFacts>) -> u32 {
        let key = Arc::as_ptr(value) as usize;
        if let Some(&id) = self.blocks.get(&key) {
            return id;
        }
        let record = Block {
            name: self.strings(&value.name),
            hardness: value.hardness,
            requires_tool: value.requires_tool,
            random_ticks: value.random_ticks,
            block_entity: value.block_entity,
            render_shape: value.render_shape,
            friction: value.friction,
            explosion_resistance: value.explosion_resistance,
            item: self.strings(&value.item),
            sounds: self.sounds(&value.sounds),
            random_offset: value.random_offset,
            outline_offset: value.outline_offset,
            classes: self.classes(&value.classes),
        };
        let id = self.snapshot.blocks.len() as u32;
        self.snapshot.blocks.push(record);
        self.blocks.insert(key, id);
        id
    }
}
impl Snapshot {
    pub(super) fn capture(facts: &BlockFacts, projection: &[GameplayProjection]) -> Self {
        let mut pools = Pools::default();
        let states = facts
            .states
            .iter()
            .map(|value| State {
                block: pools.blocks(&value.block),
                emission: value.emission,
                dampening: value.dampening,
                skylight_down: value.skylight_down,
                shape_occlusion: value.shape_occlusion,
                can_occlude: value.can_occlude,
                solid_render: value.solid_render,
                replaceable: value.replaceable,
                full_collision: value.full_collision,
                fluid: value.fluid,
                fluid_level: value.fluid_level,
                fluid_falling: value.fluid_falling,
                waterlogged: value.waterlogged,
                legacy_solid: value.legacy_solid,
                sturdy_full: value.sturdy_full,
                outline: pools.shapes(&value.outline),
                outline_edges: pools.shapes(&value.outline_edges),
                map_color: value.map_color,
            })
            .collect();
        let projection = projection
            .iter()
            .map(|value| {
                let value = &value.0;
                Projection {
                    flags: value.flags,
                    emission: value.emission,
                    dampening: value.dampening,
                    hardness: f64::from_bits(value.hardness),
                    push: pools.strings(&value.push),
                    volume: f64::from_bits(value.volume),
                    pitch: f64::from_bits(value.pitch),
                    place: pools.strings(&value.place),
                    break_sound: pools.strings(&value.break_sound),
                    fall: pools.strings(&value.fall),
                }
            })
            .collect();
        pools.snapshot.states = states;
        pools.snapshot.projection = projection;
        pools.snapshot.air = facts.air.0;
        pools.snapshot
    }
    pub(super) fn restore(self) -> Option<(BlockFacts, Vec<GameplayProjection>)> {
        // Validate the complete graph before allocating its Arc-backed runtime
        // form. A checksum detects accidental corruption, not a validly
        // checksummed payload with out-of-range cross references.
        fn in_range(index: u32, len: usize) -> bool {
            (index as usize) < len
        }
        if self.states.is_empty()
            || self.air as usize >= self.states.len()
            || (!self.projection.is_empty() && self.projection.len() != self.states.len())
            || self
                .shapes
                .iter()
                .flatten()
                .flatten()
                .any(|v| !v.is_finite())
            || self.sounds.iter().any(|s| {
                !s.volume.is_finite()
                    || !s.pitch.is_finite()
                    || !in_range(s.break_sound, self.strings.len())
                    || !in_range(s.step, self.strings.len())
                    || !in_range(s.place, self.strings.len())
                    || !in_range(s.hit, self.strings.len())
                    || !in_range(s.fall, self.strings.len())
            })
            || self.blocks.iter().any(|b| {
                !b.hardness.is_finite()
                    || !b.friction.is_finite()
                    || !b.explosion_resistance.is_finite()
                    || !in_range(b.name, self.strings.len())
                    || !in_range(b.item, self.strings.len())
                    || !in_range(b.sounds, self.sounds.len())
                    || !in_range(b.classes, self.classes.len())
            })
            || self
                .classes
                .iter()
                .flatten()
                .any(|&s| !in_range(s, self.strings.len()))
            || self.states.iter().any(|s| {
                !in_range(s.block, self.blocks.len())
                    || !in_range(s.outline, self.shapes.len())
                    || !in_range(s.outline_edges, self.shapes.len())
            })
            || self.projection.iter().any(|p| {
                !p.hardness.is_finite()
                    || !p.volume.is_finite()
                    || !p.pitch.is_finite()
                    || !in_range(p.push, self.strings.len())
                    || !in_range(p.place, self.strings.len())
                    || !in_range(p.break_sound, self.strings.len())
                    || !in_range(p.fall, self.strings.len())
            })
        {
            return None;
        }
        let strings: Vec<Arc<str>> = self.strings.into_iter().map(Arc::from).collect();
        let shapes: Vec<Arc<[[f64; 6]]>> = self.shapes.into_iter().map(Arc::from).collect();
        let mut class_chains = HashSet::new();
        let classes: Vec<Arc<[Arc<str>]>> = self
            .classes
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|id| strings.get(id as usize).cloned())
                    .collect::<Option<Vec<_>>>()
                    .map(|chain| intern_classes(&mut class_chains, chain))
            })
            .collect::<Option<_>>()?;
        drop(class_chains);
        let sounds: Vec<Arc<Sounds>> = self
            .sounds
            .into_iter()
            .map(|row| {
                Some(Arc::new(Sounds {
                    volume: row.volume,
                    pitch: row.pitch,
                    break_sound: strings.get(row.break_sound as usize)?.clone(),
                    step: strings.get(row.step as usize)?.clone(),
                    place: strings.get(row.place as usize)?.clone(),
                    hit: strings.get(row.hit as usize)?.clone(),
                    fall: strings.get(row.fall as usize)?.clone(),
                }))
            })
            .collect::<Option<_>>()?;
        let blocks: Vec<Arc<BlockTypeFacts>> = self
            .blocks
            .into_iter()
            .map(|row| {
                Some(Arc::new(BlockTypeFacts {
                    name: strings.get(row.name as usize)?.clone(),
                    hardness: row.hardness,
                    requires_tool: row.requires_tool,
                    random_ticks: row.random_ticks,
                    block_entity: row.block_entity,
                    render_shape: row.render_shape,
                    friction: row.friction,
                    explosion_resistance: row.explosion_resistance,
                    item: strings.get(row.item as usize)?.clone(),
                    sounds: sounds.get(row.sounds as usize)?.clone(),
                    random_offset: row.random_offset,
                    outline_offset: row.outline_offset,
                    classes: classes.get(row.classes as usize)?.clone(),
                }))
            })
            .collect::<Option<_>>()?;
        drop((classes, sounds));
        let mut state_records = StateRecords::default();
        let states: Vec<StateFacts> = self
            .states
            .into_iter()
            .map(|row| {
                Some(state_records.intern(StateRecord {
                    block: blocks.get(row.block as usize)?.clone(),
                    emission: row.emission,
                    dampening: row.dampening,
                    skylight_down: row.skylight_down,
                    shape_occlusion: row.shape_occlusion,
                    can_occlude: row.can_occlude,
                    solid_render: row.solid_render,
                    replaceable: row.replaceable,
                    full_collision: row.full_collision,
                    fluid: row.fluid,
                    fluid_level: row.fluid_level,
                    fluid_falling: row.fluid_falling,
                    waterlogged: row.waterlogged,
                    legacy_solid: row.legacy_solid,
                    sturdy_full: row.sturdy_full,
                    outline: shapes.get(row.outline as usize)?.clone(),
                    outline_edges: shapes.get(row.outline_edges as usize)?.clone(),
                    map_color: row.map_color,
                }))
            })
            .collect::<Option<_>>()?;
        drop((state_records, blocks, shapes));
        let mut gameplay_records = HashSet::new();
        let projection: Vec<GameplayProjection> = self
            .projection
            .into_iter()
            .map(|row| {
                let value = GameplayInputs {
                    flags: row.flags,
                    emission: row.emission,
                    dampening: row.dampening,
                    hardness: row.hardness.to_bits(),
                    push: strings.get(row.push as usize)?.clone(),
                    volume: row.volume.to_bits(),
                    pitch: row.pitch.to_bits(),
                    place: strings.get(row.place as usize)?.clone(),
                    break_sound: strings.get(row.break_sound as usize)?.clone(),
                    fall: strings.get(row.fall as usize)?.clone(),
                };
                Some(value.intern(&mut gameplay_records))
            })
            .collect::<Option<_>>()?;
        Some((
            BlockFacts {
                states,
                air: BlockStateId(self.air),
            },
            projection,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> BlockFacts {
        let sounds = Arc::new(Sounds {
            volume: 1.0,
            pitch: 1.0,
            break_sound: Arc::from("a:break"),
            step: Arc::from("a:step"),
            place: Arc::from("a:place"),
            hit: Arc::from("a:hit"),
            fall: Arc::from("a:fall"),
        });
        let block = Arc::new(BlockTypeFacts {
            name: Arc::from("a:block"),
            hardness: 1.0,
            requires_tool: false,
            random_ticks: false,
            block_entity: false,
            render_shape: RenderShape::Model,
            friction: 0.6,
            explosion_resistance: 1.0,
            item: Arc::from("a:block"),
            sounds,
            random_offset: false,
            outline_offset: false,
            classes: Arc::from([Arc::<str>::from("a:class")]),
        });
        let outline: Arc<[[f64; 6]]> = Arc::from([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
        let state = || {
            StateFacts(Arc::new(StateRecord {
                block: block.clone(),
                emission: 0,
                dampening: 0,
                skylight_down: true,
                shape_occlusion: true,
                can_occlude: true,
                solid_render: true,
                replaceable: false,
                full_collision: true,
                fluid: Fluid::None,
                fluid_level: 0,
                fluid_falling: false,
                waterlogged: false,
                legacy_solid: true,
                sturdy_full: 63,
                outline: outline.clone(),
                outline_edges: Arc::from([]),
                map_color: 0,
            }))
        };
        BlockFacts {
            states: vec![state(), state()],
            air: BlockStateId(0),
        }
    }

    #[test]
    fn pooled_round_trip_preserves_sharing() {
        let snapshot = Snapshot::capture(&fixture(), &[]);
        let (facts, projection) = snapshot.restore().unwrap();
        assert!(projection.is_empty());
        assert!(Arc::ptr_eq(&facts.states[0].block, &facts.states[1].block));
        assert!(Arc::ptr_eq(
            &facts.states[0].block.sounds,
            &facts.states[1].block.sounds
        ));
        assert!(Arc::ptr_eq(
            &facts.states[0].outline,
            &facts.states[1].outline
        ));
        assert!(Arc::ptr_eq(
            &facts.states[0].block.name,
            &facts.states[1].block.item
        ));
    }

    #[test]
    fn rejects_invalid_indices_and_non_finite_values_before_restore() {
        let mut snapshot = Snapshot::capture(&fixture(), &[]);
        snapshot.states[0].block = u32::MAX;
        assert!(snapshot.restore().is_none());

        let mut snapshot = Snapshot::capture(&fixture(), &[]);
        snapshot.blocks[0].hardness = f32::NAN;
        assert!(snapshot.restore().is_none());
    }

    #[test]
    fn value_dedup_shares_identical_distinct_outline_allocations() {
        let mut a = fixture();
        let distinct_outline: Arc<[[f64; 6]]> = Arc::from([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
        let distinct_empty: Arc<[[f64; 6]]> = Arc::from([]);
        a.states[1].outline = distinct_outline;
        a.states[1].outline_edges = distinct_empty;
        let snapshot = Snapshot::capture(&a, &[]);
        assert_eq!(snapshot.shapes.len(), 2);
        let (restored, _) = snapshot.restore().unwrap();
        assert!(Arc::ptr_eq(
            &restored.states[0].outline,
            &restored.states[1].outline
        ));
        assert!(Arc::ptr_eq(
            &restored.states[0].outline_edges,
            &restored.states[1].outline_edges
        ));
    }
}
