//! PLAY-state translation for "classic" post-flattening versions (1.13-1.20,
//! protocol 393-767).
//!
//! EN: this module is the skeleton of the biggest step family in the ladder.
//! After the 1.13 flattening, block ids became block *states* and chunks
//! became palette-coded, so 1.13..=1.20.x share a family resemblance that
//! ViaVersion exploits with per-step rewrites. Here we own exactly ONE
//! step deeply (1.13 -> 1.14) and two packets (Join Game, Chunk Data),
//! parsing them at protocol 393 only; everything else passes through
//! untouched as [`TranslatedClassic::Other`] until the per-version id
//! tables arrive with the wiring (see FIXMEs).
//!
//! RU: ÑÐºÐµÐ»ÐµÑ‚ ÑÐ°Ð¼Ð¾Ð³Ð¾ Ð±Ð¾Ð»ÑŒÑˆÐ¾Ð³Ð¾ ÑÐµÐ¼ÐµÐ¹ÑÑ‚Ð²Ð° ÑˆÐ°Ð³Ð¾Ð² Ð»ÐµÑÑ‚Ð½Ð¸Ñ†Ñ‹. ÐŸÐ¾ÑÐ»Ðµ Ñ„Ð»ÑÑ‚Ñ‚ÐµÐ½Ð¸Ð½Ð³Ð°
//! 1.13 Ð±Ð»Ð¾ÐºÐ¸ ÑÑ‚Ð°Ð»Ð¸ ÑÐ¾ÑÑ‚Ð¾ÑÐ½Ð¸ÑÐ¼Ð¸, Ð° Ñ‡Ð°Ð½ÐºÐ¸ â€” Ð¿Ð°Ð»Ð¸Ñ‚Ñ€Ð¾Ð²Ñ‹Ð¼Ð¸; 1.13..=1.20.x
//! Ð¿Ð¾Ñ…Ð¾Ð¶Ð¸ Ð´Ñ€ÑƒÐ³ Ð½Ð° Ð´Ñ€ÑƒÐ³Ð°, Ð¸ ViaVersion Ð¿ÐµÑ€ÐµÐ¿Ð¸ÑÑ‹Ð²Ð°ÐµÑ‚ Ð¸Ñ… Ð¼Ð°Ð»ÐµÐ½ÑŒÐºÐ¸Ð¼Ð¸ ÑˆÐ°Ð³Ð°Ð¼Ð¸.
//! Ð—Ð´ÐµÑÑŒ Ð³Ð»ÑƒÐ±Ð¾ÐºÐ¾ Ð¿Ñ€Ð¾Ñ€Ð°Ð±Ð¾Ñ‚Ð°Ð½ Ð¾Ð´Ð¸Ð½ ÑˆÐ°Ð³ (1.13 -> 1.14) Ð¸ Ð´Ð²Ð° Ð¿Ð°ÐºÐµÑ‚Ð°
//! (Join Game, Chunk Data) â€” Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Ð´Ð»Ñ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 393; Ð¾ÑÑ‚Ð°Ð»ÑŒÐ½Ð¾Ðµ Ð¸Ð´Ñ‘Ñ‚
//! Ð¿Ð°Ñsthrough ÐºÐ°Ðº `Other`, Ð¿Ð¾ÐºÐ° Ñ‚Ð°Ð±Ð»Ð¸Ñ†Ñ‹ id Ð½Ðµ Ð¿Ñ€Ð¸Ð´ÑƒÑ‚ Ñ wiring (ÑÐ¼. FIXME).

use crate::errors::ViaError;
use crate::frame::read_varint;
use crate::nbt_compat::decode_modified_utf8;
use crate::Step;

/// Join Game packet id at protocol 393 (1.13, clientbound, state PLAY).
///
/// FIXME(wiring): the id varies across versions (0x25 in 1.13, 0x25 in 1.14,
/// 0x26 in 1.15 ...). The per-version id table arrives with the wiring day;
/// until then this constant is the only id this module knows.
///
/// RU: id Ð¿Ð°ÐºÐµÑ‚Ð° Join Game Ð½Ð° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ðµ 393; Ð´Ð»Ñ Ð¿Ñ€Ð¾Ñ‡Ð¸Ñ… Ð²ÐµÑ€ÑÐ¸Ð¹ â€” FIXME.
pub const JOIN_GAME_ID_1_13: u32 = 0x25;

/// Chunk Data packet id at protocol 393 (1.13, clientbound, state PLAY).
///
/// FIXME(wiring): same story as [`JOIN_GAME_ID_1_13`] â€” the id moves between
/// versions (0x22 in 1.13/1.14, 0x21 in 1.15/1.16, ...). Table comes later.
///
/// RU: id Ð¿Ð°ÐºÐµÑ‚Ð° Chunk Data Ð½Ð° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ðµ 393; Ð´Ð»Ñ Ð¿Ñ€Ð¾Ñ‡Ð¸Ñ… Ð²ÐµÑ€ÑÐ¸Ð¹ â€” FIXME.
pub const CHUNK_DATA_ID_1_13: u32 = 0x22;

/// Protocol number of 1.13 â€” the only wire format parsed field-by-field here.
/// RU: Ð½Ð¾Ð¼ÐµÑ€ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 1.13 â€” ÐµÐ´Ð¸Ð½ÑÑ‚Ð²ÐµÐ½Ð½Ñ‹Ð¹, Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÐ¼Ñ‹Ð¹ Ð·Ð´ÐµÑÑŒ Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼.
pub const PROTOCOL_1_13: i32 = 393;

/// Protocol number of 1.14 â€” the target of the base step.
/// RU: Ð½Ð¾Ð¼ÐµÑ€ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 1.14 â€” Ñ†ÐµÐ»ÑŒ Ð±Ð°Ð·Ð¾Ð²Ð¾Ð³Ð¾ ÑˆÐ°Ð³Ð°.
pub const PROTOCOL_1_14: i32 = 477;

/// One PLAY-state translation step in the "classic" family.
///
/// EN: fields `from`/`to` mirror the [`Step::FROM`]/[`Step::TO`] constants of
/// the only constructor this skeleton exposes
/// ([`PlayClassicStep::base_1_13_to_1_14`]). They exist as *fields* so a later
/// generalized step (any 393..767 pair) can reuse the same struct without a
/// breaking change; until that day the invariant `from == FROM && to == TO`
/// holds for every instance, because the fields are private and the base
/// constructor is the only way in.
///
/// RU: Ð¿Ð¾Ð»Ñ Ð¿Ð¾Ð²Ñ‚Ð¾Ñ€ÑÑŽÑ‚ ÐºÐ¾Ð½ÑÑ‚Ð°Ð½Ñ‚Ñ‹ ÑˆÐ°Ð³Ð°; Ð¾Ð½Ð¸ Ð¿Ð¾Ð»Ñ â€” Ñ‡Ñ‚Ð¾Ð±Ñ‹ Ð±ÑƒÐ´ÑƒÑ‰Ð¸Ð¹ Ð¾Ð±Ð¾Ð±Ñ‰Ñ‘Ð½Ð½Ñ‹Ð¹
/// ÑˆÐ°Ð³ (Ð»ÑŽÐ±Ð°Ñ Ð¿Ð°Ñ€Ð° Ð¸Ð· 393..767) Ð¿ÐµÑ€ÐµÐ¸ÑÐ¿Ð¾Ð»ÑŒÐ·Ð¾Ð²Ð°Ð» Ñ‚Ð¾Ñ‚ Ð¶Ðµ Ñ‚Ð¸Ð¿ Ð±ÐµÐ· Ð»Ð¾Ð¼ÐºÐ¸ API.
/// ÐŸÐ¾ÐºÐ° Ð¸Ð½Ð²Ð°Ñ€Ð¸Ð°Ð½Ñ‚ `from == FROM` Ð´ÐµÑ€Ð¶Ð¸Ñ‚ÑÑ: Ð¿Ð¾Ð»Ñ Ð¿Ñ€Ð¸Ð²Ð°Ñ‚Ð½Ñ‹, ÐºÐ¾Ð½ÑÑ‚Ñ€ÑƒÐºÑ‚Ð¾Ñ€ Ð¾Ð´Ð¸Ð½.
pub struct PlayClassicStep {
    /// Source protocol of this step instance (equals [`Step::FROM`]).
    /// RU: Ð¸ÑÑ…Ð¾Ð´Ð½Ñ‹Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð° ÑˆÐ°Ð³Ð°.
    from: i32,
    /// Target protocol of this step instance (equals [`Step::TO`]).
    /// RU: Ñ†ÐµÐ»ÐµÐ²Ð¾Ð¹ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» ÑÐºÐ·ÐµÐ¼Ð¿Ð»ÑÑ€Ð° ÑˆÐ°Ð³Ð°.
    to: i32,
}

impl PlayClassicStep {
    /// The base 1.13 -> 1.14 step (protocol 393 -> 477).
    ///
    /// EN: the only public constructor of this skeleton. Its packets are
    /// parsed at 393; nothing is re-encoded yet (see the module doc â€” the
    /// translation *into* 1.14 shapes arrives with the wiring).
    ///
    /// RU: Ð±Ð°Ð·Ð¾Ð²Ñ‹Ð¹ ÑˆÐ°Ð³ 1.13 -> 1.14; ÐµÐ´Ð¸Ð½ÑÑ‚Ð²ÐµÐ½Ð½Ñ‹Ð¹ ÐºÐ¾Ð½ÑÑ‚Ñ€ÑƒÐºÑ‚Ð¾Ñ€ ÑÐºÐµÐ»ÐµÑ‚Ð°.
    pub fn base_1_13_to_1_14() -> Self {
        PlayClassicStep {
            from: PROTOCOL_1_13,
            to: PROTOCOL_1_14,
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
    /// EN: dispatches by packet id **only for protocol 393**:
    /// - [`JOIN_GAME_ID_1_13`] -> full field parse into
    ///   [`ParsedJoin13`];
    /// - [`CHUNK_DATA_ID_1_13`] -> field parse into [`ParsedChunk13`];
    /// - anything else (and any packet at a source version other than 393)
    ///   -> [`TranslatedClassic::Other`] passthrough.
    ///
    /// A known id whose body fails to parse is a [`ViaError::Malformed`]
    /// (non-fatal: the caller drops the packet); an unknown id is *not* an
    /// error at this stage of the skeleton â€” it passes through.
    ///
    /// FIXME(wiring): for every source version besides 393 the answer is
    /// always `Other`, because per-version id tables are not wired yet.
    ///
    /// RU: Ñ€Ð°Ð·Ð±Ð¾Ñ€ clientbound PLAY-Ð¿Ð°ÐºÐµÑ‚Ð°; Ð³Ð»ÑƒÐ±Ð¾ÐºÐ¸Ð¹ Ð¿Ð°Ñ€ÑÐ¸Ð½Ð³ Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Ð´Ð»Ñ 393,
    /// Ð¾ÑÑ‚Ð°Ð»ÑŒÐ½Ð¾Ðµ â€” Ð¿Ð°Ñsthrough.
    pub fn parse_clientbound(
        &self,
        id: u32,
        body: &[u8],
    ) -> Result<TranslatedClassic, ViaError> {
        // Only protocol 393 has field layouts in this skeleton.
        // RU: Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼ Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÐ¼ Ñ‚Ð¾Ð»ÑŒÐºÐ¾ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð» 393.
        if self.from != PROTOCOL_1_13 {
            // FIXME(wiring): per-version id tables + layouts for 401/404/477+.
            return Ok(TranslatedClassic::Other(id, body.to_vec()));
        }
        match id {
            JOIN_GAME_ID_1_13 => {
                let join = parse_join_393(body)?;
                Ok(TranslatedClassic::Join(join))
            }
            CHUNK_DATA_ID_1_13 => {
                let chunk = parse_chunk_393(body)?;
                Ok(TranslatedClassic::Chunk(chunk))
            }
            // Unknown ids ride along untouched until the id tables arrive.
            _ => Ok(TranslatedClassic::Other(id, body.to_vec())),
        }
    }
}

impl Step for PlayClassicStep {
    const FROM: i32 = PROTOCOL_1_13;
    const TO: i32 = PROTOCOL_1_14;

    fn name(&self) -> &'static str {
        "play classic 1.13 -> 1.14 (join/chunk skeleton)"
    }
}

/// The outcome of parsing one PLAY packet in this step.
///
/// RU: Ñ€ÐµÐ·ÑƒÐ»ÑŒÑ‚Ð°Ñ‚ Ñ€Ð°Ð·Ð±Ð¾Ñ€Ð° Ð¾Ð´Ð½Ð¾Ð³Ð¾ PLAY-Ð¿Ð°ÐºÐµÑ‚Ð° ÑˆÐ°Ð³Ð°.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslatedClassic {
    /// A parsed Join Game (protocol 393 layout).
    /// RU: Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Join Game (Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐ° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 393).
    Join(ParsedJoin13),
    /// A parsed Chunk Data (protocol 393 layout).
    /// RU: Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Chunk Data (Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐ° Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 393).
    Chunk(ParsedChunk13),
    /// A packet this skeleton does not touch: its wire id and raw body,
    /// byte-for-byte as they arrived.
    /// RU: Ð½ÐµÑ‚Ñ€Ð¾Ð½ÑƒÑ‚Ñ‹Ð¹ Ð¿Ð°ÐºÐµÑ‚: id Ð¸ ÑÑ‹Ñ€Ð¾Ðµ Ñ‚ÐµÐ»Ð¾ ÐºÐ°Ðº ÐµÑÑ‚ÑŒ.
    Other(u32, Vec<u8>),
}

/// Join Game (clientbound) parsed at protocol 393 â€” 1.13 only.
///
/// EN: field order on the wire at 393:
/// `Int entity-id, UByte gamemode, Int dimension, UByte difficulty,
/// UByte max-players, String level-type, Boolean reduced-debug-info`.
/// The format mutates almost every version afterwards (difficulty dies in
/// 1.15, dimensions become strings in 1.16, view/hardcore flags appear in
/// 1.20+), which is exactly why this struct is 393-shaped and nothing else.
///
/// RU: Join Game, Ñ€Ð°Ð·Ð¾Ð±Ñ€Ð°Ð½Ð½Ñ‹Ð¹ Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 393 (Ñ‚Ð¾Ð»ÑŒÐºÐ¾ 1.13):
/// entityId, gamemode, dimension, difficulty, maxPlayers, levelType,
/// reducedDebugInfo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedJoin13 {
    /// The player's own entity id.
    /// RU: id ÑÑƒÑ‰Ð½Ð¾ÑÑ‚Ð¸ Ð¸Ð³Ñ€Ð¾ÐºÐ°.
    pub entity_id: i32,
    /// Client-side gamemode (0 = survival, 1 = creative, 2 = adventure, 3 = spectator).
    /// RU: Ð¸Ð³Ñ€Ð¾Ð²Ð¾Ð¹ Ñ€ÐµÐ¶Ð¸Ð¼.
    pub gamemode: u8,
    /// Dimension id as sent at 393 (numeric: -1 nether, 0 overworld, 1 end).
    /// RU: Ñ‡Ð¸ÑÐ»Ð¾Ð²Ð¾Ð¹ id Ð¸Ð·Ð¼ÐµÑ€ÐµÐ½Ð¸Ñ.
    pub dimension: i32,
    /// Difficulty byte (0 peaceful .. 3 hard). Removed from the packet in 1.15.
    /// RU: ÑÐ»Ð¾Ð¶Ð½Ð¾ÑÑ‚ÑŒ; Ð¸Ð· Ð¿Ð°ÐºÐµÑ‚Ð° ÑƒÐ´Ð°Ð»ÐµÐ½Ð° Ð² 1.15.
    pub difficulty: u8,
    /// Max players â€” client-side cap for the player list, not a server truth.
    /// RU: Ð¼Ð°ÐºÑÐ¸Ð¼ÑƒÐ¼ Ð¸Ð³Ñ€Ð¾ÐºÐ¾Ð² (ÐºÐ»Ð¸ÐµÐ½Ñ‚ÑÐºÐ¸Ð¹ Ð»Ð¸Ð¼Ð¸Ñ‚ ÑÐ¿Ð¸ÑÐºÐ°).
    pub max_players: u8,
    /// Level type string, e.g. `"default"`, `"flat"`.
    /// RU: ÑÑ‚Ñ€Ð¾ÐºÐ° Ñ‚Ð¸Ð¿Ð° Ð¼Ð¸Ñ€Ð°.
    pub level_type: String,
    /// Whether the client should render the reduced (F3) debug info.
    /// RU: Ð²ÐºÐ»ÑŽÑ‡Ñ‘Ð½ Ð»Ð¸ ÑÐ¾ÐºÑ€Ð°Ñ‰Ñ‘Ð½Ð½Ñ‹Ð¹ debug-Ð¸Ð½Ñ„Ð¾.
    pub reduced_debug_info: bool,
}

/// Chunk Data (clientbound) parsed at protocol 393 â€” 1.13 only.
///
/// EN: field order on the wire at 393:
/// `Int chunk-x, Int chunk-z, Boolean full-chunk, VarInt primary-mask,
/// VarInt size, Byte[size] data, NBT-tag-array block-entities`.
///
/// The `data` stored here is **only** the `size`-long section payload: the
/// biome array (present when `full` is set, prepended at 393 inside `data`
/// proper, 256 bytes right at the start â€” left as raw bytes, not decoded)
/// plus the palette-coded sections. The block-entity NBT tail is validated
/// to exist only as "whatever remains after `size` bytes" and is *not*
/// retained in this struct â€” the skeleton has no NBT-level rewriter yet.
///
/// RU: Chunk Data Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ 393: chunkX, chunkZ, fullChunk, primaryMask,
/// size, data, Ñ…Ð²Ð¾ÑÑ‚ blockEntities (NBT). Ð’ `data` â€” Ñ‚Ð¾Ð»ÑŒÐºÐ¾ ÑÐµÐºÑ†Ð¸Ð¾Ð½Ð½Ð°Ñ Ñ‡Ð°ÑÑ‚ÑŒ
/// (Ð±Ð¸Ð¾Ð¼Ñ‹ Ð²Ð½ÑƒÑ‚Ñ€Ð¸, Ð½Ðµ Ð´ÐµÐºÐ¾Ð´Ð¸Ñ€ÑƒÐµÐ¼); NBT-Ñ…Ð²Ð¾ÑÑ‚ Ð½Ðµ Ñ…Ñ€Ð°Ð½Ð¸Ñ‚ÑÑ â€” ÑÐºÐµÐ»ÐµÑ‚ ÐµÐ³Ð¾ Ð¿Ð¾ÐºÐ°
/// Ð½Ðµ Ð¿ÐµÑ€ÐµÐ¿Ð¸ÑÑ‹Ð²Ð°ÐµÑ‚.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedChunk13 {
    /// Chunk X coordinate (chunk units, not blocks).
    /// RU: ÐºÐ¾Ð¾Ñ€Ð´Ð¸Ð½Ð°Ñ‚Ð° X Ñ‡Ð°Ð½ÐºÐ°.
    pub x: i32,
    /// Chunk Z coordinate (chunk units, not blocks).
    /// RU: ÐºÐ¾Ð¾Ñ€Ð´Ð¸Ð½Ð°Ñ‚Ð° Z Ñ‡Ð°Ð½ÐºÐ°.
    pub z: i32,
    /// Whether this is a full chunk (replaces the old one; carries biomes).
    /// RU: Ð¿Ð¾Ð»Ð½Ñ‹Ð¹ Ð»Ð¸ Ñ‡Ð°Ð½Ðº (Ñ Ð±Ð¸Ð¾Ð¼Ð°Ð¼Ð¸, Ð·Ð°Ð¼ÐµÐ½ÑÐµÑ‚ Ð¿Ñ€ÐµÐ¶Ð½Ð¸Ð¹).
    pub full: bool,
    /// Bitmask of present vertical sections (bit i = section i, 16 in total
    /// at 393; 18 bits exist in the wild for edge servers â€” kept raw).
    /// RU: Ð±Ð¸Ñ‚Ð¾Ð²Ð°Ñ Ð¼Ð°ÑÐºÐ° ÑÐµÐºÑ†Ð¸Ð¹ Ñ‡Ð°Ð½ÐºÐ°.
    pub primary_mask: u32,
    /// The `size`-long raw section payload (biomes + palette sections).
    /// RU: ÑÑ‹Ñ€Ð°Ñ ÑÐµÐºÑ†Ð¸Ð¾Ð½Ð½Ð°Ñ Ñ‡Ð°ÑÑ‚ÑŒ Ð´Ð»Ð¸Ð½Ð¾Ð¹ size.
    pub data: Vec<u8>,
}

/// Parses a Join Game body at protocol 393 (1.13). See [`ParsedJoin13`].
///
/// RU: Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Join Game Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ Ð¿Ñ€Ð¾Ñ‚Ð¾ÐºÐ¾Ð»Ð° 393.
fn parse_join_393(body: &[u8]) -> Result<ParsedJoin13, ViaError> {
    let mut pos = 0usize;

    let entity_id = read_i32(body, &mut pos, "join: entity id")?;
    let gamemode = read_u8(body, &mut pos, "join: gamemode")?;
    let dimension = read_i32(body, &mut pos, "join: dimension")?;
    let difficulty = read_u8(body, &mut pos, "join: difficulty")?;
    let max_players = read_u8(body, &mut pos, "join: max players")?;
    let level_type = read_string(body, &mut pos, "join: level type")?;
    let reduced_debug_info = read_bool(body, &mut pos, "join: reduced debug info")?;

    if pos != body.len() {
        return Err(ViaError::Malformed {
            context: "join: trailing bytes after last field".to_string(),
            bytes_seen: pos,
        });
    }

    Ok(ParsedJoin13 {
        entity_id,
        gamemode,
        dimension,
        difficulty,
        max_players,
        level_type,
        reduced_debug_info,
    })
}

/// Parses a Chunk Data body at protocol 393 (1.13). See [`ParsedChunk13`].
///
/// The declared `size` is validated against the remaining buffer (`size`
/// covers only the section payload; the block-entity NBT tail follows it,
/// so `size == remaining` is legal, as is `size < remaining`).
///
/// RU: Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Chunk Data Ð¿Ð¾ Ñ€Ð°ÑÐºÐ»Ð°Ð´ÐºÐµ 393; `size` Ð¿Ñ€Ð¾Ð²ÐµÑ€ÑÐµÑ‚ÑÑ Ð¿Ñ€Ð¾Ñ‚Ð¸Ð²
/// Ð¾ÑÑ‚Ð°Ñ‚ÐºÐ° Ð±ÑƒÑ„ÐµÑ€Ð° (NBT-Ñ…Ð²Ð¾ÑÑ‚ blockEntities Ð¸Ð´Ñ‘Ñ‚ Ð¿Ð¾ÑÐ»Ðµ size-Ð±Ð°Ð¹Ñ‚Ð¾Ð²).
fn parse_chunk_393(body: &[u8]) -> Result<ParsedChunk13, ViaError> {
    let mut pos = 0usize;

    let x = read_i32(body, &mut pos, "chunk: chunk x")?;
    let z = read_i32(body, &mut pos, "chunk: chunk z")?;
    let full = read_bool(body, &mut pos, "chunk: full chunk")?;
    let mask = read_varint(body, &mut pos)?;
    if mask < 0 {
        return Err(ViaError::Malformed {
            context: "chunk: negative primary mask".to_string(),
            bytes_seen: pos,
        });
    }
    let primary_mask = mask as u32;
    let size = read_varint(body, &mut pos)?;
    if size < 0 {
        return Err(ViaError::Malformed {
            context: "chunk: negative data size".to_string(),
            bytes_seen: pos,
        });
    }
    let size = size as usize;

    // size covers the section payload only; the NBT block-entity tail may
    // follow, so the check is `size <= remaining`, not `size == remaining`.
    // RU: size â€” Ñ‚Ð¾Ð»ÑŒÐºÐ¾ ÑÐµÐºÑ†Ð¸Ð¾Ð½Ð½Ð°Ñ Ñ‡Ð°ÑÑ‚ÑŒ; Ð·Ð° Ð½ÐµÐ¹ Ð¼Ð¾Ð¶ÐµÑ‚ Ð¸Ð´Ñ‚Ð¸ NBT-Ñ…Ð²Ð¾ÑÑ‚.
    let remaining = body.len() - pos;
    if size > remaining {
        return Err(ViaError::Malformed {
            context: format!(
                "chunk: declared data size {} exceeds remaining {} bytes",
                size, remaining
            ),
            bytes_seen: pos,
        });
    }

    let data = body[pos..pos + size].to_vec();
    // The bytes after `data` (if any) are the block-entity NBT array â€”
    // intentionally not retained here (see the ParsedChunk13 doc).
    // RU: Ð¾ÑÑ‚Ð°Ñ‚Ð¾Ðº â€” NBT blockEntities; ÑÐ¾Ð·Ð½Ð°Ñ‚ÐµÐ»ÑŒÐ½Ð¾ Ð½Ðµ Ñ…Ñ€Ð°Ð½Ð¸Ð¼.

    Ok(ParsedChunk13 {
        x,
        z,
        full,
        primary_mask,
        data,
    })
}

/// Counts set bits in a 32-bit section mask â€” the number of sections a
/// chunk payload carries.
///
/// EN: plain popcount; `popcount32(0b101)` is 2. Chunk masks are the only
/// caller today.
///
/// RU: Ñ‡Ð¸ÑÐ»Ð¾ Ð²Ñ‹ÑÑ‚Ð°Ð²Ð»ÐµÐ½Ð½Ñ‹Ñ… Ð±Ð¸Ñ‚Ð¾Ð² Ð¼Ð°ÑÐºÐ¸ ÑÐµÐºÑ†Ð¸Ð¹.
pub fn popcount32(mask: u32) -> u32 {
    mask.count_ones()
}

/// Rough lower-bound estimate of a 1.13 chunk section payload's length,
/// **without** decoding palettes.
///
/// EN: deliberately simplified. At protocol 393 each of the `sections`
/// sections begins with `UByte bits-per-block` followed by:
/// - *direct* palette (bits >= 9): a raw `4096 * bits / 8` byte array,
///   no palette length precedes it;
/// - *indirect* palette (bits < 9): `VarInt palette-length` + entries +
///   the data array â€” whose exact size requires reading the palette, which
///   this helper refuses to do.
///
/// Therefore: the walk returns `Some(total)` only while every section reads
/// as direct; the first indirect section (or a truncated stream) returns
/// `None`. The returned `total` counts `bits-per-block` bytes plus data
/// arrays only â€” a real section also carries a `VarInt block-count` in
/// 1.14+, but at 393 it does not, so no slack is added. **Do not** use this
/// for slicing; it exists for sanity checks and logging.
///
/// RU: Ð³Ñ€ÑƒÐ±Ð°Ñ Ð¾Ñ†ÐµÐ½ÐºÐ° Ð´Ð»Ð¸Ð½Ñ‹ ÑÐµÐºÑ†Ð¸Ð¾Ð½Ð½Ð¾Ð¹ Ñ‡Ð°ÑÑ‚Ð¸ Ñ‡Ð°Ð½ÐºÐ° 1.13 Ð±ÐµÐ· Ð´ÐµÐºÐ¾Ð´Ð¸Ñ€Ð¾Ð²Ð°Ð½Ð¸Ñ
/// Ð¿Ð°Ð»Ð¸Ñ‚Ñ€: ÑÑ‡Ð¸Ñ‚Ð°ÐµÐ¼ Ñ‚Ð¾Ð»ÑŒÐºÐ¾ direct-ÑÐµÐºÑ†Ð¸Ð¸ (bits >= 9, 4096*bits/8 Ð±Ð°Ð¹Ñ‚);
/// Ð¿ÐµÑ€Ð²Ð°Ñ indirect-ÑÐµÐºÑ†Ð¸Ñ Ð¸Ð»Ð¸ Ð¾Ð±Ñ€Ñ‹Ð² â€” None. ÐÐµ Ð´Ð»Ñ Ð½Ð°Ñ€ÐµÐ·ÐºÐ¸ Ð±ÑƒÑ„ÐµÑ€Ð°.
pub fn section_palette_data_len(data: &[u8], sections: u32) -> Option<usize> {
    let mut pos = 0usize;
    let mut total = 0usize;
    for _ in 0..sections {
        if pos >= data.len() {
            // Truncated before the section header: no estimate possible.
            // RU: Ð¾Ð±Ñ€Ñ‹Ð² Ð¿ÐµÑ€ÐµÐ´ Ð·Ð°Ð³Ð¾Ð»Ð¾Ð²ÐºÐ¾Ð¼ ÑÐµÐºÑ†Ð¸Ð¸.
            return None;
        }
        let bits = data[pos];
        pos += 1;
        total += 1;
        if bits >= 9 {
            // Direct palette: raw 4096 entries at `bits` bits each.
            // RU: direct-Ð¿Ð°Ð»Ð¸Ñ‚Ñ€Ð°: ÑÑ‹Ñ€Ñ‹Ðµ 4096 Ð·Ð½Ð°Ñ‡ÐµÐ½Ð¸Ð¹ Ð¿Ð¾ bits Ð±Ð¸Ñ‚Ð¾Ð².
            let len = 4096usize * bits as usize / 8;
            if pos + len > data.len() {
                return None;
            }
            pos += len;
            total += len;
        } else {
            // Indirect palette: exact sizing requires reading the palette
            // length and entries â€” out of scope for this helper.
            // RU: indirect-Ð¿Ð°Ð»Ð¸Ñ‚Ñ€Ð°: Ñ‚Ð¾Ñ‡Ð½Ñ‹Ð¹ Ñ€Ð°Ð·Ð¼ÐµÑ€ Ñ‚Ñ€ÐµÐ±ÑƒÐµÑ‚ Ñ‡Ñ‚ÐµÐ½Ð¸Ñ Ð¿Ð°Ð»Ð¸Ñ‚Ñ€Ñ‹.
            return None;
        }
    }
    Some(total)
}

// ---------------------------------------------------------------------------
// Tiny field readers (i32/u8/bool/String), shared by the two parsers above.
// Each names its context for the Malformed error, per the crate contract.
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

    /// Builds a 393 Join Game body from field values.
    /// RU: ÑÐ¾Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Join Game (393) Ð¸Ð· Ð·Ð½Ð°Ñ‡ÐµÐ½Ð¸Ð¹ Ð¿Ð¾Ð»ÐµÐ¹.
    fn join_body(entity_id: i32, gamemode: u8, dimension: i32, difficulty: u8,
                 max_players: u8, level_type: &str, reduced: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&entity_id.to_be_bytes());
        out.push(gamemode);
        out.extend_from_slice(&dimension.to_be_bytes());
        out.push(difficulty);
        out.push(max_players);
        write_varint(level_type.len() as i32, &mut out);
        out.extend_from_slice(level_type.as_bytes());
        out.push(u8::from(reduced));
        out
    }

    /// Builds a 393 Chunk Data body: header + `size`-announced payload +
    /// optional raw NBT tail.
    /// RU: ÑÐ¾Ð±Ð¸Ñ€Ð°ÐµÑ‚ Ñ‚ÐµÐ»Ð¾ Chunk Data (393): Ð·Ð°Ð³Ð¾Ð»Ð¾Ð²Ð¾Ðº + size + Ñ…Ð²Ð¾ÑÑ‚.
    fn chunk_body(x: i32, z: i32, full: bool, mask: i32, payload: &[u8],
                  tail: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&x.to_be_bytes());
        out.extend_from_slice(&z.to_be_bytes());
        out.push(u8::from(full));
        write_varint(mask, &mut out);
        write_varint(payload.len() as i32, &mut out);
        out.extend_from_slice(payload);
        out.extend_from_slice(tail);
        out
    }

    #[test]
    fn step_constants_match_base_constructor() {
        let step = PlayClassicStep::base_1_13_to_1_14();
        assert_eq!(step.from_protocol(), PlayClassicStep::FROM);
        assert_eq!(step.to_protocol(), PlayClassicStep::TO);
        assert_eq!(PlayClassicStep::FROM, 393);
        assert_eq!(PlayClassicStep::TO, 477);
        assert!(step.name().contains("1.13"));
    }

    #[test]
    fn join_parse_golden() {
        // A plausible 1.13 join: entity 12345, creative, overworld, normal,
        // 20 players, flat world, reduced debug off.
        // RU: Ñ‚Ð¸Ð¿Ð¾Ð²Ð¾Ð¹ join 1.13.
        let body = join_body(12345, 1, 0, 2, 20, "flat", false);
        let step = PlayClassicStep::base_1_13_to_1_14();
        let translated = step
            .parse_clientbound(JOIN_GAME_ID_1_13, &body)
            .expect("golden join must parse");
        match translated {
            TranslatedClassic::Join(j) => {
                assert_eq!(j.entity_id, 12345);
                assert_eq!(j.gamemode, 1);
                assert_eq!(j.dimension, 0);
                assert_eq!(j.difficulty, 2);
                assert_eq!(j.max_players, 20);
                assert_eq!(j.level_type, "flat");
                assert!(!j.reduced_debug_info);
            }
            other => panic!("expected Join, got {:?}", other),
        }
    }

    #[test]
    fn join_parse_negative_dimension_and_reduced() {
        // Nether join: dimension -1, reduced debug on, long level type.
        // RU: join Ð² Ð½ÐµÐ·ÐµÑ€: dimension -1.
        let body = join_body(-7, 3, -1, 3, 60, "default", true);
        let parsed = parse_join_393(&body).expect("nether join must parse");
        assert_eq!(parsed.entity_id, -7);
        assert_eq!(parsed.dimension, -1);
        assert!(parsed.reduced_debug_info);
        assert_eq!(parsed.level_type, "default");
    }

    #[test]
    fn join_truncated_is_malformed() {
        // Full body minus the last 2 bytes (boolean + part of string).
        // RU: Ñ‚ÐµÐ»Ð¾ Ð±ÐµÐ· Ð¿Ð¾ÑÐ»ÐµÐ´Ð½Ð¸Ñ… Ð±Ð°Ð¹Ñ‚Ð¾Ð² â€” Malformed, Ð½Ðµ Ð¿Ð°Ð½Ð¸ÐºÐ°.
        let body = join_body(1, 0, 0, 0, 10, "default", false);
        let cut = &body[..body.len() - 2];
        let err = parse_join_393(cut).expect_err("truncated join must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn join_trailing_bytes_is_malformed() {
        let mut body = join_body(1, 0, 0, 0, 10, "default", false);
        body.push(0xDE); // one extra byte after reducedDebugInfo
        let err = parse_join_393(&body).expect_err("trailing join must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn chunk_parse_valid_single_direct_section() {
        // One section (mask 0b1), direct palette at 13 bits:
        // 4096 * 13 / 8 = 6656 data bytes, plus a 1-byte bits-per-block
        // header = 6657 payload bytes. Plus a 4-byte raw "NBT tail".
        // RU: Ð¾Ð´Ð½Ð° ÑÐµÐºÑ†Ð¸Ñ, direct-Ð¿Ð°Ð»Ð¸Ñ‚Ñ€Ð° 13 Ð±Ð¸Ñ‚: 4096*13/8 = 6656 Ð±Ð°Ð¹Ñ‚.
        let bits = 13u8;
        let data_len = 4096usize * bits as usize / 8; // = 6656
        assert_eq!(data_len, 6656, "4096*13/8 must be 6656");
        let mut payload: Vec<u8> = Vec::with_capacity(1 + data_len);
        payload.push(bits);
        payload.extend(std::iter::repeat_n(0u8, data_len));
        let body = chunk_body(10, -20, true, 1, &payload, &[0x0A, 0x00, 0x00, 0x00]);

        let parsed = parse_chunk_393(&body).expect("valid chunk must parse");
        assert_eq!(parsed.x, 10);
        assert_eq!(parsed.z, -20);
        assert!(parsed.full);
        assert_eq!(parsed.primary_mask, 1);
        assert_eq!(parsed.data.len(), payload.len());
        assert_eq!(parsed.data[0], bits); // bits-per-block kept raw
        // The NBT tail is not part of data (size announces only the payload).
        // RU: NBT-Ñ…Ð²Ð¾ÑÑ‚ Ð½Ðµ Ð²Ñ…Ð¾Ð´Ð¸Ñ‚ Ð² data.
        assert_eq!(&body[body.len() - 4..], &[0x0A, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn chunk_size_mismatch_is_malformed() {
        // Declares 100 bytes of data but only 10 remain: Malformed.
        // RU: Ð·Ð°ÑÐ²Ð»ÐµÐ½Ð¾ 100 Ð±Ð°Ð¹Ñ‚ Ð´Ð°Ð½Ð½Ñ‹Ñ…, Ð¾ÑÑ‚Ð°Ð»Ð¾ÑÑŒ 10 â€” Malformed.
        let mut body = Vec::new();
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&0i32.to_be_bytes());
        body.push(1); // full
        write_varint(1, &mut body); // mask
        write_varint(100, &mut body); // lying size
        body.extend_from_slice(&[0u8; 10]);

        let err = parse_chunk_393(&body).expect_err("size mismatch must fail");
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. }
                if context.contains("exceeds")),
            "{}",
            err
        );
    }

    #[test]
    fn chunk_size_equal_to_remaining_is_valid() {
        // No NBT tail: size == remaining must parse (documented rule).
        // RU: Ð±ÐµÐ· NBT-Ñ…Ð²Ð¾ÑÑ‚Ð°: size == Ð¾ÑÑ‚Ð°Ñ‚Ð¾Ðº â€” Ð²Ð°Ð»Ð¸Ð´Ð½Ð¾.
        let payload = [13u8, 0, 0, 0]; // 4 bytes, whatever
        let body = chunk_body(0, 0, false, 1, &payload, &[]);
        let parsed = parse_chunk_393(&body).expect("size == remaining must parse");
        assert_eq!(parsed.data, payload);
    }

    #[test]
    fn chunk_negative_mask_is_malformed() {
        let mut body = Vec::new();
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&0i32.to_be_bytes());
        body.push(1);
        write_varint(-1, &mut body); // negative mask
        let err = parse_chunk_393(&body).expect_err("negative mask must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn chunk_truncated_header_is_malformed() {
        // Only 3 bytes of the x field: Malformed, no panic.
        // RU: Ð¾Ð±Ñ€Ñ‹Ð² Ð² Ð¿ÐµÑ€Ð²Ð¾Ð¼ Ð¿Ð¾Ð»Ðµ â€” Malformed, Ð±ÐµÐ· Ð¿Ð°Ð½Ð¸ÐºÐ¸.
        let err = parse_chunk_393(&[1, 2, 3]).expect_err("truncated chunk must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn unknown_packet_passthrough() {
        let step = PlayClassicStep::base_1_13_to_1_14();
        let out = step
            .parse_clientbound(0x42, b"unchanged bytes")
            .expect("passthrough must not fail");
        match out {
            TranslatedClassic::Other(id, body) => {
                assert_eq!(id, 0x42);
                assert_eq!(body, b"unchanged bytes");
            }
            other => panic!("expected Other, got {:?}", other),
        }
    }

    #[test]
    fn join_id_at_other_version_is_passthrough() {
        // A step whose source is not 393 must not field-parse anything,
        // even a join id â€” FIXME(wiring) territory.
        // RU: ÑˆÐ°Ð³ Ñ Ð¸ÑÑ…Ð¾Ð´Ð½Ð¾Ð¹ Ð²ÐµÑ€ÑÐ¸ÐµÐ¹ Ð½Ðµ 393 Ð½Ðµ Ñ€Ð°Ð·Ð±Ð¸Ñ€Ð°ÐµÑ‚ join Ð¿Ð¾ Ð¿Ð¾Ð»ÑÐ¼.
        let step = PlayClassicStep { from: 477, to: 573 };
        let body = join_body(1, 0, 0, 0, 10, "default", false);
        let out = step
            .parse_clientbound(JOIN_GAME_ID_1_13, &body)
            .expect("non-393 must pass through");
        assert_eq!(out, TranslatedClassic::Other(JOIN_GAME_ID_1_13, body));
    }

    #[test]
    fn popcount_cases() {
        assert_eq!(popcount32(0), 0);
        assert_eq!(popcount32(1), 1);
        assert_eq!(popcount32(0b101), 2);
        assert_eq!(popcount32(0xFFFF), 16);
        assert_eq!(popcount32(0xFFFF_FFFF), 32);
        // A full 1.13 chunk mask: 16 sections.
        // RU: Ð¼Ð°ÑÐºÐ° Ð¿Ð¾Ð»Ð½Ð¾Ð³Ð¾ Ñ‡Ð°Ð½ÐºÐ° 1.13 â€” 16 ÑÐµÐºÑ†Ð¸Ð¹.
        assert_eq!(popcount32(0xFFFF), 16);
    }

    #[test]
    fn section_len_all_direct() {
        // Two direct sections: (1 + 6656) + (1 + 4096*9/8=4608) = 11266.
        // RU: Ð´Ð²Ðµ direct-ÑÐµÐºÑ†Ð¸Ð¸: 6657 + 4609 = 11266.
        let mut payload = Vec::new();
        payload.push(13);
        payload.extend_from_slice(&[0u8; 6656]);
        payload.push(9);
        payload.extend_from_slice(&[0u8; 4608]);
        let got = section_palette_data_len(&payload, 2)
            .expect("all-direct estimate must succeed");
        assert_eq!(got, 1 + 6656 + 1 + 4608);
        assert_eq!(got, 11266);
    }

    #[test]
    fn section_len_indirect_returns_none() {
        // First section has bits-per-block 4 (indirect palette): None.
        // RU: Ð¿ÐµÑ€Ð²Ð°Ñ ÑÐµÐºÑ†Ð¸Ñ indirect (bits=4) â€” None.
        let payload = [4u8, 0, 0, 0, 0];
        assert_eq!(section_palette_data_len(&payload, 1), None);
    }

    #[test]
    fn section_len_truncated_returns_none() {
        // Section count 2 but only one section's worth of bytes.
        // RU: ÑÐµÐºÑ†Ð¸Ð¹ Ð·Ð°ÑÐ²Ð»ÐµÐ½Ð¾ 2, Ð±Ð°Ð¹Ñ‚Ð¾Ð² Ð½Ð° Ð¾Ð´Ð½Ñƒ.
        let mut payload = Vec::new();
        payload.push(13);
        payload.extend_from_slice(&[0u8; 6656]);
        assert_eq!(section_palette_data_len(&payload, 2), None);
        // Empty buffer, one section: None.
        assert_eq!(section_palette_data_len(&[], 1), None);
        // Zero sections over any buffer: Some(0) â€” a mask of 0 is legal.
        assert_eq!(section_palette_data_len(&[], 0), Some(0));
    }
}
