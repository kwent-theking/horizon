//! Item-slot decoding, encoding and registry remapping across versions.
//!
//! Inventories, window clicks, entity equipment and creative-mode screens
//! all carry *slots* ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â an item stack on the wire. The slot's outer shape
//! changed four times in Minecraft history: numeric `Short id` (up to the
//! flattening era), `VarInt id`, `Boolean present + VarInt id`, and the
//! 1.20.5+ data-component form used by 26.3. This module knows those four
//! *shapes* and converts between them; the *identity* translation
//! (`diamond_sword` was id 4 at 1.12, id 201 at 26.3) is delegated to
//! [`crate::registry_map`] ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â that is the single pivot, this file is a
//! consumer, never a duplicate, of it.
//!
//! Shape boundaries are deliberately simplified with honest FIXMEs (see
//! [`slot_shape`]): the mechanism is per-shape and localized in one match,
//! so when the orchestrator pins exact per-version widths and boundaries
//! only the match arms and field readers change.
//!
//! NBT and 26.3 component payloads are *passthrough*: bytes are captured
//! on decode and re-emitted verbatim on encode. Component <-> legacy NBT
//! conversion is NOT this module's problem (see the FIXMEs).
//!
//! RU: Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² ÃƒÂÃ‚Â² ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¦. ÃƒÂÃ¢â‚¬â„¢ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒâ€˜Ã‹â€ ÃƒÂÃ‚Â½Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã‚Â Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™ 4 Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â° ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â
//! ÃƒÂÃ‚Â·ÃƒÂÃ‚Â´ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™ ÃƒÂÃ‚Â·ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â²Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Âµ Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â¸ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¶ÃƒÂÃ‚Â´Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¸; ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ id ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²
//! ÃƒÂÃ‚Â´ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã†â€™ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã‚Â registry_map (ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â°, ÃƒÂÃ‚Â½ÃƒÂÃ‚Âµ Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â°). NBT ÃƒÂÃ‚Â¸ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ 26.3
//! ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â·Ãƒâ€˜Ã…â€™ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡-ÃƒÂÃ‚Â²-ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡, ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â¦ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â½ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â·ÃƒÂÃ‚Â´ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™.

use crate::errors::ViaError;
use crate::frame::{read_varint, write_varint};
use crate::nbt_compat::skip_named_tag;
use crate::registry_map::{IdMap, RegistryKind, Remapper};

/// The canonical id of an empty slot in [`LegacySlot`], normalized across
/// all shapes: -1 for the Short/VarInt shapes, and also what a decoded
/// `present = false` (1.16.2+) or `id = 0` (1.20.5+) empty slot collapses
/// to, so downstream code has one emptiness test.
///
/// RU: ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¹ id ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° (-1) ÃƒÂÃ‚Â´ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â²Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Â¦ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼.
pub const EMPTY_SLOT_ID: i32 = -1;

/// How a bare item *reference* (id + damage/meta, as opposed to a full
/// inventory slot) is laid out in legacy packets such as entity metadata
/// or the pre-1.13 "item with meta" fields.
///
/// - `IdShortMetaUShort`: `Short id + UShort meta` (the 1.8-1.12 era,
///   e.g. equipment in metadata).
/// - `IdVarInt`: `VarInt id` (post-flattening; meta became a component).
///
/// FIXME(orchestrator): the boundary below uses the same simplified cut as
/// [`slot_shape`] (protocol 405); the real transition (1.13 flattening vs
/// metadata rewrite) must be pinned against per-version packet layouts.
///
/// RU: Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â° Ãƒâ€šÃ‚Â«ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾Ãƒâ€šÃ‚Â» ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° (id + meta) ÃƒÂÃ‚Â² ÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸-ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Â¦.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LegacyItemShape {
    /// `Short id` followed by `UShort damage/meta`.
    IdShortMetaUShort,
    /// `VarInt id` alone (meta lives elsewhere).
    IdVarInt,
}

/// Which bare-item layout `protocol` uses. See the enum docs and the
/// boundary FIXME.
///
/// RU: ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°Ãƒâ€˜Ã‚Â Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â° Ãƒâ€šÃ‚Â«ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾Ãƒâ€šÃ‚Â» ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚Â´ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â°.
pub fn item_shape(protocol: i32) -> LegacyItemShape {
    // FIXME: exact boundary is the orchestrator's to pin (see enum docs).
    match protocol {
        47..=404 => LegacyItemShape::IdShortMetaUShort,
        _ => LegacyItemShape::IdVarInt,
    }
}

/// The four outer shapes an inventory *slot* takes on the wire.
///
/// FIXME(orchestrator): the ranges below are the agreed simplification.
/// Truth on the ground is messier: 1.13 flattened item ids but slot fields
/// kept Short width until 1.11-era VarInt experiments, 1.16.2 (758) is the
/// real `Boolean present` start, and 1.20.5 (767) introduced components.
/// Field widths (Short vs Byte vs VarInt count) are likewise simplified
/// here ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â see the per-shape branches of [`decode_slot`]/[`encode_slot`].
///
/// RU: Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â²ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒâ€˜Ã‹â€ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âµ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼Ãƒâ€˜Ã¢â‚¬Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°; ÃƒÂÃ‚Â³Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â Ãƒâ€˜Ã¢â‚¬Â¹ Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹, FIXME.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemSlotShape {
    /// `Short id` (-1 = empty); if present: `Short count`, optional NBT.
    IdShort,
    /// `VarInt id` (negative = empty); if present: `VarInt count`, NBT.
    IdVarInt,
    /// `Boolean present`; if true: `VarInt id`, `Byte count`, optional NBT.
    PresentBoolIdVarInt,
    /// 1.20.5+/26.3: `VarInt id` (0 = empty); if present: `VarInt count`,
    /// then the data-component payload (passthrough here).
    Components,
}

/// Which slot shape `protocol` uses.
///
/// Boundaries (simplified, see [`ItemSlotShape`] FIXMEs):
/// `47..=404` -> IdShort, `405..=757` -> IdVarInt, `758..=766` ->
/// PresentBoolIdVarInt, everything else (>= 767, and any unknown/negative
/// value ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â those are rejected by the handshake long before this module)
/// -> Components.
///
/// RU: Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° ÃƒÂÃ‚Â´ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â°.
pub fn slot_shape(protocol: i32) -> ItemSlotShape {
    // FIXME: boundaries simplified; the orchestrator pins exact cuts.
    match protocol {
        47..=404 => ItemSlotShape::IdShort,
        405..=750 => ItemSlotShape::IdVarInt,
        751..=766 => ItemSlotShape::PresentBoolIdVarInt, // present-bool since 1.16.2 (751)
        _ => ItemSlotShape::Components, // 767+ = 1.20.5 components
    }
}

/// A decoded inventory slot, normalized over all four shapes.
///
/// Empty slots (whatever their on-wire encoding: Short -1, VarInt -1,
/// `present = false`, component id 0) decode to [`EMPTY_SLOT_ID`] with
/// `count = 0` and no payload, and [`LegacySlot::is_empty`] recognizes them.
///
/// `nbt` holds the raw legacy-NBT bytes (tag id included, `0x00` marker
/// excluded) for the three legacy shapes; `components` holds the raw 26.3
/// component payload for the Components shape. Both are passthrough: this
/// module never interprets them, only moves them. A 26.3 slot therefore
/// has `components` set and `nbt = None` (and vice versa).
///
/// RU: Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡, ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â´ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â²Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Â¦ Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â‚¬Â¦ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼.
/// ÃƒÂÃ…Â¸Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ = id -1, count 0. NBT/ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â·Ãƒâ€˜Ã…â€™.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacySlot {
    /// Numeric item id, or [`EMPTY_SLOT_ID`] for an empty slot.
    /// RU: Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ id ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°; -1 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡.
    pub id: i32,
    /// Stack size. 0 for empty slots.
    /// RU: Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°; 0 Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾.
    pub count: i32,
    /// Raw legacy NBT bytes (passthrough), if the slot carries any.
    /// RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸-NBT, ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™.
    pub nbt: Option<Vec<u8>>,
    /// Raw 26.3 data-component payload (passthrough), if any.
    /// Component <-> NBT conversion is NOT done here.
    /// FIXME: real 1.20.5+ component payload layouts (NBT-era and the
    /// later binary form) are pinned by the component translator when
    /// wiring arrives; here they are opaque bytes.
    /// RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² 26.3 (ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â·Ãƒâ€˜Ã…â€™).
    pub components: Option<Vec<u8>>,
}

impl LegacySlot {
    /// An empty slot: [`EMPTY_SLOT_ID`], zero count, no payload.
    ///
    /// RU: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡.
    pub fn empty() -> Self {
        LegacySlot {
            id: EMPTY_SLOT_ID,
            count: 0,
            nbt: None,
            components: None,
        }
    }

    /// Whether this slot holds nothing. True exactly when
    /// `id == `[`EMPTY_SLOT_ID`].
    ///
    /// RU: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡.
    pub fn is_empty(&self) -> bool {
        self.id == EMPTY_SLOT_ID
    }
}

/// Take `n` bytes at `*pos`, advancing `*pos`, or [`ViaError::Malformed`].
/// Local twin of the private helper in `nbt_compat` (kept private there).
/// RU: ÃƒÂÃ‚Â²ÃƒÂÃ‚Â·Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ n ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡ Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸ pos, ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Âµ Malformed.
fn take<'a>(buf: &'a [u8], pos: &mut usize, n: usize, context: &str) -> Result<&'a [u8], ViaError> {
    if buf.len() < *pos + n {
        return Err(ViaError::Malformed {
            context: format!("{} truncated", context),
            bytes_seen: buf.len(),
        });
    }
    let slice = &buf[*pos..*pos + n];
    *pos += n;
    Ok(slice)
}

/// Read one big-endian `i16`.
/// RU: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ i16 big-endian.
fn read_i16(buf: &[u8], pos: &mut usize, context: &str) -> Result<i16, ViaError> {
    let b = take(buf, pos, 2, context)?;
    Ok(i16::from_be_bytes([b[0], b[1]]))
}

/// Read one byte.
/// RU: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡.
fn read_u8(buf: &[u8], pos: &mut usize, context: &str) -> Result<u8, ViaError> {
    Ok(take(buf, pos, 1, context)?[0])
}

/// Read one strict Boolean (0 = false, 1 = true; anything else is
/// malformed ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â a translator must not guess on hostile input).
/// RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¹ bool: Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ 0 ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ 1.
fn read_bool(buf: &[u8], pos: &mut usize, context: &str) -> Result<bool, ViaError> {
    match read_u8(buf, pos, context)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(ViaError::Malformed {
            context: format!("{}: boolean byte {} (expected 0 or 1)", context, other),
            bytes_seen: *pos,
        }),
    }
}

/// Read the optional legacy NBT of a slot: a leading `0x00` byte means
/// "no NBT" (consumed, `None` returned); anything else is a full named
/// root tag, which is *skipped* (bounds- and depth-checked by
/// [`skip_named_tag`]) and its raw bytes captured for passthrough.
///
/// RU: ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸-NBT Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°: 0x00 = ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡; ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ ÃƒÂÃ‚Â¸
/// ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹.
fn read_optional_legacy_nbt(
    buf: &[u8],
    pos: &mut usize,
) -> Result<Option<Vec<u8>>, ViaError> {
    if *pos >= buf.len() {
        return Err(ViaError::Malformed {
            context: "slot nbt tag id truncated".to_string(),
            bytes_seen: buf.len(),
        });
    }
    if buf[*pos] == 0x00 {
        *pos += 1;
        return Ok(None);
    }
    let start = *pos;
    skip_named_tag(buf, pos)?;
    Ok(Some(buf[start..*pos].to_vec()))
}

/// Decode one slot starting at `*pos` (advanced past the slot) at
/// `protocol`'s shape. Empty slots of every shape normalize to
/// [`LegacySlot::empty`].
///
/// Per-shape wire layout (simplified widths, see [`ItemSlotShape`] FIXMEs):
/// - `IdShort`: `Short id` (negative -> empty, nothing follows);
///   `Short count`; optional NBT.
/// - `IdVarInt`: `VarInt id` (negative -> empty); `VarInt count`; NBT.
/// - `PresentBoolIdVarInt`: `Boolean present`; `VarInt id`; `Byte count`;
///   optional NBT.
/// - `Components`: `VarInt id` (0 or negative -> empty); `VarInt count`;
///   component passthrough payload (leading `0x00` = none).
///
/// RU: Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸; ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½ÃƒÂÃ‚Â±ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â
/// ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â·Ãƒâ€˜Ã†â€™ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã‚Â ÃƒÂÃ‚Â² id -1 / count 0.
pub fn decode_slot(
    bytes: &[u8],
    pos: &mut usize,
    protocol: i32,
) -> Result<LegacySlot, ViaError> {
    match slot_shape(protocol) {
        ItemSlotShape::IdShort => {
            // FIXME: real pre-1.13 slot widths (Short vs Byte count) per
            // version are the orchestrator's to pin; simplified as Short/Short.
            let id = i32::from(read_i16(bytes, pos, "slot short item id")?);
            if id < 0 {
                return Ok(LegacySlot::empty());
            }
            let count = i32::from(read_i16(bytes, pos, "slot short count")?);
            let nbt = read_optional_legacy_nbt(bytes, pos)?;
            Ok(LegacySlot {
                id,
                count,
                nbt,
                components: None,
            })
        }
        ItemSlotShape::IdVarInt => {
            // FIXME: 1.13-1.15 real slots keep Short/Byte fields; the
            // VarInt/VarInt form here is the agreed simplification.
            let id = read_varint(bytes, pos)?;
            if id < 0 {
                return Ok(LegacySlot::empty());
            }
            let count = read_varint(bytes, pos)?;
            let nbt = read_optional_legacy_nbt(bytes, pos)?;
            Ok(LegacySlot {
                id,
                count,
                nbt,
                components: None,
            })
        }
        ItemSlotShape::PresentBoolIdVarInt => {
            let present = read_bool(bytes, pos, "slot present boolean")?;
            if !present {
                return Ok(LegacySlot::empty());
            }
            let id = read_varint(bytes, pos)?;
            // FIXME: Byte count is the real 1.16.2+ width; keep until
            // per-version tables say otherwise.
            let count = i32::from(read_u8(bytes, pos, "slot byte count")?);
            let nbt = read_optional_legacy_nbt(bytes, pos)?;
            Ok(LegacySlot {
                id,
                count,
                nbt,
                components: None,
            })
        }
        ItemSlotShape::Components => {
            let id = read_varint(bytes, pos)?;
            // 1.20.5+: id 0 is the empty marker; a negative pattern is a
            // hostile server bug, tolerated as empty rather than trusted.
            if id <= 0 {
                return Ok(LegacySlot::empty());
            }
            let count = read_varint(bytes, pos)?;
            // FIXME: full 26.3 component decoding (NBT-era form and the
            // later binary component map) arrives with wiring; passthrough
            // for now, using the same 0x00-marker convention as legacy NBT.
            let components = read_optional_legacy_nbt(bytes, pos)?;
            Ok(LegacySlot {
                id,
                count,
                nbt: None,
                components,
            })
        }
    }
}

/// Encode `slot` at `protocol`'s shape, appending to `out`. Symmetric with
/// [`decode_slot`]; an empty slot encodes per its shape's empty rule
/// (`Short -1`, `VarInt -1`, `Boolean false`, component id `0`).
///
/// Errors ([`ViaError::Malformed`], non-fatal) only for fields that
/// cannot be expressed in the target shape: an id or count outside the
/// shape's width, a negative count, or a components-shape id of 0 on a
/// non-empty slot (0 is reserved for "empty").
///
/// RU: ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸; ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â»Ãƒâ€˜Ã†â€™ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¼Ãƒâ€˜Ã¢â‚¬Â¹.
pub fn encode_slot(
    slot: &LegacySlot,
    protocol: i32,
    out: &mut Vec<u8>,
) -> Result<(), ViaError> {
    if slot.is_empty() {
        match slot_shape(protocol) {
            ItemSlotShape::IdShort => out.extend_from_slice(&(-1i16).to_be_bytes()),
            ItemSlotShape::IdVarInt => write_varint(-1, out),
            ItemSlotShape::PresentBoolIdVarInt => out.push(0x00),
            ItemSlotShape::Components => out.push(0x00),
        }
        return Ok(());
    }

    // Non-empty slot: a count below 1 is never expressible.
    if slot.count < 1 {
        return Err(ViaError::Malformed {
            context: format!("slot count {} is below 1", slot.count),
            bytes_seen: 0,
        });
    }

    match slot_shape(protocol) {
        ItemSlotShape::IdShort => {
            // FIXME: widths simplified (see decode_slot).
            let id = i16::try_from(slot.id).map_err(|_| ViaError::Malformed {
                context: format!("slot id {} does not fit a Short", slot.id),
                bytes_seen: 0,
            })?;
            let count = i16::try_from(slot.count).map_err(|_| ViaError::Malformed {
                context: format!("slot count {} does not fit a Short", slot.count),
                bytes_seen: 0,
            })?;
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&count.to_be_bytes());
            match &slot.nbt {
                Some(nbt) => out.extend_from_slice(nbt),
                None => out.push(0x00),
            }
        }
        ItemSlotShape::IdVarInt => {
            // FIXME: widths simplified (see decode_slot).
            write_varint(slot.id, out);
            write_varint(slot.count, out);
            match &slot.nbt {
                Some(nbt) => out.extend_from_slice(nbt),
                None => out.push(0x00),
            }
        }
        ItemSlotShape::PresentBoolIdVarInt => {
            out.push(0x01);
            write_varint(slot.id, out);
            let count = u8::try_from(slot.count).map_err(|_| ViaError::Malformed {
                context: format!("slot count {} does not fit a Byte", slot.count),
                bytes_seen: 0,
            })?;
            out.push(count);
            match &slot.nbt {
                Some(nbt) => out.extend_from_slice(nbt),
                None => out.push(0x00),
            }
        }
        ItemSlotShape::Components => {
            // 0 is the empty marker in the 1.20.5+/26.3 form.
            if slot.id == 0 {
                return Err(ViaError::Malformed {
                    context: "components slot id 0 is reserved for the empty slot".to_string(),
                    bytes_seen: 0,
                });
            }
            write_varint(slot.id, out);
            write_varint(slot.count, out);
            match &slot.components {
                Some(components) => out.extend_from_slice(components),
                // FIXME: real "no components" marker for 26.3 is pinned by
                // the component translator; 0x00 matches the NBT-era form.
                None => out.push(0x00),
            }
        }
    }
    Ok(())
}

/// Translate `slot`'s item id between two versions through
/// [`crate::registry_map`] ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â the name pivot, never a local table.
///
/// Empty slots pass through untouched. Non-empty ids go through
/// [`Remapper::remap_or_fallback`]: an unmappable id either lands on the
/// remapper's fallback (items default to `minecraft:stone`) or bubbles up
/// as [`ViaError::MissingRegistry`]. The NBT/component payloads are left
/// verbatim ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â their id-bearing fields (enchant ids inside NBT etc.) are
/// rewritten by the NBT/component translator, not here.
///
/// RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸ id ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¶ÃƒÂÃ‚Â´Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¸ Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· registry_map
/// (remap_or_fallback). ÃƒÂÃ…Â¸Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¸ payload ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¹.
pub fn remap_slot(
    slot: &mut LegacySlot,
    kind: RegistryKind,
    from: &IdMap,
    to: &IdMap,
    remapper: &Remapper,
) -> Result<(), ViaError> {
    if slot.is_empty() {
        return Ok(());
    }
    // is_empty() guarantees id >= 0, so the u32 cast is lossless.
    let mapped = remapper.remap_or_fallback(kind, slot.id as u32, from, to)?;
    slot.id = mapped as i32;
    Ok(())
}

/// Minimal 26.3 item-stack stub: `VarInt(count) + VarInt(0)` (0 = an
/// empty component map).
///
/// FIXME: full 26.3 component encoding arrives with wiring. The `name`
/// parameter is deliberately unused here: resolving an item *name* to a
/// 26.3 id needs an [`IdMap`], which this stub does not yet receive; the
/// wired version will look the name up and emit
/// `VarInt(id) VarInt(count) components`.
///
/// RU: ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚ÂºÃƒÂÃ‚Â° 26.3: VarInt(count) + VarInt(0). ÃƒÂÃ…Â¸ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚Â ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°
/// ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹.
pub fn item_name_to_263_components(name: &str, count: i32) -> Result<Vec<u8>, ViaError> {
    if count < 0 {
        return Err(ViaError::Malformed {
            context: format!("item stack count {} is negative", count),
            bytes_seen: 0,
        });
    }
    // FIXME(orchestrator): name -> id lookup via IdMap when wiring lands.
    let _ = name;
    let mut out = Vec::with_capacity(4);
    write_varint(count, &mut out);
    write_varint(0, &mut out);
    Ok(out)
}

/// The 26.3 empty-slot encoding: a single `VarInt(0)` (id 0 = not present).
///
/// RU: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ 26.3 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡ 0x00.
pub fn empty_slot_263() -> Vec<u8> {
    vec![0x00]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry_map::RegistryKind;

    /// A minimal but valid legacy-NBT root: TAG_Compound with an empty
    /// name and no children ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â `[0x0A, name_len=0x0000, TAG_End]`.
    /// RU: ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â´ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã…â€™ NBT: ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â½ÃƒÂÃ‚Â´ Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼.
    const NBT_EMPTY_COMPOUND: &[u8] = &[0x0A, 0x00, 0x00, 0x00];

    // ------------------------------------------------------------------
    // Shape selection
    // ------------------------------------------------------------------

    #[test]
    fn slot_shape_boundaries() {
        assert_eq!(slot_shape(47), ItemSlotShape::IdShort); // 1.8
        assert_eq!(slot_shape(110), ItemSlotShape::IdShort);
        assert_eq!(slot_shape(404), ItemSlotShape::IdShort);
        assert_eq!(slot_shape(405), ItemSlotShape::IdVarInt);
        assert_eq!(slot_shape(477), ItemSlotShape::IdVarInt); // 1.14
        assert_eq!(slot_shape(754), ItemSlotShape::PresentBoolIdVarInt); // 1.16.5: present-bool since 751
        assert_eq!(slot_shape(750), ItemSlotShape::IdVarInt);
        assert_eq!(slot_shape(758), ItemSlotShape::PresentBoolIdVarInt);
        assert_eq!(slot_shape(760), ItemSlotShape::PresentBoolIdVarInt);
        assert_eq!(slot_shape(766), ItemSlotShape::PresentBoolIdVarInt); // 1.20.5 slots: present-bool
        assert_eq!(slot_shape(767), ItemSlotShape::Components); // 1.21: components
        assert_eq!(slot_shape(999_999), ItemSlotShape::Components);
    }

    #[test]
    fn item_shape_boundaries() {
        assert_eq!(item_shape(47), LegacyItemShape::IdShortMetaUShort);
        assert_eq!(item_shape(404), LegacyItemShape::IdShortMetaUShort);
        assert_eq!(item_shape(405), LegacyItemShape::IdVarInt);
        assert_eq!(item_shape(767), LegacyItemShape::IdVarInt);
        assert_eq!(item_shape(999_999), LegacyItemShape::IdVarInt);
    }

    // ------------------------------------------------------------------
    // Round-trips, one per shape (manual golden bytes)
    // ------------------------------------------------------------------

    #[test]
    fn roundtrip_id_short_with_and_without_nbt() {
        // id=5, count=2, NBT compound.
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x00, 0x05]); // Short id 5
        wire.extend_from_slice(&[0x00, 0x02]); // Short count 2
        wire.extend_from_slice(NBT_EMPTY_COMPOUND);

        let mut pos = 0;
        let slot = decode_slot(&wire, &mut pos, 47).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(pos, wire.len(), "decode must consume exactly the slot");
        assert_eq!(
            slot,
            LegacySlot {
                id: 5,
                count: 2,
                nbt: Some(NBT_EMPTY_COMPOUND.to_vec()),
                components: None,
            }
        );

        let mut re = Vec::new();
        encode_slot(&slot, 47, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, wire, "IdShort round-trip must be byte-identical");

        // Same slot without NBT: trailing 0x00 marker.
        let mut no_nbt = Vec::new();
        no_nbt.extend_from_slice(&[0x00, 0x05, 0x00, 0x02, 0x00]);
        let mut pos = 0;
        let slot = decode_slot(&no_nbt, &mut pos, 340).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.nbt, None);
        assert_eq!(pos, no_nbt.len());
        let mut re = Vec::new();
        encode_slot(&slot, 340, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, no_nbt);
    }

    #[test]
    fn roundtrip_id_varint() {
        // id=5, count=64, NBT compound; both VarInts are single-byte here.
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x05]); // VarInt id 5
        wire.extend_from_slice(&[0x40]); // VarInt count 64
        wire.extend_from_slice(NBT_EMPTY_COMPOUND);

        let mut pos = 0;
        let slot = decode_slot(&wire, &mut pos, 405).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(pos, wire.len());
        assert_eq!(slot.id, 5);
        assert_eq!(slot.count, 64);
        assert_eq!(slot.nbt, Some(NBT_EMPTY_COMPOUND.to_vec()));
        assert!(slot.components.is_none());

        let mut re = Vec::new();
        encode_slot(&slot, 405, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, wire, "IdVarInt round-trip must be byte-identical");

        // Multi-byte VarInts: id 300, count 200.
        let mut big = Vec::new();
        write_varint(300, &mut big);
        write_varint(200, &mut big);
        big.push(0x00); // no NBT
        let mut pos = 0;
        let slot = decode_slot(&big, &mut pos, 405).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!((slot.id, slot.count), (300, 200));
        assert_eq!(pos, big.len());
        let mut re = Vec::new();
        encode_slot(&slot, 405, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, big);
    }

    #[test]
    fn roundtrip_present_bool_id_varint() {
        // present=true, id=5, count=2 (Byte), NBT compound.
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x01, 0x05, 0x02]);
        wire.extend_from_slice(NBT_EMPTY_COMPOUND);

        let mut pos = 0;
        let slot = decode_slot(&wire, &mut pos, 758).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(pos, wire.len());
        assert_eq!(
            slot,
            LegacySlot {
                id: 5,
                count: 2,
                nbt: Some(NBT_EMPTY_COMPOUND.to_vec()),
                components: None,
            }
        );

        let mut re = Vec::new();
        encode_slot(&slot, 766, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, wire, "PresentBool round-trip must be byte-identical");
    }

    #[test]
    fn roundtrip_components() {
        // 26.3: id=5 (VarInt), count=2 (VarInt), component payload
        // (passthrough bytes; here a compound-shaped blob).
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x05, 0x02]);
        wire.extend_from_slice(NBT_EMPTY_COMPOUND);

        let mut pos = 0;
        let slot = decode_slot(&wire, &mut pos, 767).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(pos, wire.len());
        assert_eq!(
            slot,
            LegacySlot {
                id: 5,
                count: 2,
                nbt: None,
                components: Some(NBT_EMPTY_COMPOUND.to_vec()),
            }
        );

        let mut re = Vec::new();
        encode_slot(&slot, 999_999, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, wire, "Components round-trip must be byte-identical");

        // No components: trailing 0x00 marker.
        let bare = [0x05, 0x02, 0x00];
        let mut pos = 0;
        let slot = decode_slot(&bare, &mut pos, 767).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.components, None);
        let mut re = Vec::new();
        encode_slot(&slot, 767, &mut re).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(re, bare);
    }

    // ------------------------------------------------------------------
    // Empty slots: every shape decodes to the normalized empty and
    // encodes back to its shape's golden empty bytes.
    // ------------------------------------------------------------------

    #[test]
    fn empty_slot_every_shape() {
        // IdShort: Short -1, nothing follows (NBT reading is skipped).
        let short_empty = [0xFF, 0xFF, 0xAA]; // 0xAA = next packet's bytes
        let mut pos = 0;
        let slot = decode_slot(&short_empty, &mut pos, 47).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(slot.count, 0);
        assert_eq!(pos, 2, "trailing bytes must stay untouched");

        // IdVarInt: VarInt -1 (5 bytes, two's complement pattern).
        let varint_empty = [0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        let mut pos = 0;
        let slot = decode_slot(&varint_empty, &mut pos, 477).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 5);

        // PresentBool: Boolean false.
        let mut pos = 0;
        let slot = decode_slot(&[0x00], &mut pos, 758).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 1);

        // Components: VarInt id 0 = not present.
        let mut pos = 0;
        let slot = decode_slot(&[0x00], &mut pos, 767).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 1);

        // Encode side: each shape's golden empty bytes.
        let empty = LegacySlot::empty();
        let mut out = Vec::new();
        encode_slot(&empty, 47, &mut out).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out, [0xFF, 0xFF]);
        let mut out = Vec::new();
        encode_slot(&empty, 405, &mut out).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        let mut out = Vec::new();
        encode_slot(&empty, 758, &mut out).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out, [0x00]);
        let mut out = Vec::new();
        encode_slot(&empty, 767, &mut out).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out, [0x00]);

        // Round-trip: empty in, empty out, for every shape.
        for &protocol in &[47, 405, 758, 767] {
            let mut out = Vec::new();
            encode_slot(&empty, protocol, &mut out).unwrap_or_else(|e| panic!("{}", e));
            let mut pos = 0;
            let back = decode_slot(&out, &mut pos, protocol).unwrap_or_else(|e| panic!("{}", e));
            assert!(back.is_empty(), "protocol {}", protocol);
            assert_eq!(pos, out.len(), "protocol {}", protocol);
        }
    }

    #[test]
    fn negative_id_skips_nbt_reading() {
        // IdShort with a non-canonical negative id (-2): treated as empty,
        // count and NBT are never read; trailing bytes untouched.
        let buf = [0xFF, 0xFE, 0x11, 0x22];
        let mut pos = 0;
        let slot = decode_slot(&buf, &mut pos, 47).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 2);
        assert_eq!(&buf[pos..], &[0x11, 0x22]);

        // IdVarInt negative (e.g. -1 pattern) likewise stops after the id.
        let buf = [0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x42];
        let mut pos = 0;
        let slot = decode_slot(&buf, &mut pos, 405).unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 5);
        assert_eq!(buf[pos], 0x42);
    }

    // ------------------------------------------------------------------
    // Hostile input: malformed data must error, never panic.
    // ------------------------------------------------------------------

    #[test]
    fn varint_overflow_rejected() {
        // Six continuation bytes: never a valid VarInt.
        let buf = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        let mut pos = 0;
        let err = decode_slot(&buf, &mut pos, 405).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
        assert!(!err.is_fatal());

        // Same in the components shape's count field: id parses (5), count
        // then overflows.
        let buf = [0x05, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        let mut pos = 0;
        let err = decode_slot(&buf, &mut pos, 767).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn truncated_slot_rejected() {
        // IdShort: id read, count cut off.
        let err = decode_slot(&[0x00, 0x05], &mut 0, 47).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);

        // PresentBool: present=true, then buffer ends.
        let err = decode_slot(&[0x01], &mut 0, 758).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);

        // Components: id read, nothing after it.
        let err = decode_slot(&[0x05], &mut 0, 767).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn invalid_present_boolean_rejected() {
        // Boolean byte 2: neither false nor true.
        let err = decode_slot(&[0x02, 0x05, 0x01, 0x00], &mut 0, 758).unwrap_err();
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. } if context.contains("boolean")),
            "{}",
            err
        );
    }

    #[test]
    fn encode_rejects_out_of_range_fields() {
        // PresentBool count is a Byte: 256 does not fit.
        let slot = LegacySlot {
            id: 5,
            count: 256,
            nbt: None,
            components: None,
        };
        let err = encode_slot(&slot, 758, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. } if context.contains("Byte")),
            "{}",
            err
        );

        // IdShort id beyond Short range.
        let slot = LegacySlot {
            id: 40_000,
            count: 1,
            nbt: None,
            components: None,
        };
        let err = encode_slot(&slot, 47, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);

        // Negative count on a non-empty slot: never expressible.
        let slot = LegacySlot {
            id: 5,
            count: 0,
            nbt: None,
            components: None,
        };
        let err = encode_slot(&slot, 405, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);

        // Components shape: id 0 is the empty marker, not a valid item.
        let slot = LegacySlot {
            id: 0,
            count: 1,
            nbt: None,
            components: None,
        };
        let err = encode_slot(&slot, 767, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. } if context.contains("reserved")),
            "{}",
            err
        );
    }

    // ------------------------------------------------------------------
    // Registry remapping (delegated to registry_map, not duplicated).
    // ------------------------------------------------------------------

    /// Source snapshot (1.12-style ids) modelling the grass rename.
    /// RU: ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚Â ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼ grass.
    fn old_items() -> IdMap {
        let mut m = IdMap::new();
        m.insert(RegistryKind::Item, 1, "minecraft:stone");
        m.insert(RegistryKind::Item, 2, "minecraft:grass_block"); // renamed on data load
        m.insert(RegistryKind::Item, 4, "minecraft:diamond_sword");
        m
    }

    /// Target snapshot (26.3-style ids).
    /// RU: Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚Â ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°.
    fn new_items() -> IdMap {
        let mut m = IdMap::new();
        m.insert(RegistryKind::Item, 200, "minecraft:stone");
        m.insert(RegistryKind::Item, 101, "minecraft:grass_block");
        m.insert(RegistryKind::Item, 201, "minecraft:diamond_sword");
        m
    }

    #[test]
    fn remap_slot_renamed_item() {
        let (from, to) = (old_items(), new_items());
        let remapper = Remapper::with_defaults();

        let mut slot = LegacySlot {
            id: 2, // minecraft:grass_block at 1.12
            count: 3,
            nbt: None,
            components: None,
        };
        remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.id, 101, "renamed item must land on the 26.3 id");
        assert_eq!(slot.count, 3, "count is never touched by the remap");

        // Surviving name, different id.
        let mut slot = LegacySlot {
            id: 4,
            count: 1,
            nbt: None,
            components: None,
        };
        remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.id, 201);
    }

    #[test]
    fn remap_slot_fallback_for_unknown_id() {
        let (from, to) = (old_items(), new_items());
        let remapper = Remapper::with_defaults();

        // Id absent from the source map: falls back to stone in the target.
        let mut slot = LegacySlot {
            id: 999,
            count: 1,
            nbt: None,
            components: None,
        };
        remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.id, 200);
    }

    #[test]
    fn remap_slot_empty_is_noop() {
        let (from, to) = (old_items(), new_items());
        let remapper = Remapper::with_defaults();

        let mut slot = LegacySlot::empty();
        remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
    }

    #[test]
    fn remap_slot_missing_registry_without_fallback() {
        let (from, to) = (old_items(), new_items());
        // A remapper with no Item fallback: unmappable must be an error.
        let remapper = Remapper::default();

        let mut slot = LegacySlot {
            id: 999,
            count: 1,
            nbt: None,
            components: None,
        };
        let err = remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .expect_err("no fallback -> MissingRegistry");
        assert!(matches!(err, ViaError::MissingRegistry { .. }), "{}", err);
        assert!(!err.is_fatal(), "drop the entry, keep the session");
    }

    #[test]
    fn remap_then_encode_end_to_end() {
        // The full inventory story: decode a 1.8 slot, remap the id to
        // 26.3, encode as a components slot.
        let (from, to) = (old_items(), new_items());
        let remapper = Remapper::with_defaults();

        // 1.8 wire: Short id 4 (diamond_sword), Short count 1, no NBT.
        let wire = [0x00, 0x04, 0x00, 0x01, 0x00];
        let mut pos = 0;
        let mut slot = decode_slot(&wire, &mut pos, 47).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(pos, wire.len());

        remap_slot(&mut slot, RegistryKind::Item, &from, &to, &remapper)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(slot.id, 201);

        // 26.3 wire: VarInt id 201, VarInt count 1, 0x00 (no components).
        let mut out = Vec::new();
        encode_slot(&slot, 767, &mut out).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out, vec![0xC9, 0x01, 0x01, 0x00]);
    }

    // ------------------------------------------------------------------
    // 26.3 stubs
    // ------------------------------------------------------------------

    #[test]
    fn item_name_to_263_bytes() {
        // count 3, empty component map: VarInt(3) + VarInt(0).
        let bytes = item_name_to_263_components("minecraft:stone", 3)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(bytes, vec![0x03, 0x00]);

        // Multi-byte count: 128 -> [0x80, 0x01].
        let bytes = item_name_to_263_components("minecraft:diamond_sword", 128)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(bytes, vec![0x80, 0x01, 0x00]);

        // Zero count is still a valid (if useless) stack encoding.
        let bytes = item_name_to_263_components("minecraft:air", 0)
            .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(bytes, vec![0x00, 0x00]);

        // Negative count: rejected, non-fatal.
        let err = item_name_to_263_components("minecraft:stone", -1).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
        assert!(!err.is_fatal());
    }

    #[test]
    fn empty_slot_263_bytes() {
        assert_eq!(empty_slot_263(), vec![0x00]);
        // And it decodes back to the normalized empty slot.
        let mut pos = 0;
        let slot = decode_slot(&empty_slot_263(), &mut pos, 767)
            .unwrap_or_else(|e| panic!("{}", e));
        assert!(slot.is_empty());
        assert_eq!(pos, 1);
    }
}
