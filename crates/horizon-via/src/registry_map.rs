//! Numeric id <-> string-name mapping for Minecraft registries.
//!
//! This module is the *mechanism*, not the *data*: the full block/item/
//! enchant tables for every protocol version are megabytes of generated
//! content and live outside the crate (orchestrator loads them from the
//! prepared Horizon `.local` resources). What lives here is the lookup
//! structure those tables are poured into, and the translation primitive
//! every chunk/inventory rewriter calls: given id `47` at 1.12, what is it
//! at 26.3? Ã¢â‚¬â€ resolve the *name* at 1.12 (`minecraft:stone`), then the id
//! of that name at 26.3. Names are the stable pivot; ids are not.
//!
//! RU: Ã‘ÂÃ‘â€šÃÂ¾Ã‘â€š ÃÂ¼ÃÂ¾ÃÂ´Ã‘Æ’ÃÂ»Ã‘Å’ Ã¢â‚¬â€ ÃÂ¼ÃÂµÃ‘â€¦ÃÂ°ÃÂ½ÃÂ¸ÃÂ·ÃÂ¼, ÃÂ° ÃÂ½ÃÂµ ÃÂ´ÃÂ°ÃÂ½ÃÂ½Ã‘â€¹ÃÂµ. ÃÂ¢ÃÂ°ÃÂ±ÃÂ»ÃÂ¸Ã‘â€ Ã‘â€¹ ÃÂ¾ÃÂ³Ã‘â‚¬ÃÂ¾ÃÂ¼ÃÂ½Ã‘â€¹ ÃÂ¸ ÃÂ¿ÃÂ¾ÃÂ´ÃÂºÃÂ»Ã‘Å½Ã‘â€¡ÃÂ°Ã‘Å½Ã‘â€šÃ‘ÂÃ‘Â
//! ÃÂ¾Ã‘â‚¬ÃÂºÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ°Ã‘â€šÃÂ¾Ã‘â‚¬ÃÂ¾ÃÂ¼ ÃÂ¸ÃÂ·ÃÂ²ÃÂ½ÃÂµ; ÃÂ·ÃÂ´ÃÂµÃ‘ÂÃ‘Å’ Ã¢â‚¬â€ Ã‘ÂÃ‘â€šÃ‘â‚¬Ã‘Æ’ÃÂºÃ‘â€šÃ‘Æ’Ã‘â‚¬ÃÂ° ÃÂ´ÃÂ»Ã‘Â ÃÂ¸Ã‘â€¦ ÃÂ·ÃÂ°ÃÂ³Ã‘â‚¬Ã‘Æ’ÃÂ·ÃÂºÃÂ¸ ÃÂ¸ ÃÂ¿Ã‘â‚¬ÃÂ¸ÃÂ¼ÃÂ¸Ã‘â€šÃÂ¸ÃÂ²
//! Ã‘â€šÃ‘â‚¬ÃÂ°ÃÂ½Ã‘ÂÃÂ»Ã‘ÂÃ‘â€ ÃÂ¸ÃÂ¸ Ã‚Â«id -> ÃÂ¸ÃÂ¼Ã‘Â -> idÃ‚Â», ÃÂ½ÃÂ° ÃÂºÃÂ¾Ã‘â€šÃÂ¾Ã‘â‚¬ÃÂ¾ÃÂ¼ ÃÂ´ÃÂµÃ‘â‚¬ÃÂ¶ÃÂ°Ã‘â€šÃ‘ÂÃ‘Â ÃÂ¿ÃÂµÃ‘â‚¬ÃÂµÃÂ¿ÃÂ¸Ã‘ÂÃ‘â€¹ÃÂ²ÃÂ°Ã‘â€šÃÂµÃÂ»ÃÂ¸ Ã‘â€¡ÃÂ°ÃÂ½ÃÂºÃÂ¾ÃÂ²
//! ÃÂ¸ ÃÂ¸ÃÂ½ÃÂ²ÃÂµÃÂ½Ã‘â€šÃÂ°Ã‘â‚¬ÃÂµÃÂ¹. ÃÂ¡Ã‘â€šÃÂ°ÃÂ±ÃÂ¸ÃÂ»Ã‘Å’ÃÂ½ÃÂ°Ã‘Â ÃÂ¾Ã‘ÂÃ‘Å’ Ã¢â‚¬â€ ÃÂ¸ÃÂ¼ÃÂµÃÂ½ÃÂ°, ÃÂ° ÃÂ½ÃÂµ Ã‘â€¡ÃÂ¸Ã‘ÂÃÂ»ÃÂ°.
//!
//! # Flattening boundary (NOT this module's problem)
//!
//! Before 1.13 (the "flattening"), chunk block storage carried
//! `id + meta` (12 bits of block id + 4 bits of metadata). From 1.13 on,
//! chunks carry flat numeric ids of *block states*. Combining or splitting
//! id/meta into a state id before (resp. after) calling into this module is
//! the responsibility of the chunk translator, not of the registry map:
//! here an id is already whatever a single `u32` means in that version's
//! registry. A pre-1.13 pipeline therefore rewrites `id|meta -> state id`
//! first, then remaps the state id.
//!
//! RU: ÃÂ´ÃÂ¾ 1.13 Ã‘â€¡ÃÂ°ÃÂ½ÃÂºÃÂ¸ ÃÂ½ÃÂµÃ‘ÂÃÂ»ÃÂ¸ id+meta (12+4 ÃÂ±ÃÂ¸Ã‘â€šÃÂ°), ÃÂ¿ÃÂ¾Ã‘ÂÃÂ»ÃÂµ Ã¢â‚¬â€ ÃÂ¿ÃÂ»ÃÂ¾Ã‘ÂÃÂºÃÂ¸ÃÂ¹ id
//! ÃÂ±ÃÂ»ÃÂ¾ÃÂº-Ã‘ÂÃ‘â€šÃÂµÃÂ¹Ã‘â€šÃÂ°. ÃÂ¡ÃÂºÃÂ»ÃÂµÃÂ¹ÃÂºÃÂ°/Ã‘â‚¬ÃÂ°ÃÂ·ÃÂ±ÃÂ¾Ã‘â‚¬ id+meta Ã¢â‚¬â€ ÃÂ·ÃÂ°ÃÂ´ÃÂ°Ã‘â€¡ÃÂ° Ã‘â€¡ÃÂ°ÃÂ½ÃÂº-Ã‘â€šÃ‘â‚¬ÃÂ°ÃÂ½Ã‘ÂÃÂ»Ã‘ÂÃ‘â€šÃÂ¾Ã‘â‚¬ÃÂ°,
//! ÃÂÃâ€¢ Ã‘ÂÃ‘â€šÃÂ¾ÃÂ³ÃÂ¾ ÃÂ¼ÃÂ¾ÃÂ´Ã‘Æ’ÃÂ»Ã‘Â: ÃÂ·ÃÂ´ÃÂµÃ‘ÂÃ‘Å’ id Ã¢â‚¬â€ Ã‘ÂÃ‘â€šÃÂ¾ Ã‘â€šÃÂ¾, Ã‘â€¡Ã‘â€šÃÂ¾ u32 ÃÂ·ÃÂ½ÃÂ°Ã‘â€¡ÃÂ¸Ã‘â€š ÃÂ² Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂµ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸.

use crate::errors::ViaError;
use std::collections::HashMap;

/// Which registry a mapping belongs to. Registries are kept in one shared
/// [`IdMap`] per version, keyed by this enum, so a single `HashMap` pair
/// serves blocks, items and everything else without name collisions
/// between namespaces.
///
/// RU: ÃÂ²ÃÂ¸ÃÂ´ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ°. Ãâ€™Ã‘ÂÃÂµ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬Ã‘â€¹ ÃÂ¾ÃÂ´ÃÂ½ÃÂ¾ÃÂ¹ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸ ÃÂ¶ÃÂ¸ÃÂ²Ã‘Æ’Ã‘â€š ÃÂ² ÃÂ¾ÃÂ´ÃÂ½ÃÂ¾ÃÂ¼ [`IdMap`],
/// ÃÂºÃÂ»Ã‘Å½Ã‘â€¡ Ã¢â‚¬â€ Ã‘ÂÃ‘â€šÃÂ¾Ã‘â€š enum, Ã‘â€¡Ã‘â€šÃÂ¾ÃÂ±Ã‘â€¹ ÃÂ¸ÃÂ¼ÃÂµÃÂ½ÃÂ° Ã‘â‚¬ÃÂ°ÃÂ·ÃÂ½Ã‘â€¹Ã‘â€¦ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ¾ÃÂ² ÃÂ½ÃÂµ ÃÂºÃÂ¾ÃÂ½Ã‘â€žÃÂ»ÃÂ¸ÃÂºÃ‘â€šÃÂ¾ÃÂ²ÃÂ°ÃÂ»ÃÂ¸.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum RegistryKind {
    /// Blocks / block states (chunk data, world updates).
    Block,
    /// Items (inventory, windows, item components).
    Item,
    /// Enchantments (item NBT, enchant packets).
    Enchantment,
    /// Entity types (spawn packets, metadata).
    Entity,
    /// Particle types.
    Particle,
    /// Sound events (sound effect packets).
    Sound,
}

impl RegistryKind {
    /// Canonical wire-ish key for this registry, used in error messages and
    /// when loading data files (`"minecraft:block"`, `"minecraft:item"`...).
    ///
    /// RU: ÃÂºÃÂ°ÃÂ½ÃÂ¾ÃÂ½ÃÂ¸Ã‘â€¡ÃÂµÃ‘ÂÃÂºÃÂ¸ÃÂ¹ ÃÂºÃÂ»Ã‘Å½Ã‘â€¡ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ° ÃÂ´ÃÂ»Ã‘Â ÃÂ¾Ã‘Ë†ÃÂ¸ÃÂ±ÃÂ¾ÃÂº ÃÂ¸ ÃÂ·ÃÂ°ÃÂ³Ã‘â‚¬Ã‘Æ’ÃÂ·ÃÂºÃÂ¸ ÃÂ´ÃÂ°ÃÂ½ÃÂ½Ã‘â€¹Ã‘â€¦.
    pub fn key(self) -> &'static str {
        match self {
            RegistryKind::Block => "minecraft:block",
            RegistryKind::Item => "minecraft:item",
            RegistryKind::Enchantment => "minecraft:enchantment",
            RegistryKind::Entity => "minecraft:entity_type",
            RegistryKind::Particle => "minecraft:particle_type",
            RegistryKind::Sound => "minecraft:sound_event",
        }
    }
}

/// Bidirectional id <-> name map for ONE protocol version, covering all
/// registry kinds (see [`RegistryKind`]).
///
/// The two `HashMap`s are kept in sync by [`IdMap::insert`]: forward for
/// id -> name (decoding packets from a server), backward for name -> id
/// (encoding packets to a server, and the actual pivot of version
/// translation in [`remap`]).
///
/// Duplicate `name` -> a later `insert` wins in the backward map (same
/// behaviour as registry data files that re-define an entry); duplicate
/// `id` overwrites forward. Entries are never removed: registry tables are
/// append-per-version snapshots.
///
/// RU: ÃÂ´ÃÂ²Ã‘Æ’Ã‘ÂÃ‘â€šÃÂ¾Ã‘â‚¬ÃÂ¾ÃÂ½ÃÂ½Ã‘ÂÃ‘Â ÃÂºÃÂ°Ã‘â‚¬Ã‘â€šÃÂ° id <-> ÃÂ¸ÃÂ¼Ã‘Â ÃÂ´ÃÂ»Ã‘Â ÃÅ¾Ãâ€ÃÂÃÅ¾Ãâ„¢ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸ ÃÂ¿Ã‘â‚¬ÃÂ¾Ã‘â€šÃÂ¾ÃÂºÃÂ¾ÃÂ»ÃÂ°.
/// ÃÅ¸Ã‘â‚¬Ã‘ÂÃÂ¼ÃÂ°Ã‘Â ÃÂºÃÂ°Ã‘â‚¬Ã‘â€šÃÂ° Ã¢â‚¬â€ ÃÂ´ÃÂµÃÂºÃÂ¾ÃÂ´ÃÂ¸Ã‘â‚¬ÃÂ¾ÃÂ²ÃÂ°ÃÂ½ÃÂ¸ÃÂµ ÃÂ¿ÃÂ°ÃÂºÃÂµÃ‘â€šÃÂ¾ÃÂ², ÃÂ¾ÃÂ±Ã‘â‚¬ÃÂ°Ã‘â€šÃÂ½ÃÂ°Ã‘Â Ã¢â‚¬â€ ÃÂºÃÂ¾ÃÂ´ÃÂ¸Ã‘â‚¬ÃÂ¾ÃÂ²ÃÂ°ÃÂ½ÃÂ¸ÃÂµ ÃÂ¸
/// Ã‘â€šÃ‘â‚¬ÃÂ°ÃÂ½Ã‘ÂÃÂ»Ã‘ÂÃ‘â€ ÃÂ¸Ã‘Â ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¹ Ã‘â€¡ÃÂµÃ‘â‚¬ÃÂµÃÂ· [`remap`].
#[derive(Debug, Default, Clone)]
pub struct IdMap {
    forward: HashMap<(RegistryKind, u32), String>,
    backward: HashMap<(RegistryKind, String), u32>,
}

impl IdMap {
    /// Empty map; the data loader fills it with [`IdMap::insert`].
    ///
    /// RU: ÃÂ¿Ã‘Æ’Ã‘ÂÃ‘â€šÃÂ°Ã‘Â ÃÂºÃÂ°Ã‘â‚¬Ã‘â€šÃÂ°, ÃÂ´ÃÂ°ÃÂ½ÃÂ½Ã‘â€¹ÃÂµ ÃÂ·ÃÂ°ÃÂ»ÃÂ¸ÃÂ²ÃÂ°ÃÂµÃ‘â€š ÃÂ·ÃÂ°ÃÂ³Ã‘â‚¬Ã‘Æ’ÃÂ·Ã‘â€¡ÃÂ¸ÃÂº.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `name` as the id `id` for registry `kind`.
    ///
    /// Both directions are updated; a re-insert of the same pair is a
    /// no-op. See the struct docs for overwrite semantics on duplicates.
    ///
    /// RU: ÃÂ·ÃÂ°ÃÂ¿ÃÂ¸Ã‘ÂÃÂ°Ã‘â€šÃ‘Å’ ÃÂ¿ÃÂ°Ã‘â‚¬Ã‘Æ’ Ã‚Â«id, ÃÂ¸ÃÂ¼Ã‘ÂÃ‚Â» ÃÂ² ÃÂ¾ÃÂ±ÃÂµ Ã‘ÂÃ‘â€šÃÂ¾Ã‘â‚¬ÃÂ¾ÃÂ½Ã‘â€¹.
    pub fn insert(&mut self, kind: RegistryKind, id: u32, name: impl Into<String>) {
        let name = name.into();
        self.backward.insert((kind, name.clone()), id);
        self.forward.insert((kind, id), name);
    }

    /// Name for `id` in registry `kind` at this version, if known.
    ///
    /// RU: ÃÂ¸ÃÂ¼Ã‘Â ÃÂ¿ÃÂ¾ id ÃÂ² Ã‘ÂÃ‘â€šÃÂ¾ÃÂ¹ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸.
    pub fn name_of(&self, kind: RegistryKind, id: u32) -> Option<&str> {
        self.forward.get(&(kind, id)).map(String::as_str)
    }

    /// Id for `name` in registry `kind` at this version, if known.
    ///
    /// RU: id ÃÂ¿ÃÂ¾ ÃÂ¸ÃÂ¼ÃÂµÃÂ½ÃÂ¸ ÃÂ² Ã‘ÂÃ‘â€šÃÂ¾ÃÂ¹ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸.
    pub fn id_of(&self, kind: RegistryKind, name: &str) -> Option<u32> {
        self.backward.get(&(kind, name.to_string())).copied()
    }

    /// Number of recorded entries for registry `kind` (forward map size;
    /// equal to backward size unless duplicate names were inserted).
    ///
    /// RU: ÃÂºÃÂ¾ÃÂ»ÃÂ¸Ã‘â€¡ÃÂµÃ‘ÂÃ‘â€šÃÂ²ÃÂ¾ ÃÂ·ÃÂ°ÃÂ¿ÃÂ¸Ã‘ÂÃÂµÃÂ¹ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ° ÃÂ² Ã‘ÂÃ‘â€šÃÂ¾ÃÂ¹ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸.
    pub fn len(&self, kind: RegistryKind) -> usize {
        self.forward.keys().filter(|(k, _)| *k == kind).count()
    }

    /// True when registry `kind` has no entries in this map.
    ///
    /// RU: Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ ÃÂ¿Ã‘Æ’Ã‘ÂÃ‘â€š.
    pub fn is_empty(&self, kind: RegistryKind) -> bool {
        self.len(kind) == 0
    }
}

/// A chain of per-version [`IdMap`]s, ordered oldest -> newest.
///
/// Index 0 is the oldest version in the chain (e.g. 1.8), the last index is
/// the client's version (26.3). The *indices* are opaque to this struct:
/// the pipeline owns the version -> index correspondence.
///
/// RU: Ã‘â€ ÃÂµÃÂ¿ÃÂ¾Ã‘â€¡ÃÂºÃÂ° ÃÂºÃÂ°Ã‘â‚¬Ã‘â€š ÃÂ¿ÃÂ¾ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸Ã‘ÂÃÂ¼, ÃÂ¾Ã‘â€š Ã‘ÂÃ‘â€šÃÂ°Ã‘â‚¬ÃÂ¾ÃÂ¹ ÃÂº ÃÂ½ÃÂ¾ÃÂ²ÃÂ¾ÃÂ¹. ÃÂ¡ÃÂ¾ÃÂ¾Ã‘â€šÃÂ²ÃÂµÃ‘â€šÃ‘ÂÃ‘â€šÃÂ²ÃÂ¸ÃÂµ
/// Ã‚Â«ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸Ã‘Â ÃÂ¿Ã‘â‚¬ÃÂ¾Ã‘â€šÃÂ¾ÃÂºÃÂ¾ÃÂ»ÃÂ° -> ÃÂ¸ÃÂ½ÃÂ´ÃÂµÃÂºÃ‘ÂÃ‚Â» ÃÂ´ÃÂµÃ‘â‚¬ÃÂ¶ÃÂ¸Ã‘â€š ÃÂ¿ÃÂ°ÃÂ¹ÃÂ¿ÃÂ»ÃÂ°ÃÂ¹ÃÂ½, ÃÂ½ÃÂµ Ã‘ÂÃ‘â€šÃÂ° Ã‘ÂÃ‘â€šÃ‘â‚¬Ã‘Æ’ÃÂºÃ‘â€šÃ‘Æ’Ã‘â‚¬ÃÂ°.
#[derive(Debug, Default, Clone)]
pub struct RegistrySet {
    maps: Vec<IdMap>,
}

impl RegistrySet {
    /// A set holding exactly the given maps, in order.
    ///
    /// RU: ÃÂ½ÃÂ°ÃÂ±ÃÂ¾Ã‘â‚¬ ÃÂ¸ÃÂ· ÃÂºÃÂ°Ã‘â‚¬Ã‘â€š ÃÂ² ÃÂ¿ÃÂ¾Ã‘â‚¬Ã‘ÂÃÂ´ÃÂºÃÂµ Ã‘ÂÃÂ»ÃÂµÃÂ´ÃÂ¾ÃÂ²ÃÂ°ÃÂ½ÃÂ¸Ã‘Â.
    pub fn from_maps(maps: Vec<IdMap>) -> Self {
        Self { maps }
    }

    /// Push a newer version's map onto the end of the chain.
    ///
    /// RU: ÃÂ´ÃÂ¾ÃÂ±ÃÂ°ÃÂ²ÃÂ¸Ã‘â€šÃ‘Å’ ÃÂºÃÂ°Ã‘â‚¬Ã‘â€šÃ‘Æ’ ÃÂ±ÃÂ¾ÃÂ»ÃÂµÃÂµ ÃÂ½ÃÂ¾ÃÂ²ÃÂ¾ÃÂ¹ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸ ÃÂ² ÃÂºÃÂ¾ÃÂ½ÃÂµÃ‘â€  Ã‘â€ ÃÂµÃÂ¿ÃÂ¾Ã‘â€¡ÃÂºÃÂ¸.
    pub fn push(&mut self, map: IdMap) {
        self.maps.push(map);
    }

    /// Number of versions in the chain.
    ///
    /// RU: ÃÂºÃÂ¾ÃÂ»ÃÂ¸Ã‘â€¡ÃÂµÃ‘ÂÃ‘â€šÃÂ²ÃÂ¾ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¹ ÃÂ² Ã‘â€ ÃÂµÃÂ¿ÃÂ¾Ã‘â€¡ÃÂºÃÂµ.
    pub fn version_count(&self) -> usize {
        self.maps.len()
    }

    /// The map at `version_index`, or `None` if the index is out of range.
    /// Index 0 = oldest version.
    ///
    /// RU: ÃÂºÃÂ°Ã‘â‚¬Ã‘â€šÃÂ° ÃÂ¿ÃÂ¾ ÃÂ¸ÃÂ½ÃÂ´ÃÂµÃÂºÃ‘ÂÃ‘Æ’ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸ÃÂ¸ (0 Ã¢â‚¬â€ Ã‘ÂÃÂ°ÃÂ¼ÃÂ°Ã‘Â Ã‘ÂÃ‘â€šÃÂ°Ã‘â‚¬ÃÂ°Ã‘Â).
    pub fn at(&self, version_index: usize) -> Option<&IdMap> {
        self.maps.get(version_index)
    }
}

/// Translate `id` of registry `kind` between two versions by pivoting
/// through the entry *name*.
///
/// Returns the id in `to` for the name that `id` carries in `from`, or
/// `None` when the name is unknown on either side. `None` is a policy
/// decision point for the caller: map to a fallback entry (see
/// [`Remapper`]), drop the entry, or bubble an error up.
///
/// Note the flattening boundary (module docs): for pre-1.13 blocks the
/// caller must fold `id|meta` into a state id *before* calling this.
///
/// RU: ÃÂ¿ÃÂµÃ‘â‚¬ÃÂµÃÂ²ÃÂµÃ‘ÂÃ‘â€šÃÂ¸ id ÃÂ¼ÃÂµÃÂ¶ÃÂ´Ã‘Æ’ ÃÂ²ÃÂµÃ‘â‚¬Ã‘ÂÃÂ¸Ã‘ÂÃÂ¼ÃÂ¸ Ã‘â€¡ÃÂµÃ‘â‚¬ÃÂµÃÂ· ÃÂ¸ÃÂ¼Ã‘Â. None Ã¢â‚¬â€ Ã‘â€šÃÂ¾Ã‘â€¡ÃÂºÃÂ° Ã‘â‚¬ÃÂµÃ‘Ë†ÃÂµÃÂ½ÃÂ¸Ã‘Â
/// ÃÂ´ÃÂ»Ã‘Â ÃÂ²Ã‘â€¹ÃÂ·Ã‘â€¹ÃÂ²ÃÂ°Ã‘Å½Ã‘â€°ÃÂµÃÂ³ÃÂ¾: Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂº, ÃÂ´Ã‘â‚¬ÃÂ¾ÃÂ¿ ÃÂ¸ÃÂ»ÃÂ¸ ÃÂ¾Ã‘Ë†ÃÂ¸ÃÂ±ÃÂºÃÂ°.
pub fn remap(kind: RegistryKind, id: u32, from: &IdMap, to: &IdMap) -> Option<u32> {
    // Intermediate binding keeps the borrow of `from` tied to `name`,
    // so the later call on `to` (immutable too) has no overlap.
    let name = from.name_of(kind, id)?;
    to.id_of(kind, name)
}

/// Fallback policy for unmappable ids.
///
/// A missing entry is normal life on cross-version servers: pre-1.13
/// `id|meta` combinations that no post-flattening state matches, items
/// removed by an update, a plugin sending a custom id. The remapper's
/// job is to turn "unmappable" into "safe substitute or explicit error"
/// instead of a panic or a silent wire corruption.
///
/// * [`Remapper::with_defaults`]: blocks/items fall back to
///   `minecraft:stone` (invisible-ish, harmless, present in every version
///   since forever); enchantments, entities, particles and sounds have no
///   safe substitute, so no fallback Ã¢â‚¬â€ the caller drops the entry and
///   reports [`ViaError::MissingRegistry`].
/// * A custom fallback table is set with [`Remapper::set_fallback`].
///
/// RU: ÃÂ¿ÃÂ¾ÃÂ»ÃÂ¸Ã‘â€šÃÂ¸ÃÂºÃÂ° Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂºÃÂ¾ÃÂ². Ãâ€˜ÃÂ»ÃÂ¾ÃÂºÃÂ¸/ÃÂ¿Ã‘â‚¬ÃÂµÃÂ´ÃÂ¼ÃÂµÃ‘â€šÃ‘â€¹ ÃÂ·ÃÂ°ÃÂ¼ÃÂµÃÂ½Ã‘ÂÃ‘Å½Ã‘â€šÃ‘ÂÃ‘Â ÃÂ½ÃÂ° minecraft:stone;
/// ÃÂ´ÃÂ»Ã‘Â ÃÂ¾Ã‘ÂÃ‘â€šÃÂ°ÃÂ»Ã‘Å’ÃÂ½Ã‘â€¹Ã‘â€¦ Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ¾ÃÂ² ÃÂ±ÃÂµÃÂ·ÃÂ¾ÃÂ¿ÃÂ°Ã‘ÂÃÂ½ÃÂ¾ÃÂ¹ ÃÂ·ÃÂ°ÃÂ¼ÃÂµÃÂ½Ã‘â€¹ ÃÂ½ÃÂµÃ‘â€š Ã¢â‚¬â€ ÃÂ¾Ã‘Ë†ÃÂ¸ÃÂ±ÃÂºÃÂ° ÃÂ²Ã‘â€¹ÃÂ·Ã‘â€¹ÃÂ²ÃÂ°Ã‘Å½Ã‘â€°ÃÂµÃÂ¼Ã‘Æ’.
#[derive(Debug, Default, Clone)]
pub struct Remapper {
    fallbacks: HashMap<RegistryKind, String>,
}

impl Remapper {
    /// Default fallback table (see struct docs):
    ///
    /// | kind | fallback |
    /// |---|---|
    /// | Block | `minecraft:stone` |
    /// | Item | `minecraft:stone` |
    /// | Enchantment | none (drop) |
    /// | Entity | none (drop) |
    /// | Particle | none (drop) |
    /// | Sound | none (drop) |
    ///
    /// "None" means: [`Remapper::remap_or_fallback`] returns
    /// [`ViaError::MissingRegistry`] and the caller decides (normally:
    /// skip the enchant line / the particle effect, log once).
    ///
    /// RU: Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂºÃÂ¸ ÃÂ¿ÃÂ¾ Ã‘Æ’ÃÂ¼ÃÂ¾ÃÂ»Ã‘â€¡ÃÂ°ÃÂ½ÃÂ¸Ã‘Å½: stone ÃÂ´ÃÂ»Ã‘Â ÃÂ±ÃÂ»ÃÂ¾ÃÂºÃÂ¾ÃÂ² ÃÂ¸ ÃÂ¿Ã‘â‚¬ÃÂµÃÂ´ÃÂ¼ÃÂµÃ‘â€šÃÂ¾ÃÂ²,
    /// ÃÂ´ÃÂ»Ã‘Â ÃÂ¾Ã‘ÂÃ‘â€šÃÂ°ÃÂ»Ã‘Å’ÃÂ½Ã‘â€¹Ã‘â€¦ Ã¢â‚¬â€ ÃÂ´Ã‘â‚¬ÃÂ¾ÃÂ¿ Ã‘Â ÃÂ¾Ã‘Ë†ÃÂ¸ÃÂ±ÃÂºÃÂ¾ÃÂ¹.
    pub fn with_defaults() -> Self {
        let mut fallbacks = HashMap::new();
        fallbacks.insert(RegistryKind::Block, "minecraft:stone".to_string());
        fallbacks.insert(RegistryKind::Item, "minecraft:stone".to_string());
        Self { fallbacks }
    }

    /// Override (or remove, with `None`) the fallback name for `kind`.
    ///
    /// RU: ÃÂ·ÃÂ°ÃÂ´ÃÂ°Ã‘â€šÃ‘Å’ ÃÂ¸ÃÂ»ÃÂ¸ Ã‘Æ’ÃÂ±Ã‘â‚¬ÃÂ°Ã‘â€šÃ‘Å’ Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂº ÃÂ´ÃÂ»Ã‘Â Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ°.
    pub fn set_fallback(&mut self, kind: RegistryKind, name: Option<&str>) {
        match name {
            Some(n) => {
                self.fallbacks.insert(kind, n.to_string());
            }
            None => {
                self.fallbacks.remove(&kind);
            }
        }
    }

    /// The configured fallback name for `kind`, if any.
    ///
    /// RU: Ã‘â€šÃÂµÃÂºÃ‘Æ’Ã‘â€°ÃÂ¸ÃÂ¹ Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂº Ã‘â‚¬ÃÂµÃÂµÃ‘ÂÃ‘â€šÃ‘â‚¬ÃÂ°, ÃÂµÃ‘ÂÃÂ»ÃÂ¸ ÃÂ·ÃÂ°ÃÂ´ÃÂ°ÃÂ½.
    pub fn fallback_for(&self, kind: RegistryKind) -> Option<&str> {
        self.fallbacks.get(&kind).map(String::as_str)
    }

    /// [`remap`] with the fallback policy applied.
    ///
    /// 1. Try the name pivot; on success, done.
    /// 2. On `None`, look up the fallback name for `kind` *in `to`* (the
    ///    fallback must exist in the destination version Ã¢â‚¬â€ a fallback that
    ///    is itself unmappable is a data bug, not a runtime state).
    /// 3. No fallback (or fallback missing from `to`):
    ///    [`ViaError::MissingRegistry`] with the pivot context, e.g.
    ///    `"minecraft:enchantment/5"`.
    ///
    /// Non-fatal by design: the caller drops one entry and continues
    /// (see [`ViaError::is_fatal`]).
    ///
    /// RU: remap Ã‘Â Ã‘Æ’Ã‘â€¡Ã‘â€˜Ã‘â€šÃÂ¾ÃÂ¼ Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂºÃÂ¾ÃÂ²; ÃÂ¾Ã‘Ë†ÃÂ¸ÃÂ±ÃÂºÃÂ° MissingRegistry Ã¢â‚¬â€ Ã‘â€šÃÂ¾ÃÂ»Ã‘Å’ÃÂºÃÂ¾
    /// ÃÂºÃÂ¾ÃÂ³ÃÂ´ÃÂ° ÃÂ¸ ÃÂ¸ÃÂ¼ÃÂµÃÂ½ÃÂ¸, ÃÂ¸ Ã‘â€žÃÂ¾ÃÂ»ÃÂ±Ã‘ÂÃÂºÃÂ° ÃÂ½ÃÂµÃ‘â€š.
    pub fn remap_or_fallback(
        &self,
        kind: RegistryKind,
        id: u32,
        from: &IdMap,
        to: &IdMap,
    ) -> Result<u32, ViaError> {
        if let Some(mapped) = remap(kind, id, from, to) {
            return Ok(mapped);
        }
        let fallback = self
            .fallbacks
            .get(&kind)
            .ok_or_else(|| ViaError::MissingRegistry {
                key: format!("{}/{}", kind.key(), id),
            })?;
        to.id_of(kind, fallback).ok_or_else(|| {
            ViaError::MissingRegistry {
                key: format!(
                    "{}/{} (fallback \"{}\" absent in target)",
                    kind.key(),
                    id,
                    fallback
                ),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tiny version snapshots modelling the 1.13 flattening-era rename
    /// `minecraft:grass -> minecraft:grass_block` (1.8 style names vs 1.13+).
    fn v_old() -> IdMap {
        let mut m = IdMap::new();
        m.insert(RegistryKind::Block, 1, "minecraft:stone");
        m.insert(RegistryKind::Block, 2, "minecraft:grass");
        m.insert(RegistryKind::Block, 3, "minecraft:dirt");
        m.insert(RegistryKind::Item, 1, "minecraft:stone");
        m.insert(RegistryKind::Item, 4, "minecraft:diamond_sword");
        m
    }

    fn v_new() -> IdMap {
        let mut m = IdMap::new();
        m.insert(RegistryKind::Block, 100, "minecraft:stone");
        m.insert(RegistryKind::Block, 101, "minecraft:grass_block");
        m.insert(RegistryKind::Block, 102, "minecraft:dirt");
        m.insert(RegistryKind::Item, 200, "minecraft:stone");
        m.insert(RegistryKind::Item, 201, "minecraft:diamond_sword");
        m
    }

    #[test]
    fn insert_then_lookup_both_ways() {
        let m = v_old();
        // Full cycle: insert -> name_of -> id_of.
        assert_eq!(m.name_of(RegistryKind::Block, 1), Some("minecraft:stone"));
        assert_eq!(m.name_of(RegistryKind::Block, 2), Some("minecraft:grass"));
        assert_eq!(m.id_of(RegistryKind::Block, "minecraft:stone"), Some(1));
        assert_eq!(m.id_of(RegistryKind::Block, "minecraft:grass"), Some(2));
        assert_eq!(m.id_of(RegistryKind::Item, "minecraft:diamond_sword"), Some(4));
        // Unknown on both sides.
        assert_eq!(m.name_of(RegistryKind::Block, 9999), None);
        assert_eq!(m.id_of(RegistryKind::Block, "minecraft:zenith"), None);
        // Kinds do not leak into each other.
        assert_eq!(m.name_of(RegistryKind::Entity, 1), None);
        assert_eq!(m.id_of(RegistryKind::Item, "minecraft:stone"), Some(1));
    }

    #[test]
    fn reinsert_is_idempotent() {
        let mut m = IdMap::new();
        m.insert(RegistryKind::Block, 1, "minecraft:stone");
        m.insert(RegistryKind::Block, 1, "minecraft:stone");
        assert_eq!(m.len(RegistryKind::Block), 1);
        assert_eq!(m.id_of(RegistryKind::Block, "minecraft:stone"), Some(1));
    }

    #[test]
    fn len_and_empty_per_kind() {
        let m = v_old();
        assert_eq!(m.len(RegistryKind::Block), 3);
        assert_eq!(m.len(RegistryKind::Item), 2);
        assert_eq!(m.len(RegistryKind::Sound), 0);
        assert!(m.is_empty(RegistryKind::Sound));
        assert!(!m.is_empty(RegistryKind::Block));
    }

    #[test]
    fn remap_basic_translation() {
        let (old, new) = (v_old(), v_new());
        // Same name, different id: 1.12 stone(1) -> 1.13 stone(100).
        assert_eq!(remap(RegistryKind::Block, 1, &old, &new), Some(100));
        assert_eq!(remap(RegistryKind::Block, 3, &old, &new), Some(102));
        // Reverse direction works too: 26.3 dirt(102) -> 1.12 dirt(3).
        assert_eq!(remap(RegistryKind::Block, 102, &new, &old), Some(3));
    }

    #[test]
    fn remap_renamed_grass_to_grass_block() {
        let (old, new) = (v_old(), v_new());
        // 1.8 "minecraft:grass" was renamed to "minecraft:grass_block".
        // A registry set that models the rename must already carry the
        // corrected name at the *source* side (name-level fixes are done
        // when the data files are built); remap itself only pivots names.
        // Straight pivot with a name present on both sides:
        let mut old_fixed = v_old();
        old_fixed.insert(RegistryKind::Block, 2, "minecraft:grass_block");
        assert_eq!(
            remap(RegistryKind::Block, 2, &old_fixed, &new),
            Some(101)
        );
        // And the raw 1.8 name that does not exist at 1.13+: unmappable,
        // caller's fallback policy kicks in.
        assert_eq!(remap(RegistryKind::Block, 2, &old, &new), None);
    }

    #[test]
    fn remap_unknown_source_id_is_none() {
        let (old, new) = (v_old(), v_new());
        assert_eq!(remap(RegistryKind::Block, 404, &old, &new), None);
        assert_eq!(remap(RegistryKind::Enchantment, 1, &old, &new), None);
    }

    #[test]
    fn registry_set_indexing() {
        let mut set = RegistrySet::default();
        assert_eq!(set.version_count(), 0);
        assert!(set.at(0).is_none());
        set.push(v_old());
        set.push(v_new());
        assert_eq!(set.version_count(), 2);
        assert_eq!(set.at(0).unwrap().name_of(RegistryKind::Block, 2), Some("minecraft:grass"));
        assert_eq!(set.at(1).unwrap().name_of(RegistryKind::Block, 102), Some("minecraft:dirt"));
        assert!(set.at(2).is_none());
        // from_maps keeps order.
        let direct = RegistrySet::from_maps(vec![v_old(), v_new()]);
        assert_eq!(direct.version_count(), 2);
    }

    #[test]
    fn remapper_defaults_table() {
        let r = Remapper::with_defaults();
        assert_eq!(r.fallback_for(RegistryKind::Block), Some("minecraft:stone"));
        assert_eq!(r.fallback_for(RegistryKind::Item), Some("minecraft:stone"));
        assert_eq!(r.fallback_for(RegistryKind::Enchantment), None);
        assert_eq!(r.fallback_for(RegistryKind::Entity), None);
        assert_eq!(r.fallback_for(RegistryKind::Particle), None);
        assert_eq!(r.fallback_for(RegistryKind::Sound), None);
    }

    #[test]
    fn fallback_to_stone_for_unmapped_block() {
        let (old, new) = (v_old(), v_new());
        let r = Remapper::with_defaults();
        // Removed/re-named block (raw 1.8 grass): falls back to stone's id
        // in the TARGET version.
        let got = r.remap_or_fallback(RegistryKind::Block, 2, &old, &new);
        assert_eq!(got.unwrap(), 100); // minecraft:stone at 26.3 snapshot
        // Same for an id entirely absent from the source map.
        let got = r.remap_or_fallback(RegistryKind::Block, 404, &old, &new);
        assert_eq!(got.unwrap(), 100);
        // And for items.
        let got = r.remap_or_fallback(RegistryKind::Item, 999, &old, &new);
        assert_eq!(got.unwrap(), 200); // minecraft:stone item
    }

    #[test]
    fn missing_registry_when_no_fallback() {
        let (old, new) = (v_old(), v_new());
        let r = Remapper::with_defaults();
        let err = r
            .remap_or_fallback(RegistryKind::Enchantment, 5, &old, &new)
            .expect_err("no enchant fallback -> must be MissingRegistry");
        match err {
            ViaError::MissingRegistry { ref key } => {
                assert!(key.contains("minecraft:enchantment"), "key was {key}");
                assert!(key.contains('5'), "key was {key}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // Not fatal: drop the entry, keep the session.
        assert!(!err.is_fatal());
    }

    #[test]
    fn missing_registry_when_fallback_absent_in_target() {
        let (old, new) = (v_old(), v_new());
        let mut r = Remapper::with_defaults();
        // Fallback name that does not exist in the target map at all.
        r.set_fallback(RegistryKind::Block, Some("minecraft:zenith"));
        let err = r
            .remap_or_fallback(RegistryKind::Block, 404, &old, &new)
            .expect_err("bad fallback -> must be MissingRegistry");
        match err {
            ViaError::MissingRegistry { ref key } => {
                assert!(key.contains("fallback"), "key was {key}");
                assert!(key.contains("minecraft:zenith"), "key was {key}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // set_fallback(None) removes the entry again.
        r.set_fallback(RegistryKind::Block, None);
        assert_eq!(r.fallback_for(RegistryKind::Block), None);
    }

    #[test]
    fn remapper_passthrough_when_mappable() {
        let (old, new) = (v_old(), v_new());
        let r = Remapper::with_defaults();
        // diamond sword survives by name
        assert_eq!(r.remap_or_fallback(RegistryKind::Item, 4, &old, &new).unwrap(), 201);
    }
}
