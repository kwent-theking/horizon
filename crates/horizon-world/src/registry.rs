//! Runtime block-state IDs imported from a version-pinned registry data export.
//! This supplies identities and default properties, not block behavior.
mod facts;
mod json;
#[cfg(test)]
mod snapshot_tests;
mod table_cache;
use crate::{DATA_VERSION, chunk::BlockState, compression::invalid, nbt::Tag};
pub use json::BlockStateJsonCodec;
pub(crate) use json::java_integer;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    io,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockStateId(pub u32);

// Only ordered descriptors and defaults belong on disk. Runtime hash indexes
// are rebuilt and validated, never trusted from a snapshot.
impl serde::Serialize for BlockRegistry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let defaults: BTreeMap<&str, i64> = self
            .defaults
            .iter()
            .map(|(name, id)| (name.as_str(), i64::from(id.0)))
            .collect();
        serde::Serialize::serialize(&(&self.states, defaults), serializer)
    }
}
impl<'de> serde::Deserialize<'de> for BlockRegistry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (states, defaults) = serde::Deserialize::deserialize(deserializer)?;
        Self::from_ordered_states(states, defaults).map_err(serde::de::Error::custom)
    }
}

pub struct BlockRegistry {
    states: Vec<BlockState>,
    /// Ids by a hash of their state, checked against `states`: 12 bytes per
    /// state instead of a second copy of every name and property map (about
    /// 125 MB for a registry of 190,000 states).
    ids: HashMap<u64, u32>,
    /// States whose hash an earlier state already took (none in vanilla).
    collided: BTreeMap<BlockState, BlockStateId>,
    defaults: BTreeMap<String, BlockStateId>,
    cache_identity: std::sync::OnceLock<[u8; 32]>,
}
/// The hash [`BlockRegistry`] indexes states by (fixed keys: the same in
/// every process).
fn state_key(state: &BlockState) -> u64 {
    let mut hasher = DefaultHasher::new();
    state.name.hash(&mut hasher);
    for (key, value) in &state.properties {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    hasher.finish()
}
impl BlockRegistry {
    pub fn from_nbt(tag: &Tag) -> io::Result<Self> {
        if tag.get("DataVersion") != Some(&Tag::Int(DATA_VERSION)) {
            return Err(invalid("block registry DataVersion mismatch"));
        }
        let Some(Tag::List(entries)) = tag.get("states") else {
            return Err(invalid("missing registry states"));
        };
        let Some(Tag::Compound(defaults)) = tag.get("defaults") else {
            return Err(invalid("missing registry defaults"));
        };
        let states = entries
            .iter()
            .map(BlockState::from_nbt)
            .collect::<io::Result<_>>()?;
        let defaults = defaults
            .iter()
            .map(|(name, id)| {
                let name = name.to_string().map_err(|e| invalid(e.to_string()))?;
                let Tag::Int(id) = id else {
                    return Err(invalid("default state ID is not an int"));
                };
                Ok((name, i64::from(*id)))
            })
            .collect::<io::Result<_>>()?;
        Self::from_ordered_states(states, defaults)
    }

    /// The registry of a resources `facts.json` (`{"states": [[..., ..., ...,
    /// ..., state], ...], "defaults": {name: id}}`), read as it streams: no
    /// JSON or NBT tree of the whole file is built. For a pack of 190,000
    /// states the two trees were several hundred megabytes at startup.
    pub fn from_facts_json(reader: impl io::Read) -> io::Result<Self> {
        Ok(Self::from_facts_json_with_flags(reader)?.0)
    }

    /// [`Self::from_facts_json`], with each state's `(air, fluid)` columns.
    pub fn from_facts_json_with_flags(
        reader: impl io::Read,
    ) -> io::Result<(Self, Vec<(bool, bool)>)> {
        let facts: facts::Facts = serde_json::from_reader(io::BufReader::new(reader))
            .map_err(|e| invalid(e.to_string()))?;
        let flags = facts.states.iter().map(|r| (r.air, r.fluid)).collect();
        let states = facts.states.into_iter().map(|r| r.state).collect();
        Ok((Self::from_ordered_states(states, facts.defaults)?, flags))
    }

    /// Rebuild the runtime indexes from ordered registry descriptors. The
    /// descriptor order defines every `BlockStateId` and must be preserved.
    pub fn from_ordered_states(
        mut states: Vec<BlockState>,
        defaults: BTreeMap<String, i64>,
    ) -> io::Result<Self> {
        if states.is_empty() || states.len() > i32::MAX as usize {
            return Err(invalid("invalid registry size"));
        }
        let mut ids: HashMap<u64, u32> = HashMap::with_capacity(states.len());
        let mut collided: BTreeMap<BlockState, BlockStateId> = BTreeMap::new();
        for (i, state) in states.iter().enumerate() {
            let key = state_key(state);
            match ids.entry(key) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(i as u32);
                }
                std::collections::hash_map::Entry::Occupied(slot) => {
                    if states[*slot.get() as usize] == *state || collided.contains_key(state) {
                        return Err(invalid("duplicate block registry state"));
                    }
                    collided.insert(state.clone(), BlockStateId(i as u32));
                }
            }
        }
        states.shrink_to_fit();
        let mut result = Self {
            states,
            ids,
            collided,
            defaults: BTreeMap::new(),
            cache_identity: std::sync::OnceLock::new(),
        };
        for (name, id) in defaults {
            if id < 0
                || id > i64::from(i32::MAX)
                || result
                    .states
                    .get(id as usize)
                    .is_none_or(|s| s.name != name)
            {
                return Err(invalid("invalid default block state ID"));
            }
            result.defaults.insert(name, BlockStateId(id as u32));
        }
        let names: BTreeSet<_> = result.states.iter().map(|s| &s.name).collect();
        if names.len() != result.defaults.len()
            || names
                .iter()
                .any(|name| !result.defaults.contains_key(*name))
        {
            return Err(invalid("registry is missing block defaults"));
        }
        Ok(result)
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
    pub fn state(&self, id: BlockStateId) -> Option<&BlockState> {
        self.states.get(id.0 as usize)
    }
    pub fn id(&self, state: &BlockState) -> Option<BlockStateId> {
        match self.ids.get(&state_key(state)) {
            Some(&id) if self.states[id as usize] == *state => Some(BlockStateId(id)),
            _ => self.collided.get(state).copied(),
        }
    }
    pub fn default_state(&self, name: &str) -> Option<BlockStateId> {
        self.defaults.get(name).copied()
    }
    /// Stable digest of the complete state identity list, computed once for
    /// derived resource caches. Defaults are deliberately not part of it.
    pub fn cache_identity(&self) -> &[u8; 32] {
        table_cache::identity(self)
    }

    /// The values `property` takes on `block`, in `Property.getPossibleValues`
    /// order. The state list is in `StateDefinition` order (properties sorted
    /// by name, values in their own order), so a value's first appearance is
    /// that order.
    pub fn property_values(&self, block: &str, property: &str) -> Vec<&str> {
        let mut values: Vec<&str> = Vec::new();
        for state in self
            .states
            .iter()
            .filter(|state| state.name.as_str() == block)
        {
            if let Some(value) = state.properties.get(property) {
                let value = value.as_str();
                if !values.contains(&value) {
                    values.push(value);
                }
            }
        }
        values
    }

    /// Resolve a saved descriptor, supplying omitted default properties. Unlike
    /// vanilla's lenient logging paths, invalid properties return a typed error.
    pub fn resolve(&self, descriptor: &BlockState) -> io::Result<BlockStateId> {
        let default = self
            .default_state(&descriptor.name)
            .ok_or_else(|| invalid("unknown block name"))?;
        let mut state = self.state(default).unwrap().clone();
        for (key, value) in &descriptor.properties {
            if !state.properties.contains_key(key) {
                return Err(invalid("unknown block property"));
            }
            state.properties.insert(key.clone(), value.clone());
        }
        self.id(&state)
            .ok_or_else(|| invalid("invalid block property value"))
    }
}

/// A per-state table (`occlusion.nbt`, `cached-collision.nbt`, ...) whose
/// `states` list, the identity of every state it describes in id order, has
/// been checked against `registry`.
#[derive(Clone, Copy)]
pub struct BlockTable<'t, 'r> {
    tag: &'t Tag,
    registry: &'r BlockRegistry,
    /// How many levels, from this one down, [`BlockTableFile::read`] checked
    /// while reading: their `states` lists are kept empty in the tree.
    read_checked: u8,
}
impl<'t, 'r> BlockTable<'t, 'r> {
    /// Checks that `tag`'s `states` names the registry's states in id order.
    pub fn check(tag: &'t Tag, registry: &'r BlockRegistry, what: &str) -> io::Result<Self> {
        Self {
            tag,
            registry,
            read_checked: 0,
        }
        .checked(what)
    }
    fn checked(self, what: &str) -> io::Result<Self> {
        let Some(Tag::List(states)) = self.tag.get("states") else {
            return Err(invalid(format!("missing {what} state identities")));
        };
        if self.read_checked == 0 {
            if states.len() != self.registry.len() {
                return Err(invalid(format!("{what} registry size mismatch")));
            }
            for (id, state) in states.iter().enumerate() {
                if self.registry.state(BlockStateId(id as u32))
                    != Some(&BlockState::from_nbt(state)?)
                {
                    return Err(invalid(format!("{what} state identities differ")));
                }
            }
        }
        Ok(self)
    }
    /// The table nested under `key` (`block-properties.nbt`'s `occlusion`),
    /// with its own `states`, checked the same way.
    pub fn nested(&self, key: &str, what: &str) -> io::Result<Self> {
        Self {
            tag: self
                .tag
                .get(key)
                .ok_or_else(|| invalid(format!("missing {what}")))?,
            registry: self.registry,
            read_checked: self.read_checked.saturating_sub(1),
        }
        .checked(what)
    }
    pub fn tag(&self) -> &'t Tag {
        self.tag
    }
    pub fn get(&self, key: &str) -> Option<&'t Tag> {
        self.tag.get(key)
    }
    pub fn registry(&self) -> &'r BlockRegistry {
        self.registry
    }
}

/// A per-state table read from its file with its state identities checked
/// while reading, so they never exist as a tree: for 190,000 states (a big
/// modpack) that tree took 300-600 MB per table.
pub struct BlockTableFile<'r> {
    tag: Tag,
    registry: &'r BlockRegistry,
}
impl<'r> BlockTableFile<'r> {
    /// [`Self::read`] of the file at `path`.
    pub fn open(path: &std::path::Path, registry: &'r BlockRegistry) -> io::Result<Self> {
        let cache = table_cache::cache(path, registry);
        if let Some(tag) = cache.as_ref().and_then(table_cache::read) {
            return Ok(Self { tag, registry });
        }
        let bytes = std::fs::read(path)?;
        let file = Self::read(&bytes, registry)
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        drop(bytes);
        if let Some(cache) = cache {
            table_cache::write(&cache, &file.tag);
        }
        Ok(file)
    }
    /// Reads `bytes`, checking every `states` list of the root compound and
    /// of the compounds directly under it against `registry`.
    pub fn read(bytes: &[u8], registry: &'r BlockRegistry) -> io::Result<Self> {
        let mut visit = |states: &mut crate::nbt::ListElements<'_, '_, '_>| {
            if states.len() != registry.len() {
                return Err(states.error("table registry size mismatch"));
            }
            let mut id = 0u32;
            while let Some(tag) = states.next_element()? {
                let state =
                    BlockState::from_nbt(&tag).map_err(|_| states.error("invalid table state"))?;
                if registry.state(BlockStateId(id)) != Some(&state) {
                    return Err(states.error("table state identities differ"));
                }
                id += 1;
            }
            Ok(())
        };
        let (named, _) = crate::nbt::read_named_streaming(
            bytes,
            crate::nbt::Limits::tables(),
            "states",
            &mut visit,
        )
        .map_err(|e| invalid(e.to_string()))?;
        Ok(Self {
            tag: named.tag,
            registry,
        })
    }
    /// The table, checked (`what` names it in errors).
    pub fn table(&self, what: &str) -> io::Result<BlockTable<'_, 'r>> {
        BlockTable {
            tag: &self.tag,
            registry: self.registry,
            read_checked: 2,
        }
        .checked(what)
    }
}
