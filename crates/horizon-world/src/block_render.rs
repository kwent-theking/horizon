//! Neighbor face visibility over immutable block definitions and occlusion grids.
//! This decides whether a culled model face is emitted; it does not shade it or
//! implement translucent sorting, fluid meshing, or block updates.
use crate::{
    collision::CachedCollisionRegistry,
    compression::invalid,
    fluid::{FluidKind, FluidState},
    registry::BlockStateId,
};
use horizon_core::geometry::{Direction, ShapeError, ShapeOp, VoxelShape};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipRendering {
    Never,
    SameBlock,
    Bars,
    Leaves,
    Liquid,
    MangroveRoots,
}
#[derive(Clone, Copy, Debug)]
pub struct RenderDefinition {
    pub skip: SkipRendering,
    pub can_occlude: bool,
    pub bars_tag: bool,
}
#[derive(Clone, Copy)]
struct State {
    block: BlockStateId,
    face_set: u32,
    definition: RenderDefinition,
    connections: u8,
    connection_properties: u8,
    fluid: FluidKind,
}
/// Prepared face geometry and a bounded cache shared across state pairs. Geometry
/// is deduplicated by the imported immutable shape, not copied for every state.
#[derive(Clone)]
pub struct FaceVisibility {
    states: Arc<[State]>,
    face_sets: Arc<[[u32; 6]]>,
    faces: Arc<[VoxelShape]>,
    cache: HashMap<(u32, u32), bool>,
}
struct FacePool {
    faces: Vec<VoxelShape>,
    by_grid: HashMap<u64, u32>,
    hasher: std::hash::RandomState,
}
impl FacePool {
    fn new() -> Self {
        Self {
            faces: vec![VoxelShape::empty(), VoxelShape::block()],
            by_grid: HashMap::new(),
            hasher: std::hash::RandomState::new(),
        }
    }

    fn push(&mut self, mut face: VoxelShape) -> u32 {
        use std::{
            collections::hash_map::Entry,
            hash::{BuildHasher, Hasher},
        };
        let id = self.faces.len() as u32;
        let mut hasher = self.hasher.build_hasher();
        face.hash_grid(&mut hasher);
        let mut hash = hasher.finish();
        loop {
            match self.by_grid.entry(hash) {
                Entry::Occupied(entry) => {
                    if face.share_grid(&self.faces[*entry.get() as usize]) {
                        break;
                    }
                    hash = hash.wrapping_add(1);
                }
                Entry::Vacant(entry) => {
                    entry.insert(id);
                    break;
                }
            }
        }
        self.faces.push(face);
        id
    }
}
impl FaceVisibility {
    pub fn new(
        occlusion: &CachedCollisionRegistry<'_>,
        definitions: &BTreeMap<String, RenderDefinition>,
    ) -> io::Result<Self> {
        Self::build(occlusion, definitions, VoxelShape::face_shape)
    }

    /// Take the freshly loaded occlusion table and keep only its projected faces.
    pub fn from_owned_occlusion(
        occlusion: CachedCollisionRegistry<'_>,
        definitions: &BTreeMap<String, RenderDefinition>,
    ) -> io::Result<Self> {
        // `build` projects each source shape once. Avoid retaining a second set
        // of projections through the face cache of a face that aliases its source.
        Self::build(&occlusion, definitions, VoxelShape::project_face)
    }

    fn build(
        occlusion: &CachedCollisionRegistry<'_>,
        definitions: &BTreeMap<String, RenderDefinition>,
        project: fn(&VoxelShape, Direction) -> Result<VoxelShape, ShapeError>,
    ) -> io::Result<Self> {
        let registry = occlusion.registry();
        let mut states = Vec::with_capacity(registry.len());
        // IDs 0 and 1 represent the canonical empty/full face singletons used
        // by BlockState.initCache; other empty-like shapes retain their identity.
        let mut pool = FacePool::new();
        let mut face_sets = vec![[0; 6]];
        let mut by_set = HashMap::from([([0; 6], 0u32)]);
        let mut shape_faces = HashMap::new();
        for id in 0..registry.len() {
            let id = BlockStateId(id as u32);
            let state = registry
                .state(id)
                .ok_or_else(|| invalid("unknown render state"))?;
            let definition = *definitions
                .get(&state.name)
                .ok_or_else(|| invalid("missing render definition"))?;
            let shape = occlusion
                .cached_shape(id)?
                .ok_or_else(|| invalid("missing occlusion geometry"))?;
            let face_set = if !definition.can_occlude || shape.is_empty() {
                0
            } else if let Some(set) = shape_faces.get(&std::ptr::from_ref(shape)) {
                *set
            } else {
                let mut ids = [1; 6];
                if shape
                    .join_is_not_empty(&pool.faces[1], ShapeOp::XOR)
                    .map_err(|e| invalid(e.to_string()))?
                {
                    for direction in Direction::ALL {
                        let face = project(shape, direction).map_err(|e| invalid(e.to_string()))?;
                        ids[direction as usize] = if face.same_identity(&pool.faces[0]) {
                            0
                        } else if face.same_identity(&pool.faces[1]) {
                            1
                        } else {
                            pool.push(face)
                        };
                    }
                }
                let set = *by_set.entry(ids).or_insert_with(|| {
                    let set = face_sets.len() as u32;
                    face_sets.push(ids);
                    set
                });
                shape_faces.insert(std::ptr::from_ref(shape), set);
                set
            };
            let mut connections = 0;
            let mut connection_properties = 0;
            for (name, direction) in [
                ("north", Direction::North),
                ("south", Direction::South),
                ("west", Direction::West),
                ("east", Direction::East),
            ] {
                if let Some(value @ ("false" | "true")) =
                    state.properties.get(name).map(String::as_str)
                {
                    connection_properties |= 1 << direction as u8;
                    if value == "true" {
                        connections |= 1 << direction as u8;
                    }
                }
            }
            if definition.skip == SkipRendering::Bars && connection_properties != 0b111100 {
                return Err(invalid("bars rendering requires four boolean connections"));
            }
            states.push(State {
                block: registry
                    .default_state(&state.name)
                    .ok_or_else(|| invalid("missing render block identity"))?,
                face_set,
                definition,
                connections,
                connection_properties,
                fluid: FluidState::from_block(state)?.kind(),
            });
        }
        Ok(Self {
            states: states.into(),
            face_sets: face_sets.into(),
            faces: pool.faces.into(),
            cache: HashMap::new(),
        })
    }
    fn state(&self, id: BlockStateId) -> io::Result<State> {
        self.states
            .get(id.0 as usize)
            .copied()
            .ok_or_else(|| invalid("unknown render state ID"))
    }
    fn skips(a: State, b: State, direction: Direction, cutout_leaves: bool) -> bool {
        let vertical = matches!(direction, Direction::Down | Direction::Up);
        let bit = 1 << direction as u8;
        let opposite = 1 << direction.opposite() as u8;
        match a.definition.skip {
            SkipRendering::Never => false,
            SkipRendering::SameBlock => a.block == b.block,
            SkipRendering::Leaves => !cutout_leaves && b.definition.skip == SkipRendering::Leaves,
            SkipRendering::Liquid => a.fluid.same_family(b.fluid),
            SkipRendering::MangroveRoots => a.block == b.block && vertical,
            SkipRendering::Bars => {
                (a.block == b.block
                    || (a.definition.bars_tag
                        && b.definition.bars_tag
                        && b.connection_properties & opposite != 0))
                    && (vertical || (a.connections & bit != 0 && b.connections & opposite != 0))
            }
        }
    }
    /// The state's occlusion face toward `direction` (`getFaceOcclusionShape`) and
    /// its shared ID: 0 is the empty singleton, 1 the full-block singleton.
    pub fn occlusion_face(
        &self,
        state: BlockStateId,
        direction: Direction,
    ) -> Option<(u32, &VoxelShape)> {
        let set = self.states.get(state.0 as usize)?.face_set;
        let id = self.face_sets[set as usize][direction as usize];
        Some((id, &self.faces[id as usize]))
    }
    pub fn skip_rendering(
        &self,
        state: BlockStateId,
        neighbor: BlockStateId,
        direction: Direction,
        cutout_leaves: bool,
    ) -> io::Result<bool> {
        Ok(Self::skips(
            self.state(state)?,
            self.state(neighbor)?,
            direction,
            cutout_leaves,
        ))
    }
    pub fn should_render_face(
        &mut self,
        state: BlockStateId,
        neighbor: BlockStateId,
        direction: Direction,
        cutout_leaves: bool,
    ) -> io::Result<bool> {
        let a = self.state(state)?;
        let b = self.state(neighbor)?;
        let neighbor_face = self.face_sets[b.face_set as usize][direction.opposite() as usize];
        if neighbor_face == 1 || Self::skips(a, b, direction, cutout_leaves) {
            return Ok(false);
        }
        let own_face = self.face_sets[a.face_set as usize][direction as usize];
        if neighbor_face == 0 || own_face == 0 {
            return Ok(true);
        }
        let key = (own_face, neighbor_face);
        if let Some(value) = self.cache.get(&key) {
            return Ok(*value);
        }
        let visible = self.faces[own_face as usize]
            .join_is_not_empty(&self.faces[neighbor_face as usize], ShapeOp::ONLY_FIRST)
            .map_err(|e| invalid(e.to_string()))?;
        // Cache policy affects speed only. Keep bounded memory while retaining
        // more shared face pairs than the native 256-entry per-thread cache.
        if self.cache.len() == 4096 {
            self.cache.clear();
        }
        self.cache.insert(key, visible);
        Ok(visible)
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    #[test]
    fn noncanonical_face_grids_share_allocations_and_keep_their_identity() {
        let mut pool = FacePool::new();
        let a = VoxelShape::from_cube([2, 1, 1], &[true, false]).unwrap();
        let b = VoxelShape::from_cube([2, 1, 1], &[true, false]).unwrap();
        let c = VoxelShape::from_cube([2, 1, 1], &[false, true]).unwrap();
        assert!(!std::ptr::eq(a.boxes(), b.boxes()));
        let original = [a, b, c];
        for (i, face) in original.iter().enumerate() {
            assert_eq!(pool.push(face.clone()), (i + 2) as u32);
        }
        let pooled = &pool.faces[2..];
        assert!(std::ptr::eq(pooled[0].boxes(), pooled[1].boxes()));
        assert!(!std::ptr::eq(pooled[0].boxes(), pooled[2].boxes()));
        for (i, a) in pooled.iter().enumerate() {
            assert!(a.same_identity(&original[i]));
            for (j, b) in pooled.iter().enumerate() {
                assert_eq!(a.same_identity(b), original[i].same_identity(&original[j]));
                assert_eq!(
                    a.join_is_not_empty(b, ShapeOp::ONLY_FIRST),
                    original[i].join_is_not_empty(&original[j], ShapeOp::ONLY_FIRST),
                );
            }
        }
    }

    #[test]
    fn clones_share_geometry_but_keep_independent_pair_caches() {
        let a = FaceVisibility {
            states: Vec::new().into(),
            face_sets: Arc::new([]),
            faces: vec![VoxelShape::empty(), VoxelShape::block()].into(),
            cache: HashMap::from([((0, 1), false)]),
        };
        let mut b = a.clone();
        assert!(Arc::ptr_eq(&a.states, &b.states));
        assert!(Arc::ptr_eq(&a.faces, &b.faces));
        assert!(Arc::ptr_eq(&a.face_sets, &b.face_sets));
        b.cache.insert((1, 0), true);
        assert!(!a.cache.contains_key(&(1, 0)));
    }
}
