//! Block-space AABBs from the dev-only NeoForge FactsExporter.
use crate::{
    chunk::BlockState,
    compression::invalid,
    registry::{BlockRegistry, BlockStateId},
};
use horizon_core::geometry::{Axis, VoxelShape};
use serde::Deserialize;
use std::{
    collections::{HashMap, hash_map::Entry},
    fs, io,
    path::Path,
    sync::{Arc, OnceLock},
};

#[derive(Default)]
pub struct OracleShapes(HashMap<BlockStateId, StateShapes>);

type Boxes = Vec<[f64; 6]>;
type SharedBoxes = Arc<[[f64; 6]]>;
/// Block-space picking boxes and highlight edges, respectively.
pub type OutlineGeometry = (SharedBoxes, SharedBoxes);
type ShapePool = HashMap<Vec<[u64; 6]>, Arc<Shape>>;

#[derive(Default, Deserialize)]
struct RawShapes {
    collision: Option<Boxes>,
    outline: Option<Boxes>,
}

#[derive(Deserialize)]
struct Neighbours {
    sides: u8,
    palette: Vec<RawShapes>,
    masks: HashMap<u8, usize>,
}

#[derive(Deserialize)]
struct RawState {
    collision: Option<Boxes>,
    outline: Option<Boxes>,
    neighbours: Option<Neighbours>,
}

#[derive(Deserialize)]
struct Block {
    states: HashMap<String, RawState>,
}

#[derive(Deserialize)]
struct Facts {
    blocks: HashMap<String, Block>,
}

struct Shapes {
    collision: Option<Arc<Shape>>,
    outline: Option<Arc<Shape>>,
}

struct Shape {
    voxel: VoxelShape,
    outline: OnceLock<OutlineGeometry>,
}

struct StateShapes {
    base: Shapes,
    profiles: Option<Box<Profiles>>,
}

struct Profiles {
    sides: u8,
    palette: Vec<Shapes>,
    // -1 denotes the original shape. Values of sides not sampled are discarded.
    masks: [i8; 64],
}

impl Shape {
    fn outline_geometry(&self) -> &OutlineGeometry {
        self.outline.get_or_init(|| {
            let boxes = self
                .voxel
                .boxes()
                .iter()
                .map(|aabb| {
                    let lo = aabb.min_position();
                    let hi = aabb.max_position();
                    [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
                })
                .collect();
            (boxes, outline_edges(&self.voxel).into())
        })
    }
}

impl Shapes {
    fn decode(raw: RawShapes, pool: &mut ShapePool) -> io::Result<Self> {
        let mut intern = |boxes: Boxes| {
            let key = boxes.iter().map(|b| b.map(f64::to_bits)).collect();
            let shape = match pool.entry(key) {
                Entry::Occupied(entry) => entry.get().clone(),
                Entry::Vacant(entry) => entry
                    .insert(Arc::new(Shape {
                        voxel: voxels(boxes)?,
                        outline: OnceLock::new(),
                    }))
                    .clone(),
            };
            Ok::<_, io::Error>(shape)
        };
        Ok(Self {
            collision: raw.collision.map(&mut intern).transpose()?,
            outline: raw.outline.map(intern).transpose()?,
        })
    }
}

fn voxels(boxes: Boxes) -> io::Result<VoxelShape> {
    if boxes.is_empty() {
        return Ok(VoxelShape::empty());
    }
    if boxes == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]] {
        return Ok(VoxelShape::block());
    }
    if boxes
        .iter()
        .any(|b| b.iter().any(|v| !v.is_finite()) || (0..3).any(|axis| b[axis] >= b[axis + 3]))
    {
        return Err(invalid("invalid NeoForge shape box"));
    }
    // Use the exported boundaries directly. Shapes.create can snap box
    // coordinates near one of its supported fractions.
    let coordinates = std::array::from_fn::<_, 3, _>(|axis| {
        let mut points: Vec<_> = boxes.iter().flat_map(|b| [b[axis], b[axis + 3]]).collect();
        points.sort_unstable_by(f64::total_cmp);
        points.dedup();
        points
    });
    let [nx, ny, nz] = coordinates.each_ref().map(|points| points.len() - 1);
    let cells = nx
        .checked_mul(ny)
        .and_then(|n| n.checked_mul(nz))
        .filter(|&n| n <= 1_048_576)
        .ok_or_else(|| invalid("NeoForge shape grid is too large"))?;
    let mut occupied = vec![false; cells];
    for b in boxes {
        let [x, y, z] = std::array::from_fn(|axis| {
            let find = |coordinate| {
                coordinates[axis]
                    .binary_search_by(|v| v.partial_cmp(&coordinate).unwrap())
                    .unwrap()
            };
            find(b[axis])..find(b[axis + 3])
        });
        for x in x {
            for y in y.clone() {
                let row = (x * ny + y) * nz;
                occupied[(row + z.start)..(row + z.end)].fill(true);
            }
        }
    }
    VoxelShape::from_grid(coordinates, &occupied).map_err(io::Error::other)
}

impl StateShapes {
    fn decode(raw: RawState, pool: &mut ShapePool) -> io::Result<Self> {
        let base = Shapes::decode(
            RawShapes {
                collision: raw.collision,
                outline: raw.outline,
            },
            pool,
        )?;
        let profiles = raw
            .neighbours
            .map(|data| Profiles::decode(data, pool))
            .transpose()?;
        Ok(Self {
            base,
            profiles: profiles.map(Box::new),
        })
    }

    fn select(&self, mask: u8, shape: fn(&Shapes) -> Option<&Shape>) -> Option<&Shape> {
        self.variant(mask)
            .and_then(shape)
            .or_else(|| shape(&self.base))
    }

    fn variant(&self, mask: u8) -> Option<&Shapes> {
        self.profiles.as_ref()?.select(mask)
    }
}

impl Profiles {
    fn decode(raw: Neighbours, pool: &mut ShapePool) -> io::Result<Self> {
        if raw.sides >= 64 || raw.palette.len() > 64 {
            return Err(invalid("invalid NeoForge shape profile"));
        }
        let mut masks = [-1; 64];
        for (mask, index) in raw.masks {
            if mask & raw.sides != mask || index >= raw.palette.len() {
                return Err(invalid("invalid NeoForge shape mask or palette index"));
            }
            masks[usize::from(mask)] = index as i8;
        }
        let palette = raw
            .palette
            .into_iter()
            .map(|v| Shapes::decode(v, pool))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            sides: raw.sides,
            palette,
            masks,
        })
    }

    fn select(&self, mask: u8) -> Option<&Shapes> {
        let index = self.masks[usize::from(mask & self.sides)];
        self.palette.get(index as usize)
    }
}

impl OracleShapes {
    /// Open the explicitly selected export, or use the normal prepared tables.
    /// The path is independent of Horizon's generated mod-content roots.
    pub fn load(registry: &BlockRegistry) -> io::Result<Self> {
        let Some(path) = std::env::var_os("HORIZON_NEOFORGE_FACTS") else {
            return Ok(Self::default());
        };
        let path = Path::new(&path);
        Self::from_json(&fs::read(path)?, registry)
            .map_err(|error| invalid(format!("{}: {error}", path.display())))
    }

    pub fn from_json(json: &[u8], registry: &BlockRegistry) -> io::Result<Self> {
        let facts: Facts = serde_json::from_slice(json)?;
        let mut shapes = HashMap::new();
        let mut pool = ShapePool::new();
        for (name, block) in facts.blocks {
            if name.starts_with("minecraft:") || registry.default_state(&name).is_none() {
                continue;
            }
            let mut state = BlockState {
                name,
                properties: Default::default(),
            };
            for (key, raw) in block.states {
                state.properties.clear();
                if !key.is_empty() {
                    for pair in key.split(',') {
                        let (name, value) = pair
                            .split_once('=')
                            .ok_or_else(|| invalid("invalid NeoForge state property"))?;
                        if state.properties.insert(name.into(), value.into()).is_some() {
                            return Err(invalid("duplicate NeoForge state property"));
                        }
                    }
                }
                if let Some(id) = registry.id(&state)
                    && shapes
                        .insert(id, StateShapes::decode(raw, &mut pool)?)
                        .is_some()
                {
                    return Err(invalid("duplicate NeoForge state"));
                }
            }
        }
        Ok(Self(shapes))
    }

    pub fn collision(&self, state: BlockStateId, mask: u8) -> Option<&VoxelShape> {
        Some(
            &self
                .0
                .get(&state)?
                .select(mask, |s| s.collision.as_deref())?
                .voxel,
        )
    }

    pub fn outline(&self, state: BlockStateId, mask: u8) -> Option<&VoxelShape> {
        Some(
            &self
                .0
                .get(&state)?
                .select(mask, |s| s.outline.as_deref())?
                .voxel,
        )
    }

    pub fn outline_geometry(&self, state: BlockStateId, mask: u8) -> Option<OutlineGeometry> {
        let shape = self.0.get(&state)?.select(mask, |s| s.outline.as_deref())?;
        Some(shape.outline_geometry().clone())
    }

    pub(crate) fn outline_override(
        &self,
        state: BlockStateId,
        mask: u8,
    ) -> Option<&OutlineGeometry> {
        let state = self.0.get(&state)?;
        let outline = |mask| state.variant(mask)?.outline.as_deref();
        let shape = outline(mask).or_else(|| {
            // The static facts include any mask-zero outline. Restore the base
            // outline when that variant is no longer selected.
            outline(0)?;
            state.base.outline.as_deref()
        })?;
        Some(shape.outline_geometry())
    }

    pub fn collision_is_full(&self, state: BlockStateId, mask: u8) -> Option<bool> {
        self.collision(state, mask)
            .map(|shape| shape.boxes() == VoxelShape::block().boxes())
    }

    pub fn states(&self) -> impl Iterator<Item = BlockStateId> + '_ {
        self.0.keys().copied()
    }

    pub(crate) fn collision_sides(&self, state: BlockStateId) -> u8 {
        self.sides_with(state, |s| s.collision.is_some())
            .unwrap_or(0)
    }

    pub(crate) fn outline_sides(&self, state: BlockStateId) -> Option<u8> {
        self.sides_with(state, |s| s.outline.is_some())
    }

    fn sides_with(
        &self,
        state: BlockStateId,
        has_shape: impl FnMut(&Shapes) -> bool,
    ) -> Option<u8> {
        let profiles = self.0.get(&state)?.profiles.as_ref()?;
        profiles
            .palette
            .iter()
            .any(has_shape)
            .then_some(profiles.sides)
    }

    pub(crate) fn any_collision_variant(
        &self,
        state: BlockStateId,
        predicate: impl FnMut(&VoxelShape) -> bool,
    ) -> bool {
        self.0
            .get(&state)
            .and_then(|s| s.profiles.as_ref())
            .is_some_and(|s| {
                s.palette
                    .iter()
                    .filter_map(|v| v.collision.as_ref())
                    .map(|s| &s.voxel)
                    .any(predicate)
            })
    }
}

fn outline_edges(shape: &VoxelShape) -> Vec<[f64; 6]> {
    if shape.is_empty() {
        return Vec::new();
    }
    let sizes = shape.sizes();
    let coords = Axis::ALL.map(|axis| shape.coordinates(axis));
    let mut edges = Vec::new();
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for u in 0..=sizes[b] {
            for v in 0..=sizes[c] {
                let mut start = None;
                for along in 0..=sizes[axis] {
                    // A convex/concave corner or two diagonally touching cells.
                    let cells = [(0, 0), (-1, 0), (0, -1), (-1, -1)].map(|(du, dv)| {
                        let mut p = [0; 3];
                        p[axis] = along as i32;
                        p[b] = u as i32 + du;
                        p[c] = v as i32 + dv;
                        shape.is_full(p[0], p[1], p[2])
                    });
                    let count = cells.iter().filter(|&&full| full).count();
                    if count % 2 == 1 || (count == 2 && cells[0] == cells[3]) {
                        start.get_or_insert(along);
                    } else if let Some(from) = start.take() {
                        let mut edge = [0.0; 6];
                        edge[b] = coords[b][u];
                        edge[c] = coords[c][v];
                        edge[axis] = coords[axis][from];
                        edge.copy_within(..3, 3);
                        edge[axis + 3] = coords[axis][along];
                        edges.push(edge);
                    }
                }
            }
        }
    }
    edges
}
