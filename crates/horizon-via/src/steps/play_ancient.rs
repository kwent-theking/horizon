//! PLAY-state translation for "ancient" pre-flattening versions (1.8-1.12,
//! protocol 47-340).
//!
//! EN: this module owns the *bottom* of the translation ladder. Everything
//! below 1.13 (protocol 393) speaks the pre-flattening world: blocks are
//! `id << 4 | meta` pairs packed two bytes per block, chunks are flat
//! `bitmask`-sliced section arrays with inline light, and packet ids shift
//! almost every release. ViaVersion climbs out of this era with small
//! per-step rewrites; we mirror that shape: exactly ONE step is exposed
//! deeply (1.8 -> 1.9, protocol 47 -> 107) and exactly TWO packets are
//! field-parsed (Join Game `0x01`, Chunk Data `0x21`, both at protocol 47
//! only). Everything else passes through untouched as
//! [`Translated18::Other`] until the per-version id tables arrive with the
//! wiring (see FIXMEs).
//!
//! ## Wire-format honesty (read this before extending)
//!
//! The 1.8 Chunk Data packet has **no** length field: `data` runs to the end
//! of the packet body. (1.7 had `compressedSize`; 1.13+ reintroduced a
//! VarInt `size`; 1.8 has neither.) So [`parse_chunk_18`] validates the data
//! length the only honest way available: it recomputes the *expected* length
//! from the primary bitmask via [`expected_chunk18_data_len`] and accepts
//! the sky-light and no-sky-light variants (plus the trailing 256-byte biome
//! array when `ground_up` is set). A mismatch is
//! [`ViaError::Malformed`](crate::errors::ViaError::Malformed) â€” non-fatal,
//! the packet is dropped, the session lives on.
//!
//! RU: Ð¼Ð¾Ð´ÑƒÐ»ÑŒ Ð²Ð»Ð°Ð´ÐµÐµÑ‚ *Ð½Ð¸Ð·Ð¾Ð¼* Ð»ÐµÑÑ‚Ð½Ð¸Ñ†Ñ‹ Ñ‚Ñ€Ð°Ð½ÑÐ»ÑÑ†Ð¸Ð¸ â€” ÑÑ€Ð¾Ð¹ Ð´Ð¾ Ñ„Ð»ÑÑ‚Ñ‚ÐµÐ½Ð¸Ð½Ð³Ð°.
//! Ð—Ð´ÐµÑÑŒ Ð¾Ð´Ð¸Ð½ ÑˆÐ°Ð³ (1.8 -> 1.9, Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» 47 -> 107) Ð¸ Ð´Ð²Ð° Ð¿Ð°ÐºÐµÑ‚Ð° (Join Game
//! 0x01, Chunk Data 0x21, Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» 47); Ð¾ÑÑ‚Ð°Ð»ÑŒÐ½Ð¾Ðµ â€” passthrough.
//! Ð£ 1.8 Chunk Data Ð½ÐµÑ‚ Ð¿Ð¾Ð»Ñ Ð´Ð»Ð¸Ð½Ñ‹ (Ð¾Ð½Ð¾ Ð±Ñ‹Ð»Ð¾ Ð² 1.7 Ð¸ Ð²ÐµÑ€Ð½Ñ‘Ñ‚ÑÑ Ð² 1.13):
//! Ð´Ð»Ð¸Ð½Ð° Ð´Ð°Ð½Ð½Ñ‹Ñ… Ð¿Ñ€Ð¾Ð²ÐµÑ€ÑÐµÑ‚ÑÑ Ð¿ÐµÑ€ÐµÑÑ‡Ñ‘Ñ‚Ð¾Ð¼ Ð¿Ð¾ Ð±Ð¸Ñ‚Ð¾Ð²Ð¾Ð¹ Ð¼Ð°ÑÐºÐµ ÑÐµÐºÑ†Ð¸Ð¹.
//!
//! FIXME(wiring): the 26.3 output codec (headers, registry-mapped state
//! ids, palette re-encode) arrives with `horizon-protocol` wiring. Until
//! then [`translate_clientbound`] validates known packets and passes them
//! through byte-for-byte.
//!
//! RU: FIXME(wiring) â€” ÐºÐ¾Ð´ÐµÐº Ð²Ñ‹Ð²Ð¾Ð´Ð° Ð² 26.3 Ð¿Ñ€Ð¸Ð´Ñ‘Ñ‚ Ð²Ð¼ÐµÑÑ‚Ðµ Ñ wiring;
//! Ð¿Ð¾ÐºÐ° translate_clientbound Ð²Ð°Ð»Ð¸Ð´Ð¸Ñ€ÑƒÐµÑ‚ Ð¸ Ð¾Ñ‚Ð´Ð°Ñ‘Ñ‚ Ð¿Ð°ÐºÐµÑ‚Ñ‹ ÐºÐ°Ðº ÐµÑÑ‚ÑŒ.

use crate::errors::ViaError;
use crate::frame::read_varint;
use crate::nbt_compat::decode_modified_utf8;
use crate::Direction;
use crate::state::ProtocolState;
use crate::Step;

/// Join Game packet id at protocol 47 (1.8, clientbound, state PLAY).
///
/// EN: stable across the whole 1.8.x line (47). At 1.9 (107) it is still
/// 0x01; it starts moving afterwards (0x23 at 1.12, 0x25 at 1.13 ...).
///
/// RU: id Ð¿Ð°ÐºÐµÑ‚Ð° Join Game Ð½Ð° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ðµ 47 (Ð²ÑÑ Ð»Ð¸Ð½ÐµÐ¹ÐºÐ° 1.8.x).
pub const JOIN_GAME_ID_1_8: u32 = 0x01;

/// Chunk Data packet id at protocol 47 (1.8, clientbound, state PLAY).
///
/// EN: stable across 1.8.x. At 1.9 (107) it becomes 0x20; 1.13 uses 0x22.
///
/// RU: id Ð¿Ð°ÐºÐµÑ‚Ð° Chunk Data Ð½Ð° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ðµ 47.
pub const CHUNK_DATA_ID_1_8: u32 = 0x21;

/// Protocol number of 1.8 â€” the only wire format parsed field-by-field here.
/// RU: Ð½Ð¾Ð¼ÐµÑ€ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 1.8 â€” ÐµÐ´Ð¸Ð½ÑÑ‚Ð²ÐµÐ½Ð½Ñ‹Ð¹, Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÐ¼Ñ‹Ð¹ Ð·Ð´ÐµÑÑŒ Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼.
pub const PROTOCOL_1_8: i32 = 47;

/// Protocol number of 1.9 â€” the target of the base step.
/// RU: Ð½Ð¾Ð¼ÐµÑ€ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 1.9 â€” Ñ†ÐµÐ»ÑŒ Ð±Ð°Ð·Ð¾Ð²Ð¾Ð³Ð¾ ÑˆÐ°Ð³Ð°.
pub const PROTOCOL_1_9: i32 = 107;

/// Blocks in one vertical chunk section (16x16x16).
/// RU: Ð±Ð»Ð¾ÐºÐ¾Ð² Ð² Ð¾Ð´Ð½Ð¾Ð¹ Ð²ÐµÑ€Ñ‚Ð¸ÐºÐ°Ð»ÑŒÐ½Ð¾Ð¹ ÑÐµÐºÑ†Ð¸Ð¸ Ñ‡Ð°Ð½ÐºÐ° (16x16x16).
pub const BLOCKS_PER_SECTION: usize = 4096;

/// Bytes per block in a 1.8 section: `id << 4 | meta` stored as u16
/// (big-endian on the wire), two bytes per block.
/// RU: Ð±Ð°Ð¹Ñ‚Ð¾Ð² Ð½Ð° Ð±Ð»Ð¾Ðº Ð² ÑÐµÐºÑ†Ð¸Ð¸ 1.8: id<<4|meta Ð² Ð²Ð¸Ð´Ðµ u16.
pub const BLOCK_BYTES_18: usize = 2;

/// Bytes of one nibble-array: block light or sky light, 4096 nibbles.
/// RU: Ð±Ð°Ð¹Ñ‚Ð¾Ð² Ð¾Ð´Ð½Ð¾Ð³Ð¾ nibble-Ð¼Ð°ÑÑÐ¸Ð²Ð° Ð¾ÑÐ²ÐµÑ‰ÐµÐ½Ð¸Ñ (block/sky light).
pub const LIGHT_ARRAY_BYTES: usize = 2048;

/// Bytes of the flat biome array (256 columns, one byte each), appended to
/// a 1.8 chunk's data when `ground_up` is set.
/// RU: Ð±Ð°Ð¹Ñ‚Ð¾Ð² Ð¿Ð»Ð¾ÑÐºÐ¾Ð³Ð¾ Ð¼Ð°ÑÑÐ¸Ð²Ð° Ð±Ð¸Ð¾Ð¼Ð¾Ð² (256 ÐºÐ¾Ð»Ð¾Ð½Ð½), Ñ…Ð²Ð¾ÑÑ‚ full-chunk.
pub const BIOME_ARRAY_BYTES: usize = 256;

/// One PLAY-state translation step in the "ancient" family.
///
/// EN: fields `from`/`to` mirror the [`Step::FROM`]/[`Step::TO`] constants
/// of the only constructor this skeleton exposes
/// ([`PlayAncientStep::base_1_8_to_1_9`]). They exist as *fields* so a later
/// generalized step (any 47..340 pair) can reuse the same struct without a
/// breaking change; until that day the invariant
/// `from == FROM && to == TO` holds for every instance, because the fields
/// are private and the base constructor is the only way in.
///
/// RU: Ð¿Ð¾Ð»Ñ Ð¿Ð¾Ð²Ñ‚Ð¾Ñ€ÑÑŽÑ‚ ÐºÐ¾Ð½ÑÑ‚Ð°Ð½Ñ‚Ñ‹ ÑˆÐ°Ð³Ð°; Ð¾Ð½Ð¸ Ð¿Ð¾Ð»Ñ â€” Ñ‡Ñ‚Ð¾Ð±Ñ‹ Ð±ÑƒÐ´ÑƒÑ‰Ð¸Ð¹ Ð¾Ð±Ð¾Ð±Ñ‰Ñ‘Ð½Ð½Ñ‹Ð¹
/// ÑˆÐ°Ð³ (Ð»ÑŽÐ±Ð°Ñ Ð¿Ð°Ñ€Ð° Ð¸Ð· 47..340) Ð¿ÐµÑ€ÐµÐ¸ÑÐ¿Ð¾Ð»ÑŒÐ·Ð¾Ð²Ð°Ð» Ñ‚Ð¾Ñ‚ Ð¶Ðµ Ñ‚Ð¸Ð¿ Ð±ÐµÐ· Ð»Ð¾Ð¼ÐºÐ¸ API.
pub struct PlayAncientStep {
    /// Source protocol of this step instance (equals [`Step::FROM`]).
    /// RU: Ð¸ÑÑ…Ð¾Ð´Ð½Ñ‹Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð° ÑˆÐ°Ð³Ð°.
    from: i32,
    /// Target protocol of this step instance (equals [`Step::TO`]).
    /// RU: Ñ†ÐµÐ»ÐµÐ²Ð¾Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð° ÑˆÐ°Ð³Ð°.
    to: i32,
}

impl PlayAncientStep {
    /// The base 1.8 -> 1.9 step (protocol 47 -> 107).
    ///
    /// EN: the only public constructor of this skeleton. Its packets are
    /// parsed at 47; nothing is re-encoded yet (see the module doc â€” the
    /// translation *into* 26.3 shapes arrives with the wiring).
    ///
    /// RU: Ð±Ð°Ð·Ð¾Ð²Ñ‹Ð¹ ÑˆÐ°Ð³ 1.8 -> 1.9; ÐµÐ´Ð¸Ð½ÑÑ‚Ð²ÐµÐ½Ð½Ñ‹Ð¹ ÐºÐ¾Ð½ÑÑ‚Ñ€ÑƒÐºÑ‚Ð¾Ñ€ ÑÐºÐµÐ»ÐµÑ‚Ð°.
    pub fn base_1_8_to_1_9() -> Self {
        PlayAncientStep {
            from: PROTOCOL_1_8,
            to: PROTOCOL_1_9,
        }
    }

    /// Source protocol of this instance.
    /// RU: Ð¸ÑÑ…Ð¾Ð´Ð½Ñ‹Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÑ‚Ð¾Ð³Ð¾ ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð°.
    pub fn from_protocol(&self) -> i32 {
        self.from
    }

    /// Target protocol of this instance.
    /// RU: Ñ†ÐµÐ»ÐµÐ²Ð¾Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÑ‚Ð¾Ð³Ð¾ ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð°.
    pub fn to_protocol(&self) -> i32 {
        self.to
    }

    /// Parse one clientbound PLAY packet at this step's source version.
    ///
    /// EN: dispatches by packet id **only for protocol 47**:
    /// - [`JOIN_GAME_ID_1_8`] -> full field parse into [`ParsedJoin18`];
    /// - [`CHUNK_DATA_ID_1_8`] -> field parse into [`ParsedChunk18`];
    /// - anything else (and any packet at a source version other than 47)
    ///   -> [`Translated18::Other`] passthrough.
    ///
    /// A known id whose body fails to parse is a
    /// [`ViaError::Malformed`](crate::errors::ViaError::Malformed)
    /// (non-fatal: the caller drops the packet); an unknown id is *not* an
    /// error at this stage of the skeleton â€” it passes through.
    ///
    /// FIXME(wiring): for every source version besides 47 the answer is
    /// always `Other`, because per-version id tables are not wired yet
    /// (47..340 spans nine distinct wire formats).
    ///
    /// RU: Ñ€Ð°Ð·Ð±Ð¾Ñ€ clientbound PLAY-Ð¿Ð°ÐºÐµÑ‚Ð°; Ð³Ð»ÑƒÐ±Ð¾ÐºÐ¸Ð¹ Ð¿Ð°Ñ€ÑÐ¸Ð½Ð³ Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Ð´Ð»Ñ 47,
    /// Ð¾ÑÑ‚Ð°Ð»ÑŒÐ½Ð¾Ðµ â€” passthrough.
    pub fn parse_clientbound(&self, id: u32, body: &[u8]) -> Result<Translated18, ViaError> {
        if self.from != PROTOCOL_1_8 {
            // FIXME(wiring): per-version id tables + layouts for 107..340.
            // RU: Ñ‚Ð°Ð±Ð»Ð¸Ñ†Ñ‹ id Ð¸ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐ¸ Ð´Ð»Ñ 107..340 Ð¿Ñ€Ð¸Ð´ÑƒÑ‚ Ñ wiring.
            return Ok(Translated18::Other(id, body.to_vec()));
        }
        parse_clientbound_18(id, body)
    }
}

impl Step for PlayAncientStep {
    const FROM: i32 = PROTOCOL_1_8;
    const TO: i32 = PROTOCOL_1_9;

    fn name(&self) -> &'static str {
        "play ancient 1.8 -> 1.9 (join/chunk skeleton)"
    }
}

/// Session-level parameters every free translation function receives.
///
/// EN: the (future) pipeline owns one `TranslateParams` per connection and
/// hands it to whichever step handles the packet, so steps stay stateless.
/// This struct is defined *here* because no shared one existed when this
/// module was written; if the pipeline later introduces its own params
/// type, this one should be replaced by a re-export (one-line change, all
/// callers go through the pipeline anyway).
///
/// RU: Ð¿Ð°Ñ€Ð°Ð¼ÐµÑ‚Ñ€Ñ‹ ÑÐµÑÑÐ¸Ð¸, ÐºÐ¾Ñ‚Ð¾Ñ€Ñ‹Ðµ Ð¿Ð°Ð¹Ð¿Ð»Ð°Ð¹Ð½ Ð¿ÐµÑ€ÐµÐ´Ð°Ñ‘Ñ‚ ÑˆÐ°Ð³Ð°Ð¼. ÐžÐ¿Ñ€ÐµÐ´ÐµÐ»ÐµÐ½Ñ‹ Ð·Ð´ÐµÑÑŒ,
/// Ð¿Ð¾Ñ‚Ð¾Ð¼Ñƒ Ñ‡Ñ‚Ð¾ Ð¾Ð±Ñ‰ÐµÐ³Ð¾ Ñ‚Ð¸Ð¿Ð° Ð½Ðµ Ð±Ñ‹Ð»Ð¾; Ð¿Ñ€Ð¸ Ð¿Ð¾ÑÐ²Ð»ÐµÐ½Ð¸Ð¸ â€” Ð·Ð°Ð¼ÐµÐ½Ð¸Ñ‚ÑŒ Ð½Ð° re-export.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TranslateParams {
    /// Which way the packet is flowing. `translate_clientbound` only
    /// field-parses [`Direction::Clientbound`] packets.
    /// RU: Ð½Ð°Ð¿Ñ€Ð°Ð²Ð»ÐµÐ½Ð¸Ðµ Ð¿Ð°ÐºÐµÑ‚Ð°.
    pub direction: Direction,
    /// Connection state the packet arrived in; only `Play` is parsed here.
    /// RU: ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ ÑÐ¾ÐµÐ´Ð¸Ð½ÐµÐ½Ð¸Ñ; Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÐ¼ Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Play.
    pub state: ProtocolState,
    /// Protocol the packet was read at (the chain's current bottom).
    /// RU: Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð», Ð½Ð° ÐºÐ¾Ñ‚Ð¾Ñ€Ð¾Ð¼ Ð¿Ñ€Ð¾Ñ‡Ð¸Ñ‚Ð°Ð½ Ð¿Ð°ÐºÐµÑ‚.
    pub source_protocol: i32,
}

impl TranslateParams {
    /// Params for a clientbound packet in the PLAY state at `protocol`.
    ///
    /// RU: Ð¿Ð°Ñ€Ð°Ð¼ÐµÑ‚Ñ€Ñ‹ Ð´Ð»Ñ clientbound-Ð¿Ð°ÐºÐµÑ‚Ð° Ð² ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ð¸ Play.
    pub fn clientbound_play(source_protocol: i32) -> Self {
        TranslateParams {
            direction: Direction::Clientbound,
            state: ProtocolState::Play,
            source_protocol,
        }
    }

    /// Params for the opposite direction, keeping state and protocol.
    ///
    /// RU: Ð¿Ð°Ñ€Ð°Ð¼ÐµÑ‚Ñ€Ñ‹ Ð¾Ð±Ñ€Ð°Ñ‚Ð½Ð¾Ð³Ð¾ Ð½Ð°Ð¿Ñ€Ð°Ð²Ð»ÐµÐ½Ð¸Ñ, ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ðµ Ð¸ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» Ñ‚Ðµ Ð¶Ðµ.
    pub fn flipped(self) -> Self {
        TranslateParams {
            direction: self.direction.flip(),
            ..self
        }
    }
}

/// Main entry: translate one clientbound packet of this step's era.
///
/// EN: currently validates and passes through. For protocol 47, PLAY-state,
/// clientbound packets with id [`JOIN_GAME_ID_1_8`] or
/// [`CHUNK_DATA_ID_1_8`] the body is fully field-parsed (a bad body is
/// [`ViaError::Malformed`](crate::errors::ViaError::Malformed) and the
/// packet is dropped by the caller); every other combination â€” other ids,
/// other protocols, serverbound direction, non-PLAY states â€” rides along
/// untouched. The returned vector is the packets to hand to the next step
/// (or the client); today it is always exactly one passthrough entry.
///
/// FIXME(wiring): the real 26.3 output â€” rewritten ids, join reshaped into
/// the 26.3 layout, chunk sections re-encoded from `id<<4|meta` into
/// registry-mapped block states through
/// [`registry_map`](crate::registry_map) â€” arrives with the
/// `horizon-protocol` wiring. The chunk converter will consume
/// [`ParsedChunk18`] as its input type.
///
/// RU: Ð³Ð»Ð°Ð²Ð½Ñ‹Ð¹ Ð²Ñ…Ð¾Ð´: ÑÐµÐ¹Ñ‡Ð°Ñ Ð²Ð°Ð»Ð¸Ð´Ð¸Ñ€ÑƒÐµÑ‚ Ð¸ Ð¾Ñ‚Ð´Ð°Ñ‘Ñ‚ Ð¿Ð°ÐºÐµÑ‚ ÐºÐ°Ðº ÐµÑÑ‚ÑŒ. Ð˜Ð·Ð²ÐµÑÑ‚Ð½Ñ‹Ðµ
/// Ð¿Ð°ÐºÐµÑ‚Ñ‹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47 Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÑŽÑ‚ÑÑ Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼ (ÐºÑ€Ð¸Ð²Ð¾Ðµ Ñ‚ÐµÐ»Ð¾ â€” Malformed, Ð¿Ð°ÐºÐµÑ‚
/// Ð´Ñ€Ð¾Ð¿Ð°ÐµÑ‚ÑÑ); Ð¿Ñ€Ð¾Ñ‡ÐµÐµ Ð¿Ñ€Ð¾Ñ…Ð¾Ð´Ð¸Ñ‚ Ð½ÐµÑ‚Ñ€Ð¾Ð½ÑƒÑ‚Ñ‹Ð¼. ÐŸÐ¾Ð»Ð½Ñ‹Ð¹ ÐºÐ¾Ð´ÐµÐº Ð²Ñ‹Ð²Ð¾Ð´Ð° Ð² 26.3 â€”
/// FIXME(wiring); ÐºÐ¾Ð½Ð²ÐµÑ€Ñ‚ÐµÑ€ Ñ‡Ð°Ð½ÐºÐ¾Ð² Ð±ÑƒÐ´ÐµÑ‚ ÐµÑÑ‚ÑŒ `ParsedChunk18` Ð½Ð° Ð²Ñ…Ð¾Ð´Ðµ.
pub fn translate_clientbound(
    id: u32,
    body: &[u8],
    params: &TranslateParams,
) -> Result<Vec<(u32, Vec<u8>)>, ViaError> {
    if params.direction != Direction::Clientbound
        || params.state != ProtocolState::Play
        || params.source_protocol != PROTOCOL_1_8
    {
        // Not ours to field-parse: a clientbound translator never touches
        // serverbound bytes, non-PLAY states, or foreign protocols.
        // RU: Ð½Ðµ Ð½Ð°Ñˆ ÑÐ»ÑƒÑ‡Ð°Ð¹: serverbound, Ð½Ðµ-Play Ð¸Ð»Ð¸ Ñ‡ÑƒÐ¶Ð¾Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð».
        return Ok(vec![(id, body.to_vec())]);
    }
    // Parse (and thereby validate) the two packets we understand; the
    // parsed fields are consumed by the wiring-day codecs, so the result
    // is intentionally dropped here.
    // RU: Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÐ¼ (Ð¸ Ð²Ð°Ð»Ð¸Ð´Ð¸Ñ€ÑƒÐµÐ¼) Ð´Ð²Ð° Ð¸Ð·Ð²ÐµÑÑ‚Ð½Ñ‹Ñ… Ð¿Ð°ÐºÐµÑ‚Ð°; Ð¿Ð¾Ð»Ñ Ð½ÑƒÐ¶Ð½Ñ‹
    // ÐºÐ¾Ð´ÐµÐºÐ°Ð¼ wiring-Ð´Ð½Ñ, Ð·Ð´ÐµÑÑŒ Ñ€ÐµÐ·ÑƒÐ»ÑŒÑ‚Ð°Ñ‚ ÑÐ¾Ð·Ð½Ð°Ñ‚ÐµÐ»ÑŒÐ½Ð¾ Ð¾Ñ‚Ð±Ñ€Ð°ÑÑ‹Ð²Ð°ÐµÑ‚ÑÑ.
    let _ = parse_clientbound_18(id, body)?;
    Ok(vec![(id, body.to_vec())])
}

/// The outcome of parsing one PLAY packet in this step.
///
/// RU: Ñ€ÐµÐ·ÑƒÐ»ÑŒÑ‚Ð°Ñ‚ Ñ€Ð°Ð·Ð±Ð¾Ñ€Ð° Ð¾Ð´Ð½Ð¾Ð³Ð¾ PLAY-Ð¿Ð°ÐºÐµÑ‚Ð° ÑˆÐ°Ð³Ð°.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translated18 {
    /// A parsed Join Game (protocol 47 layout).
    /// RU: Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Join Game (Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐ° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47).
    Join(ParsedJoin18),
    /// A parsed Chunk Data (protocol 47 layout).
    /// RU: Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Chunk Data (Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐ° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47).
    Chunk(ParsedChunk18),
    /// A packet this skeleton does not touch: its wire id and raw body,
    /// byte-for-byte as they arrived.
    /// RU: Ð½ÐµÑ‚Ñ€Ð¾Ð½ÑƒÑ‚Ñ‹Ð¹ Ð¿Ð°ÐºÐµÑ‚: id Ð¸ ÑÑ‹Ñ€Ð¾Ðµ Ñ‚ÐµÐ»Ð¾ ÐºÐ°Ðº ÐµÑÑ‚ÑŒ.
    Other(u32, Vec<u8>),
}

/// Join Game (clientbound) parsed at protocol 47 â€” 1.8 only.
///
/// EN: field order on the wire at 47:
/// `Int entity-id, UByte gamemode, Byte dimension, UByte difficulty,
/// UByte max-players, String level-type`.
///
/// Note `dimension` is a *signed* byte at 1.8 (nether is -1, end is 1) â€”
/// parsing it unsigned would turn the nether into 255. The format mutates
/// right after 1.9 (difficulty moves to a separate packet in 1.15-ish,
/// dimensions become identifiers in 1.16), which is why this struct is
/// 47-shaped and nothing else.
///
/// RU: Join Game, Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47 (Ñ‚Ð¾Ð»ÑŒÐºÐ¾ 1.8):
/// entityId, gamemode, dimension (Ð·Ð½Ð°ÐºÐ¾Ð²Ñ‹Ð¹ Ð±Ð°Ð¹Ñ‚: Ð½ÐµÐ·ÐµÑ€ = -1), difficulty,
/// maxPlayers, levelType.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedJoin18 {
    /// The player's own entity id.
    /// RU: id ÑÑƒÑ‰Ð½Ð¾ÑÑ‚Ð¸ Ð¸Ð³Ñ€Ð¾ÐºÐ°.
    pub entity_id: i32,
    /// Client-side gamemode (0 = survival, 1 = creative, 2 = adventure,
    /// 3 = spectator).
    /// RU: Ð¸Ð³Ñ€Ð¾Ð²Ð¾Ð¹ Ñ€ÐµÐ¶Ð¸Ð¼.
    pub gamemode: u8,
    /// Dimension id as sent at 47 (signed: -1 nether, 0 overworld, 1 end).
    /// RU: id Ð¸Ð·Ð¼ÐµÑ€ÐµÐ½Ð¸Ñ (Ð·Ð½Ð°ÐºÐ¾Ð²Ñ‹Ð¹: -1 Ð½ÐµÐ·ÐµÑ€, 0 Ð¾Ð±Ñ‹Ñ‡Ð½Ñ‹Ð¹, 1 ÑÐ½Ð´).
    pub dimension: i8,
    /// Difficulty byte (0 peaceful .. 3 hard). Left the packet family later.
    /// RU: ÑÐ»Ð¾Ð¶Ð½Ð¾ÑÑ‚ÑŒ; Ð¿Ð¾Ð·Ð¶Ðµ ÑƒÑˆÐ»Ð° Ð¸Ð· ÑÑ‚Ð¾Ð³Ð¾ Ð¿Ð°ÐºÐµÑ‚Ð°.
    pub difficulty: u8,
    /// Max players â€” client-side cap for the player list, not a server truth.
    /// RU: Ð¼Ð°ÐºÑÐ¸Ð¼ÑƒÐ¼ Ð¸Ð³Ñ€Ð¾ÐºÐ¾Ð² (ÐºÐ»Ð¸ÐµÐ½Ñ‚ÑÐºÐ¸Ð¹ Ð»Ð¸Ð¼Ð¸Ñ‚ ÑÐ¿Ð¸ÑÐºÐ°).
    pub max_players: u8,
    /// Level type string, e.g. `"default"`, `"flat"`.
    /// RU: ÑÑ‚Ñ€Ð¾ÐºÐ° Ñ‚Ð¸Ð¿Ð° Ð¼Ð¸Ñ€Ð°.
    pub level_type: String,
}

/// Chunk Data (clientbound) parsed at protocol 47 â€” 1.8 only.
///
/// EN: field order on the wire at 47:
/// `Int chunk-x, Int chunk-z, Boolean ground-up, UShort primary-bitmask,
/// Byte[..] data`.
///
/// There is **no** data length on the 1.8 wire (see the module doc): `data`
/// is everything to the end of the body. Its length is validated against
/// the bitmask-derived expectation â€” sky-light and no-sky-light variants
/// are both accepted (dimension-dependent), plus the trailing 256-byte
/// biome array when `ground_up` is set. The stored `data` keeps the biome
/// tail raw (the wiring-day converter needs it) and keeps each section's
/// bytes raw: `4096 * u16(id<<4|meta)` + `2048 block-light` + optionally
/// `2048 sky-light`. Converting `id<<4|meta` into 26.3 block states via
/// [`registry_map`](crate::registry_map) is explicitly *not* this module's
/// job.
///
/// RU: Chunk Data Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ 47: chunkX, chunkZ, groundUp, primaryBitmask
/// (u16 BE), data â€” Ð²ÑÐµ Ð±Ð°Ð¹Ñ‚Ñ‹ Ð´Ð¾ ÐºÐ¾Ð½Ñ†Ð° (Ð¿Ð¾Ð»Ñ Ð´Ð»Ð¸Ð½Ñ‹ Ð½ÐµÑ‚). Ð”Ð»Ð¸Ð½Ð° data
/// Ð¿Ñ€Ð¾Ð²ÐµÑ€ÑÐµÑ‚ÑÑ Ð¿Ð¾ Ð¼Ð°ÑÐºÐµ: Ð²Ð°Ñ€Ð¸Ð°Ð½Ñ‚Ñ‹ ÑÐ¾ sky light Ð¸ Ð±ÐµÐ· (Ð·Ð°Ð²Ð¸ÑÐ¸Ñ‚ Ð¾Ñ‚ Ð¸Ð·Ð¼ÐµÑ€ÐµÐ½Ð¸Ñ)
/// Ð¿Ð»ÑŽÑ 256 Ð±Ð°Ð¹Ñ‚ Ð±Ð¸Ð¾Ð¼Ð¾Ð² Ð¿Ñ€Ð¸ groundUp. Ð¡ÐµÐºÑ†Ð¸Ð¸ Ñ…Ñ€Ð°Ð½ÑÑ‚ÑÑ ÑÑ‹Ñ€Ñ‹Ð¼Ð¸; ÐºÐ¾Ð½Ð²ÐµÑ€Ñ‚Ð°Ñ†Ð¸Ñ
/// id+meta Ð² ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ñ â€” Ñ€Ð°Ð±Ð¾Ñ‚Ð° wiring-Ð´Ð½Ñ, Ð½Ðµ ÑÑ‚Ð¾Ð³Ð¾ Ð¼Ð¾Ð´ÑƒÐ»Ñ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedChunk18 {
    /// Chunk X coordinate (chunk units, not blocks).
    /// RU: ÐºÐ¾Ð¾Ñ€Ð´Ð¸Ð½Ð°Ñ‚Ð° X Ñ‡Ð°Ð½ÐºÐ°.
    pub x: i32,
    /// Chunk Z coordinate (chunk units, not blocks).
    /// RU: ÐºÐ¾Ð¾Ñ€Ð´Ð¸Ð½Ð°Ñ‚Ð° Z Ñ‡Ð°Ð½ÐºÐ°.
    pub z: i32,
    /// Whether this is a full chunk (replaces the old one; carries biomes).
    /// RU: Ð¿Ð¾Ð»Ð½Ñ‹Ð¹ Ð»Ð¸ Ñ‡Ð°Ð½Ðº (Ñ Ð±Ð¸Ð¾Ð¼Ð°Ð¼Ð¸, Ð·Ð°Ð¼ÐµÐ½ÑÐµÑ‚ Ð¿Ñ€ÐµÐ¶Ð½Ð¸Ð¹).
    pub ground_up: bool,
    /// Bitmask of present vertical sections (bit i = section i, 16 in
    /// total at 1.8). Parsed as u16, so only 16 bits can ever be set.
    /// RU: Ð±Ð¸Ñ‚Ð¾Ð²Ð°Ñ Ð¼Ð°ÑÐºÐ° ÑÐµÐºÑ†Ð¸Ð¹ Ñ‡Ð°Ð½ÐºÐ° (u16, Ð¼Ð°ÐºÑÐ¸Ð¼ÑƒÐ¼ 16 Ð±Ð¸Ñ‚).
    pub primary_bitmask: u16,
    /// Raw section payload to the end of the body: per present section
    /// `8192` block bytes + `2048` block light + optionally `2048` sky
    /// light, plus the raw `256`-byte biome tail when `ground_up`.
    /// RU: ÑÑ‹Ñ€Ñ‹Ðµ ÑÐµÐºÑ†Ð¸Ð¸ Ð´Ð¾ ÐºÐ¾Ð½Ñ†Ð° Ñ‚ÐµÐ»Ð° + ÑÑ‹Ñ€Ð¾Ð¹ Ñ…Ð²Ð¾ÑÑ‚ Ð±Ð¸Ð¾Ð¼Ð¾Ð² Ð¿Ñ€Ð¸ ground_up.
    pub data: Vec<u8>,
}

/// Block-light bytes in a 1.8 chunk with `sections` present sections.
///
/// EN: one nibble-array per present section: `4096` nibbles = `2048` bytes.
/// A helper for callers doing their own chunk-length arithmetic (the
/// parser itself uses [`expected_chunk18_data_len`]).
///
/// RU: Ð±Ð°Ð¹Ñ‚Ð¾Ð² block-light Ð² Ñ‡Ð°Ð½ÐºÐµ 1.8 Ñ Ð´Ð°Ð½Ð½Ñ‹Ð¼ Ñ‡Ð¸ÑÐ»Ð¾Ð¼ ÑÐµÐºÑ†Ð¸Ð¹ (Ð¿Ð¾ 2048).
pub fn block_light_size18(sections: u16) -> usize {
    LIGHT_ARRAY_BYTES * sections as usize
}

/// Expected byte length of a 1.8 chunk's `data` field for a given bitmask.
///
/// EN: per present section (`popcount(bitmask)` of them):
/// - `4096 * 2` bytes of `id<<4|meta` block data (see
///   [`BLOCK_BYTES_18`]);
/// - `2048` bytes of block light (always);
/// - `2048` bytes of sky light **iff** `sky_light` is true (overworld
///   chunks carry it; nether/end chunks do not).
///
/// So `12288` per section with sky light, `10240` without. The trailing
/// `256`-byte biome array of ground-up chunks is deliberately **not**
/// included â€” add [`BIOME_ARRAY_BYTES`] yourself when the chunk is full
/// (kept simple per the skeleton contract).
///
/// RU: Ð¾Ð¶Ð¸Ð´Ð°ÐµÐ¼Ð°Ñ Ð´Ð»Ð¸Ð½Ð° data Ñ‡Ð°Ð½ÐºÐ° 1.8 Ð¿Ð¾ Ð¼Ð°ÑÐºÐµ: 12288 Ð½Ð° ÑÐµÐºÑ†Ð¸ÑŽ ÑÐ¾ sky
/// light, 10240 Ð±ÐµÐ·; Ð±Ð¸Ð¾Ð¼Ñ‹ (256) ÑÐ¾Ð·Ð½Ð°Ñ‚ÐµÐ»ÑŒÐ½Ð¾ Ð½Ðµ Ð²ÐºÐ»ÑŽÑ‡ÐµÐ½Ñ‹ â€” Ð´Ð¾Ð±Ð°Ð²ÑŒÑ‚Ðµ ÑÐ°Ð¼Ð¸.
pub fn expected_chunk18_data_len(bitmask: u16, sky_light: bool) -> usize {
    let per_section = BLOCKS_PER_SECTION * BLOCK_BYTES_18 + LIGHT_ARRAY_BYTES
        + if sky_light { LIGHT_ARRAY_BYTES } else { 0 };
    per_section * bitmask.count_ones() as usize
}

/// Dispatch + parse one clientbound PLAY packet at protocol 47.
///
/// RU: Ð´Ð¸ÑÐ¿ÐµÑ‚Ñ‡ÐµÑ€ Ð¸ Ð¿Ð°Ñ€ÑÐµÑ€ clientbound PLAY-Ð¿Ð°ÐºÐµÑ‚Ð° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47.
fn parse_clientbound_18(id: u32, body: &[u8]) -> Result<Translated18, ViaError> {
    match id {
        JOIN_GAME_ID_1_8 => Ok(Translated18::Join(parse_join_18(body)?)),
        CHUNK_DATA_ID_1_8 => Ok(Translated18::Chunk(parse_chunk_18(body)?)),
        // Unknown ids ride along untouched until the id tables arrive.
        // RU: Ð½ÐµÐ¸Ð·Ð²ÐµÑÑ‚Ð½Ñ‹Ðµ id ÐµÐ´ÑƒÑ‚ ÐºÐ°Ðº ÐµÑÑ‚ÑŒ Ð´Ð¾ Ñ‚Ð°Ð±Ð»Ð¸Ñ† id.
        _ => Ok(Translated18::Other(id, body.to_vec())),
    }
}

/// Parses a Join Game body at protocol 47 (1.8). See [`ParsedJoin18`].
///
/// RU: Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Join Game Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 47.
fn parse_join_18(body: &[u8]) -> Result<ParsedJoin18, ViaError> {
    let mut pos = 0usize;

    let entity_id = read_i32(body, &mut pos, "join: entity id")?;
    let gamemode = read_u8(body, &mut pos, "join: gamemode")?;
    let dimension = read_i8(body, &mut pos, "join: dimension")?;
    let difficulty = read_u8(body, &mut pos, "join: difficulty")?;
    let max_players = read_u8(body, &mut pos, "join: max players")?;
    let level_type = read_string(body, &mut pos, "join: level type")?;

    if pos != body.len() {
        return Err(ViaError::Malformed {
            context: "join: trailing bytes after last field".to_string(),
            bytes_seen: pos,
        });
    }

    Ok(ParsedJoin18 {
        entity_id,
        gamemode,
        dimension,
        difficulty,
        max_players,
        level_type,
    })
}

/// Parses a Chunk Data body at protocol 47 (1.8). See [`ParsedChunk18`].
///
/// The data length is validated bitmask-against-remainder (there is no
/// wire length field at 1.8): after optionally removing the 256-byte biome
/// tail, the remainder must equal
/// [`expected_chunk18_data_len`](expected_chunk18_data_len) with sky light
/// or without it. A mask of 0 expects an empty core (body is only biomes,
/// or nothing).
///
/// RU: Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Chunk Data Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ 47; Ð´Ð»Ð¸Ð½Ð° data Ð¿Ñ€Ð¾Ð²ÐµÑ€ÑÐµÑ‚ÑÑ
/// Ð¿ÐµÑ€ÐµÑÑ‡Ñ‘Ñ‚Ð¾Ð¼ Ð¿Ð¾ Ð¼Ð°ÑÐºÐµ (ÑÐ¾ sky light, Ð±ÐµÐ· Ð¸ Ñ ÑƒÑ‡Ñ‘Ñ‚Ð¾Ð¼ Ñ…Ð²Ð¾ÑÑ‚Ð° Ð±Ð¸Ð¾Ð¼Ð¾Ð²).
fn parse_chunk_18(body: &[u8]) -> Result<ParsedChunk18, ViaError> {
    let mut pos = 0usize;

    let x = read_i32(body, &mut pos, "chunk: chunk x")?;
    let z = read_i32(body, &mut pos, "chunk: chunk z")?;
    let ground_up = read_bool(body, &mut pos, "chunk: ground up")?;
    let primary_bitmask = read_u16(body, &mut pos, "chunk: primary bitmask")?;

    // No length field at 1.8: data is the rest of the body, validated below.
    // RU: Ð¿Ð¾Ð»Ñ Ð´Ð»Ð¸Ð½Ñ‹ Ð½ÐµÑ‚: data â€” Ð²ÐµÑÑŒ Ð¾ÑÑ‚Ð°Ñ‚Ð¾Ðº Ñ‚ÐµÐ»Ð°, Ð½Ð¸Ð¶Ðµ Ð¿Ñ€Ð¾Ð²ÐµÑ€ÑÐµÐ¼ ÐµÐ³Ð¾.
    let data = body[pos..].to_vec();
    validate_chunk18_data_len(&data, primary_bitmask, ground_up)?;

    Ok(ParsedChunk18 {
        x,
        z,
        ground_up,
        primary_bitmask,
        data,
    })
}

/// Validates the length of a parsed 1.8 chunk `data` against its bitmask.
///
/// EN: computes the core length (data minus the biome tail if present) and
/// requires it to equal the sky-light or the no-sky-light expectation.
/// FIXME(wiring): strictness may need relaxing if real 1.8 servers are
/// found sending non-standard layouts (e.g. always-sky or never-sky);
/// the acceptance set lives *only* here, in one place, for exactly that day.
///
/// RU: ÑÐ²ÐµÑ€ÑÐµÑ‚ Ð´Ð»Ð¸Ð½Ñƒ data Ñ Ð¼Ð°ÑÐºÐ¾Ð¹: ÑÐ´Ñ€Ð¾ (data Ð±ÐµÐ· Ñ…Ð²Ð¾ÑÑ‚Ð° Ð±Ð¸Ð¾Ð¼Ð¾Ð²) Ð´Ð¾Ð»Ð¶Ð½Ð¾
/// Ñ€Ð°Ð²Ð½ÑÑ‚ÑŒÑÑ Ð¾Ð¶Ð¸Ð´Ð°Ð½Ð¸ÑŽ ÑÐ¾ sky light Ð¸Ð»Ð¸ Ð±ÐµÐ·. FIXME: Ð¿Ñ€Ð¸ Ð½Ð°Ñ…Ð¾Ð´ÐºÐµ Ð½ÐµÑÑ‚Ð°Ð½Ð´Ð°Ñ€Ñ‚Ð½Ñ‹Ñ…
/// ÑÐµÑ€Ð²ÐµÑ€Ð¾Ð² â€” Ð¾ÑÐ»Ð°Ð±Ð¸Ñ‚ÑŒ; Ð½Ð°Ð±Ð¾Ñ€ Ð´Ð¾Ð¿ÑƒÑÑ‚Ð¸Ð¼Ñ‹Ñ… Ð´Ð»Ð¸Ð½ ÑÐ¾Ð±Ñ€Ð°Ð½ Ð·Ð´ÐµÑÑŒ Ð² Ð¾Ð´Ð½Ð¾Ð¼ Ð¼ÐµÑÑ‚Ðµ.
fn validate_chunk18_data_len(
    data: &[u8],
    bitmask: u16,
    ground_up: bool,
) -> Result<(), ViaError> {
    let biome_tail = if ground_up { BIOME_ARRAY_BYTES } else { 0 };
    let core_len = data.len().checked_sub(biome_tail).ok_or_else(|| {
        ViaError::Malformed {
            context: format!(
                "chunk: data length {} shorter than the {}-byte biome tail",
                data.len(), biome_tail
            ),
            bytes_seen: 0,
        }
    })?;
    let with_sky = expected_chunk18_data_len(bitmask, true);
    let without_sky = expected_chunk18_data_len(bitmask, false);
    if core_len != with_sky && core_len != without_sky {
        return Err(ViaError::Malformed {
            context: format!(
                "chunk: data length {} (core {} after biomes) matches neither \
                 sky-light {} nor no-sky-light {} for bitmask {:#06x}",
                data.len(), core_len, with_sky, without_sky, bitmask
            ),
            bytes_seen: 0,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tiny field readers (i32/i8/u8/u16/bool/String), mirroring the play_classic
// convention: each names its context for the Malformed error, per the crate
// contract. Duplicated on purpose â€” steps stay independent of each other
// until the wiring introduces a shared reader module.
// RU: Ð¼Ð¸Ð½Ð¸-Ñ‡Ð¸Ñ‚Ð°Ñ‚ÐµÐ»Ð¸ Ð¿Ð¾Ð»ÐµÐ¹, ÐºÐ°Ðº Ð² play_classic: ÐºÐ¾Ð½Ñ‚ÐµÐºÑÑ‚ Ð² Malformed-Ð¾ÑˆÐ¸Ð±ÐºÐµ.
// Ð”ÑƒÐ±Ð»Ð¸Ñ€ÑƒÑŽÑ‚ÑÑ ÑÐ¾Ð·Ð½Ð°Ñ‚ÐµÐ»ÑŒÐ½Ð¾ â€” Ð´Ð¾ wiring ÑˆÐ°Ð³Ð¸ Ð½Ðµ Ð·Ð°Ð²Ð¸ÑÑÑ‚ Ð´Ñ€ÑƒÐ³ Ð¾Ñ‚ Ð´Ñ€ÑƒÐ³Ð°.
// ---------------------------------------------------------------------------

/// Reads one big-endian i32 at `*pos`.
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ big-endian i32.
fn read_i32(buf: &[u8], pos: &mut usize, context: &str) -> Result<i32, ViaError> {
    if *pos + 4 > buf.len() {
        return Err(ViaError::Malformed {
            context: format!("{} truncated (need 4 bytes)", context),
            bytes_seen: *pos,
        });
    }
    let v = i32::from_be_bytes([
        buf[*pos],
        buf[*pos + 1],
        buf[*pos + 2],
        buf[*pos + 3],
    ]);
    *pos += 4;
    Ok(v)
}

/// Reads one big-endian u16 at `*pos` (e.g. the 1.8 primary bitmask).
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ big-endian u16 (Ð½Ð°Ð¿Ñ€Ð¸Ð¼ÐµÑ€, Ð¼Ð°ÑÐºÑƒ ÑÐµÐºÑ†Ð¸Ð¹ 1.8).
fn read_u16(buf: &[u8], pos: &mut usize, context: &str) -> Result<u16, ViaError> {
    if *pos + 2 > buf.len() {
        return Err(ViaError::Malformed {
            context: format!("{} truncated (need 2 bytes)", context),
            bytes_seen: *pos,
        });
    }
    let v = u16::from_be_bytes([buf[*pos], buf[*pos + 1]]);
    *pos += 2;
    Ok(v)
}

/// Reads one u8 at `*pos`.
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ Ð¾Ð´Ð¸Ð½ Ð±Ð°Ð¹Ñ‚.
fn read_u8(buf: &[u8], pos: &mut usize, context: &str) -> Result<u8, ViaError> {
    if *pos >= buf.len() {
        return Err(ViaError::Malformed {
            context: format!("{} truncated (need 1 byte)", context),
            bytes_seen: *pos,
        });
    }
    let v = buf[*pos];
    *pos += 1;
    Ok(v)
}

/// Reads one signed i8 at `*pos` (the 1.8 dimension byte is signed).
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ Ð·Ð½Ð°ÐºÐ¾Ð²Ñ‹Ð¹ Ð±Ð°Ð¹Ñ‚ (dimension Ð² 1.8 â€” Ð·Ð½Ð°ÐºÐ¾Ð²Ñ‹Ð¹).
fn read_i8(buf: &[u8], pos: &mut usize, context: &str) -> Result<i8, ViaError> {
    let v = read_u8(buf, pos, context)?;
    Ok(v as i8)
}

/// Reads one boolean byte at `*pos` (0 = false, non-zero = true).
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ boolean (0 â€” Ð»Ð¾Ð¶ÑŒ, Ð½Ðµ Ð½Ð¾Ð»ÑŒ â€” Ð¸ÑÑ‚Ð¸Ð½Ð°).
fn read_bool(buf: &[u8], pos: &mut usize, context: &str) -> Result<bool, ViaError> {
    let b = read_u8(buf, pos, context)?;
    Ok(b != 0)
}

/// Reads one protocol String at `*pos`: `VarInt length` + modified-UTF-8
/// bytes (decoded with the crate's modified-UTF-8 decoder, which also
/// accepts plain ASCII/UTF-8).
///
/// RU: Ñ‡Ð¸Ñ‚Ð°ÐµÑ‚ ÑÑ‚Ñ€Ð¾ÐºÑƒ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð°: VarInt-Ð´Ð»Ð¸Ð½Ð° + modified-UTF-8 Ð±Ð°Ð¹Ñ‚Ñ‹.
fn read_string(buf: &[u8], pos: &mut usize, context: &str) -> Result<String, ViaError> {
    let len = read_varint(buf, pos)?;
    if len < 0 {
        return Err(ViaError::Malformed {
            context: format!("{}: negative string length", context),
            bytes_seen: *pos,
        });
    }
    let len = len as usize;
    if *pos + len > buf.len() {
        return Err(ViaError::Malformed {
            context: format!("{}: string body truncated", context),
            bytes_seen: *pos,
        });
    }
    let bytes = &buf[*pos..*pos + len];
    *pos += len;
    decode_modified_utf8(bytes).map_err(|e| match e {
        ViaError::Malformed { bytes_seen, .. } => ViaError::Malformed {
            context: format!("{}: invalid string encoding", context),
            bytes_seen: *pos - len + bytes_seen,
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::write_varint;

    /// Builds a 47 Join Game body from field values.
    /// RU: ÑÐ¾Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Join Game (47) Ð¸Ð· Ð·Ð½Ð°Ñ‡ÐµÐ½Ð¸Ð¹ Ð¿Ð¾Ð»ÐµÐ¹.
    fn join_body(entity_id: i32, gamemode: u8, dimension: i8, difficulty: u8,
                 max_players: u8, level_type: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&entity_id.to_be_bytes());
        out.push(gamemode);
        out.push(dimension as u8);
        out.push(difficulty);
        out.push(max_players);
        write_varint(level_type.len() as i32, &mut out);
        out.extend_from_slice(level_type.as_bytes());
        out
    }

    /// Builds a 47 Chunk Data body: header + payload (sections + optional
    /// biome tail).
    /// RU: ÑÐ¾Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Chunk Data (47): Ð·Ð°Ð³Ð¾Ð»Ð¾Ð²Ð¾Ðº + payload + Ð±Ð¸Ð¾Ð¼Ñ‹.
    fn chunk_body(x: i32, z: i32, ground_up: bool, bitmask: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&x.to_be_bytes());
        out.extend_from_slice(&z.to_be_bytes());
        out.push(u8::from(ground_up));
        out.extend_from_slice(&bitmask.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn step_constants_match_base_constructor() {
        let step = PlayAncientStep::base_1_8_to_1_9();
        assert_eq!(step.from_protocol(), PlayAncientStep::FROM);
        assert_eq!(step.to_protocol(), PlayAncientStep::TO);
        assert_eq!(PlayAncientStep::FROM, 47);
        assert_eq!(PlayAncientStep::TO, 107);
        assert!(step.name().contains("1.8"));
    }

    #[test]
    fn join_parse_golden_bytes() {
        // Golden bytes, hand-assembled:
        // entity 12345 = 0x00003039, gamemode 1 (creative), dimension 0,
        // difficulty 2 (normal), max players 20, level type "default".
        // RU: Ð·Ð¾Ð»Ð¾Ñ‚Ñ‹Ðµ Ð±Ð°Ð¹Ñ‚Ñ‹, ÑÐ¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ðµ Ð²Ñ€ÑƒÑ‡Ð½ÑƒÑŽ.
        let body = [
            0x00, 0x00, 0x30, 0x39, // entity id
            0x01, // gamemode
            0x00, // dimension
            0x02, // difficulty
            0x14, // max players
            0x07, b'd', b'e', b'f', b'a', b'u', b'l', b't',
        ];
        let step = PlayAncientStep::base_1_8_to_1_9();
        let translated = step
            .parse_clientbound(JOIN_GAME_ID_1_8, &body)
            .expect("golden join must parse");
        match translated {
            Translated18::Join(j) => {
                assert_eq!(j.entity_id, 12345);
                assert_eq!(j.gamemode, 1);
                assert_eq!(j.dimension, 0);
                assert_eq!(j.difficulty, 2);
                assert_eq!(j.max_players, 20);
                assert_eq!(j.level_type, "default");
            }
            other => panic!("expected Join, got {:?}", other),
        }
    }

    #[test]
    fn join_parse_negative_dimension() {
        // Nether join: dimension -1 (0xFF as a signed byte). This is exactly
        // why the dimension field is i8, not u8.
        // RU: join Ð² Ð½ÐµÐ·ÐµÑ€: dimension -1 (0xFF ÐºÐ°Ðº Ð·Ð½Ð°ÐºÐ¾Ð²Ñ‹Ð¹ Ð±Ð°Ð¹Ñ‚).
        let body = join_body(-7, 3, -1, 3, 60, "flat");
        let parsed = parse_join_18(&body).expect("nether join must parse");
        assert_eq!(parsed.entity_id, -7);
        assert_eq!(parsed.dimension, -1);
        assert_eq!(parsed.gamemode, 3);
        assert_eq!(parsed.level_type, "flat");
    }

    #[test]
    fn join_truncated_is_malformed() {
        // Full body minus the last 2 bytes (part of the level type string).
        // RU: Ñ‚ÐµÐ»Ð¾ Ð±ÐµÐ· Ð¿Ð¾ÑÐ»ÐµÐ´Ð½Ð¸Ñ… Ð±Ð°Ð¹Ñ‚Ð¾Ð² â€” Malformed, Ð½Ðµ Ð¿Ð°Ð½Ð¸ÐºÐ°.
        let body = join_body(1, 0, 0, 0, 10, "default");
        let cut = &body[..body.len() - 2];
        let err = parse_join_18(cut).expect_err("truncated join must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn join_trailing_bytes_is_malformed() {
        let mut body = join_body(1, 0, 0, 0, 10, "default");
        body.push(0xDE); // one extra byte after levelType
        let err = parse_join_18(&body).expect_err("trailing join must fail");
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. }
                if context.contains("trailing")),
            "{}",
            err
        );
    }

    #[test]
    fn chunk_parse_valid_single_section_with_sky_and_biomes() {
        // One section (mask 0b1), sky light, full chunk with biomes:
        // 12288 section bytes + 256 biome bytes = 12544 payload bytes.
        // RU: Ð¾Ð´Ð½Ð° ÑÐµÐºÑ†Ð¸Ñ, sky light, Ð¿Ð¾Ð»Ð½Ñ‹Ð¹ Ñ‡Ð°Ð½Ðº Ñ Ð±Ð¸Ð¾Ð¼Ð°Ð¼Ð¸: 12288 + 256.
        let payload_len = expected_chunk18_data_len(0b1, true) + BIOME_ARRAY_BYTES;
        assert_eq!(payload_len, 12288 + 256);
        let payload = vec![0x42u8; payload_len];

        let body = chunk_body(5, -3, true, 0b1, &payload);
        let parsed = parse_chunk_18(&body).expect("valid chunk must parse");
        assert_eq!(parsed.x, 5);
        assert_eq!(parsed.z, -3);
        assert!(parsed.ground_up);
        assert_eq!(parsed.primary_bitmask, 0b1);
        assert_eq!(parsed.data.len(), payload_len);
        assert!(parsed.data.iter().all(|&b| b == 0x42));
    }

    #[test]
    fn chunk_parse_two_sections_no_sky_no_biomes() {
        // Mask 0b101 (sections 0 and 2), no sky light (nether), partial
        // chunk: 2 * 10240 = 20480 payload bytes.
        // RU: Ð¼Ð°ÑÐºÐ° 0b101, Ð±ÐµÐ· sky light (Ð½ÐµÐ·ÐµÑ€), Ð½Ðµ Ð¿Ð¾Ð»Ð½Ñ‹Ð¹ Ñ‡Ð°Ð½Ðº: 2*10240.
        let payload_len = expected_chunk18_data_len(0b101, false);
        assert_eq!(payload_len, 20480);
        let payload = vec![0u8; payload_len];

        let body = chunk_body(-100, 2500, false, 0b101, &payload);
        let parsed = parse_chunk_18(&body).expect("nether chunk must parse");
        assert_eq!(parsed.primary_bitmask, 0b101);
        assert!(!parsed.ground_up);
        assert_eq!(parsed.data.len(), 20480);
    }

    #[test]
    fn chunk_parse_empty_mask_full_chunk_biomes_only() {
        // Mask 0 with ground_up: the body is just the 256-byte biome array
        // (a legal "unload the sections, keep the columns" shape).
        // RU: Ð¼Ð°ÑÐºÐ° 0 Ð¸ full chunk: Ñ‚Ð¾Ð»ÑŒÐºÐ¾ 256 Ð±Ð°Ð¹Ñ‚ Ð±Ð¸Ð¾Ð¼Ð¾Ð² â€” Ð»ÐµÐ³Ð°Ð»ÑŒÐ½Ð°Ñ Ñ„Ð¾Ñ€Ð¼Ð°.
        let body = chunk_body(0, 0, true, 0, &[0xAA; BIOME_ARRAY_BYTES]);
        let parsed = parse_chunk_18(&body).expect("biomes-only chunk must parse");
        assert_eq!(parsed.primary_bitmask, 0);
        assert_eq!(parsed.data, vec![0xAA; BIOME_ARRAY_BYTES]);
    }

    #[test]
    fn chunk_data_len_desync_is_malformed() {
        // Mask 0b1 (one section) but only 100 bytes of data: neither the
        // sky-light (12288) nor no-sky-light (10240) expectation.
        // RU: Ð¼Ð°ÑÐºÐ° 0b1, Ð° Ð´Ð°Ð½Ð½Ñ‹Ñ… 100 Ð±Ð°Ð¹Ñ‚ â€” Ñ€Ð°ÑÑÐ¸Ð½Ñ…Ñ€Ð¾Ð½, Malformed.
        let body = chunk_body(0, 0, false, 0b1, &[0u8; 100]);
        let err = parse_chunk_18(&body).expect_err("data length desync must fail");
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. }
                if context.contains("matches neither")),
            "{}",
            err
        );
        assert!(!err.is_fatal(), "a bad packet must not kill the session");
    }

    #[test]
    fn chunk_data_shorter_than_biome_tail_is_malformed() {
        // ground_up with less than 256 bytes total: the biome tail alone
        // does not fit.
        // RU: full chunk ÐºÐ¾Ñ€Ð¾Ñ‡Ðµ 256 Ð±Ð°Ð¹Ñ‚ â€” Ð´Ð°Ð¶Ðµ Ñ…Ð²Ð¾ÑÑ‚ Ð±Ð¸Ð¾Ð¼Ð¾Ð² Ð½Ðµ Ð²Ð»ÐµÐ·Ð°ÐµÑ‚.
        let body = chunk_body(0, 0, true, 0b1, &[0u8; 100]);
        let err = parse_chunk_18(&body).expect_err("short full chunk must fail");
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. }
                if context.contains("biome tail")),
            "{}",
            err
        );
    }

    #[test]
    fn chunk_truncated_header_is_malformed() {
        // Only 3 bytes of the x field: Malformed, no panic.
        // RU: Ð¾Ð±Ñ€Ñ‹Ð² Ð² Ð¿ÐµÑ€Ð²Ð¾Ð¼ Ð¿Ð¾Ð»Ðµ â€” Malformed, Ð±ÐµÐ· Ð¿Ð°Ð½Ð¸ÐºÐ¸.
        let err = parse_chunk_18(&[1, 2, 3]).expect_err("truncated chunk must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn unknown_packet_passthrough() {
        let step = PlayAncientStep::base_1_8_to_1_9();
        let out = step
            .parse_clientbound(0x2A, b"unchanged bytes")
            .expect("passthrough must not fail");
        match out {
            Translated18::Other(id, body) => {
                assert_eq!(id, 0x2A);
                assert_eq!(body, b"unchanged bytes");
            }
            other => panic!("expected Other, got {:?}", other),
        }
    }

    #[test]
    fn join_id_at_other_version_is_passthrough() {
        // A step whose source is not 47 must not field-parse anything,
        // even a join id â€” FIXME(wiring) territory.
        // RU: ÑˆÐ°Ð³ Ñ Ð¸ÑÑ…Ð¾Ð´Ð½Ð¾Ð¹ Ð²ÐµÑ€ÑÐ¸ÐµÐ¹ Ð½Ðµ 47 Ð½Ðµ Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ join Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼.
        let step = PlayAncientStep { from: 107, to: 210 };
        let body = join_body(1, 0, 0, 0, 10, "default");
        let out = step
            .parse_clientbound(JOIN_GAME_ID_1_8, &body)
            .expect("non-47 must pass through");
        assert_eq!(out, Translated18::Other(JOIN_GAME_ID_1_8, body));
    }

    #[test]
    fn translate_clientbound_validates_known_packets() {
        // A known chunk id with a lying body must surface the Malformed
        // error out of the main entry.
        // RU: Ð¸Ð·Ð²ÐµÑÑ‚Ð½Ñ‹Ð¹ chunk id Ñ ÐºÑ€Ð¸Ð²Ñ‹Ð¼ Ñ‚ÐµÐ»Ð¾Ð¼ â€” Malformed Ð¸Ð· Ð³Ð»Ð°Ð²Ð½Ð¾Ð³Ð¾ Ð²Ñ…Ð¾Ð´Ð°.
        let params = TranslateParams::clientbound_play(PROTOCOL_1_8);
        let body = chunk_body(0, 0, false, 0b1, &[0u8; 100]);
        let err = translate_clientbound(CHUNK_DATA_ID_1_8, &body, &params)
            .expect_err("bad chunk must fail through the main entry");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn translate_clientbound_returns_passthrough_output() {
        // Valid known packet: one passthrough entry, byte-for-byte.
        // FIXME(wiring): the 26.3 codec will replace this output.
        // RU: Ð²Ð°Ð»Ð¸Ð´Ð½Ñ‹Ð¹ Ð¿Ð°ÐºÐµÑ‚ â€” Ð¾Ð´Ð¸Ð½ passthrough-ÑÐ»ÐµÐ¼ÐµÐ½Ñ‚, Ð±Ð°Ð¹Ñ‚ Ð² Ð±Ð°Ð¹Ñ‚.
        let params = TranslateParams::clientbound_play(PROTOCOL_1_8);
        let body = join_body(12345, 1, 0, 2, 20, "default");
        let out = translate_clientbound(JOIN_GAME_ID_1_8, &body, &params)
            .expect("valid join must pass");
        assert_eq!(out, vec![(JOIN_GAME_ID_1_8, body)]);

        let unknown = translate_clientbound(0x42, b"mystery", &params)
            .expect("unknown must pass");
        assert_eq!(unknown, vec![(0x42, b"mystery".to_vec())]);
    }

    #[test]
    fn translate_clientbound_foreign_protocol_is_passthrough() {
        // Protocol 110 (1.9.4) through this step: untouched, even with the
        // chunk id â€” the 47 layouts must not be applied to foreign bytes.
        // RU: Ñ‡ÑƒÐ¶Ð¾Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» â€” Ð½Ðµ Ñ‚Ñ€Ð¾Ð³Ð°ÐµÐ¼, Ð´Ð°Ð¶Ðµ Ð¿Ñ€Ð¸ chunk id.
        let params = TranslateParams::clientbound_play(110);
        let body = chunk_body(0, 0, false, 0b1, &[0u8; 100]); // invalid at 47
        let out = translate_clientbound(CHUNK_DATA_ID_1_8, &body, &params)
            .expect("foreign protocol must pass through unvalidated");
        assert_eq!(out, vec![(CHUNK_DATA_ID_1_8, body)]);
    }

    #[test]
    fn translate_clientbound_serverbound_is_passthrough() {
        // Serverbound bytes must never be field-parsed by a clientbound
        // translator, even at protocol 47 in PLAY.
        // RU: serverbound-Ð±Ð°Ð¹Ñ‚Ñ‹ Ð½Ðµ Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÑŽÑ‚ÑÑ clientbound-Ñ‚Ñ€Ð°Ð½ÑÐ»ÑÑ‚Ð¾Ñ€Ð¾Ð¼.
        let params = TranslateParams {
            direction: Direction::Serverbound,
            state: ProtocolState::Play,
            source_protocol: PROTOCOL_1_8,
        };
        let body = chunk_body(0, 0, false, 0b1, &[0u8; 100]); // invalid at 47
        let out = translate_clientbound(CHUNK_DATA_ID_1_8, &body, &params)
            .expect("serverbound must pass through unvalidated");
        assert_eq!(out, vec![(CHUNK_DATA_ID_1_8, body)]);
    }

    #[test]
    fn translate_clientbound_non_play_state_is_passthrough() {
        // A login-state packet with the chunk id: not ours.
        // RU: Ð¿Ð°ÐºÐµÑ‚ ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸Ñ login Ñ chunk id â€” Ð½Ðµ Ð½Ð°Ñˆ ÑÐ»ÑƒÑ‡Ð°Ð¹.
        let params = TranslateParams {
            direction: Direction::Clientbound,
            state: ProtocolState::Login,
            source_protocol: PROTOCOL_1_8,
        };
        let body = chunk_body(0, 0, false, 0b1, &[0u8; 100]);
        let out = translate_clientbound(CHUNK_DATA_ID_1_8, &body, &params)
            .expect("login state must pass through");
        assert_eq!(out, vec![(CHUNK_DATA_ID_1_8, body)]);
    }

    #[test]
    fn translate_params_flipped_roundtrip() {
        let params = TranslateParams::clientbound_play(PROTOCOL_1_8);
        let flipped = params.flipped();
        assert_eq!(flipped.direction, Direction::Serverbound);
        assert_eq!(flipped.state, ProtocolState::Play);
        assert_eq!(flipped.source_protocol, PROTOCOL_1_8);
        assert_eq!(flipped.flipped(), params);
    }

    #[test]
    fn expected_chunk18_data_len_cases() {
        // One section: 8192 blocks + 2048 block light + 2048 sky light.
        // RU: Ð¾Ð´Ð½Ð° ÑÐµÐºÑ†Ð¸Ñ: 8192 + 2048 + 2048.
        assert_eq!(expected_chunk18_data_len(0b1, true), 12288);
        assert_eq!(expected_chunk18_data_len(0b1, false), 10240);
        // Sections 0 and 2: two present sections.
        // RU: ÑÐµÐºÑ†Ð¸Ð¸ 0 Ð¸ 2 â€” Ð´Ð²Ðµ Ð¿Ñ€Ð¸ÑÑƒÑ‚ÑÑ‚Ð²ÑƒÑŽÑ‰Ð¸Ðµ ÑÐµÐºÑ†Ð¸Ð¸.
        assert_eq!(expected_chunk18_data_len(0b101, true), 24576);
        assert_eq!(expected_chunk18_data_len(0b101, false), 20480);
        // Full 1.8 chunk mask: all 16 sections.
        // RU: Ð¼Ð°ÑÐºÐ° Ð¿Ð¾Ð»Ð½Ð¾Ð³Ð¾ Ñ‡Ð°Ð½ÐºÐ° 1.8 â€” Ð²ÑÐµ 16 ÑÐµÐºÑ†Ð¸Ð¹.
        assert_eq!(expected_chunk18_data_len(0xFFFF, true), 16 * 12288);
        assert_eq!(expected_chunk18_data_len(0xFFFF, true), 196_608);
        assert_eq!(expected_chunk18_data_len(0xFFFF, false), 163_840);
        // Empty mask: nothing.
        // RU: Ð¿ÑƒÑÑ‚Ð°Ñ Ð¼Ð°ÑÐºÐ° â€” Ð½Ð¾Ð»ÑŒ.
        assert_eq!(expected_chunk18_data_len(0, true), 0);
        assert_eq!(expected_chunk18_data_len(0, false), 0);
        // Gaps do not count: 0b1000 (section 3) is one section.
        // RU: Ð´Ñ‹Ñ€ÐºÐ¸ Ð½Ðµ ÑÑ‡Ð¸Ñ‚Ð°ÑŽÑ‚ÑÑ: 0b1000 â€” Ð¾Ð´Ð½Ð° ÑÐµÐºÑ†Ð¸Ñ.
        assert_eq!(expected_chunk18_data_len(0b1000, true), 12288);
    }

    #[test]
    fn block_light_size18_cases() {
        assert_eq!(block_light_size18(0), 0);
        assert_eq!(block_light_size18(1), 2048);
        // Block light is dimension-independent: one array per section,
        // sky light is the only variable part.
        // RU: block light Ð½Ðµ Ð·Ð°Ð²Ð¸ÑÐ¸Ñ‚ Ð¾Ñ‚ Ð¸Ð·Ð¼ÐµÑ€ÐµÐ½Ð¸Ñ: Ð¿Ð¾ Ð¼Ð°ÑÑÐ¸Ð²Ñƒ Ð½Ð° ÑÐµÐºÑ†Ð¸ÑŽ.
        assert_eq!(block_light_size18(16), 32_768);
        // Consistency with the expectation helper: expected(sky) minus
        // expected(no-sky) is exactly one light array per section.
        let mask = 0b101;
        let diff = expected_chunk18_data_len(mask, true)
            - expected_chunk18_data_len(mask, false);
        assert_eq!(diff, block_light_size18(2));
    }

    #[test]
    fn full_1_8_chunk_golden_roundtrip() {
        // A full realistic 1.8 overworld chunk: mask 0xFFFF, sky light,
        // ground-up with biomes â€” parses end to end.
        // RU: Ð¿Ð¾Ð»Ð½Ñ‹Ð¹ Ñ€ÐµÐ°Ð»Ð¸ÑÑ‚Ð¸Ñ‡Ð½Ñ‹Ð¹ Ñ‡Ð°Ð½Ðº 1.8 (Ð¾Ð±Ñ‹Ñ‡Ð½Ñ‹Ð¹ Ð¼Ð¸Ñ€): Ð¼Ð°ÑÐºÐ° 0xFFFF,
        // sky light, Ð±Ð¸Ð¾Ð¼Ñ‹ â€” Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ÑÑ Ñ†ÐµÐ»Ð¸ÐºÐ¾Ð¼.
        let payload_len = expected_chunk18_data_len(0xFFFF, true) + BIOME_ARRAY_BYTES;
        assert_eq!(payload_len, 196_608 + 256);
        let payload = vec![0x11u8; payload_len];

        let body = chunk_body(1234, -5678, true, 0xFFFF, &payload);
        let step = PlayAncientStep::base_1_8_to_1_9();
        let out = step
            .parse_clientbound(CHUNK_DATA_ID_1_8, &body)
            .expect("full chunk must parse");
        match out {
            Translated18::Chunk(c) => {
                assert_eq!(c.x, 1234);
                assert_eq!(c.z, -5678);
                assert!(c.ground_up);
                assert_eq!(c.primary_bitmask, 0xFFFF);
                assert_eq!(c.data.len(), payload_len);
            }
            other => panic!("expected Chunk, got {:?}", other),
        }
    }
}
