//! NBT format differences between protocol versions (1.8 .. 26.3).
//!
//! This module is deliberately *not* a full NBT container/tree. The
//! translation pipeline only needs three things to move NBT payloads across
//! version boundaries:
//!
//! 1. **Root name stripping** â€” since 1.20.2 (protocol 764) the network form
//!    of NBT dropped the root tag's name: a packet carries
//!    `type + payload` instead of `type + name + payload`. Older servers
//!    still send the (usually empty) name; [`strip_root_name`] and
//!    [`add_root_name`] convert both ways.
//! 2. **A version flag** for the VarInt-length-prefixed NBT that appears in
//!    item components since 1.20.5 (protocol 765) â€” see
//!    [`uses_component_nbt`]. Only the flag lives here; the VarInt itself is
//!    read by the packet framing layer.
//! 3. **Modified UTF-8** (Java `DataOutput`) strings used by 1.8â€“1.15 era
//!    network NBT, where U+0000 is encoded as `C0 80` and supplementary
//!    code points as CESU-8-style surrogate pairs â€” see
//!    [`decode_modified_utf8`] / [`encode_modified_utf8`].
//!
//! For navigating inside a payload without building a tree there is a
//! minimal tag walker: [`TagType`], [`skip_payload`], [`skip_named_tag`].
//! It answers "where does this tag end" so callers can skip or search for
//! named fields while streaming.
//!
//! RU: Ñ€Ð°Ð·Ð»Ð¸Ñ‡Ð¸Ñ Ñ„Ð¾Ñ€Ð¼Ð°Ñ‚Ð° NBT Ð¼ÐµÐ¶Ð´Ñƒ Ð²ÐµÑ€ÑÐ¸ÑÐ¼Ð¸. ÐŸÐ¾Ð»Ð½Ð¾Ð³Ð¾ Ð´ÐµÑ€ÐµÐ²Ð° Ð½ÐµÑ‚ â€” Ñ‚Ð¾Ð»ÑŒÐºÐ¾
//! ÑÑ€ÐµÐ· ÐºÐ¾Ñ€Ð½ÐµÐ²Ð¾Ð³Ð¾ Ð¸Ð¼ÐµÐ½Ð¸, Ñ„Ð»Ð°Ð³ Ð²ÐµÑ€ÑÐ¸Ð¸ Ð´Ð»Ñ item-ÐºÐ¾Ð¼Ð¿Ð¾Ð½ÐµÐ½Ñ‚Ð¾Ð², modified UTF-8
//! Ð¸ Â«Ð¿Ñ€Ð¾Ð¿ÑƒÑÐºÐ°Ñ‚ÐµÐ»ÑŒÂ» payload-Ð¾Ð² Ñ‚ÐµÐ³Ð¾Ð² Ð´Ð»Ñ Ð½Ð°Ð²Ð¸Ð³Ð°Ñ†Ð¸Ð¸ Ð¿Ð¾ Ð¿Ð¾Ñ‚Ð¾ÐºÑƒ.

use crate::errors::ViaError;

/// Maximum nesting depth accepted by [`skip_payload`].
///
/// A hostile server could announce a compound nested millions of levels
/// deep; without this guard the recursive skip would overflow the stack and
/// abort the process, violating the crate rule "a malformed packet must
/// never panic the client".
///
/// RU: Ð¿Ñ€ÐµÐ´ÐµÐ» Ð²Ð»Ð¾Ð¶ÐµÐ½Ð½Ð¾ÑÑ‚Ð¸; Ð¸Ð½Ð°Ñ‡Ðµ Ñ€ÐµÐºÑƒÑ€ÑÐ¸Ñ ÑƒÑ€Ð¾Ð½Ð¸Ñ‚ ÑÑ‚ÐµÐº Ð½Ð° Ð²Ñ€Ð°Ð¶Ð´ÐµÐ±Ð½Ð¾Ð¼ Ð¿Ð°ÐºÐµÑ‚Ðµ.
const MAX_DEPTH: usize = 512;

/// All NBT tag types (the 13 defined by the format; ids 0â€“12).
///
/// RU: Ð²ÑÐµ 13 Ñ‚Ð¸Ð¿Ð¾Ð² Ñ‚ÐµÐ³Ð¾Ð² NBT (id 0â€“12).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TagType {
    /// 0x00 â€” end of a compound (or empty list marker). No payload.
    End,
    /// 0x01 â€” signed byte.
    Byte,
    /// 0x02 â€” signed big-endian 16-bit int.
    Short,
    /// 0x03 â€” signed big-endian 32-bit int.
    Int,
    /// 0x04 â€” signed big-endian 64-bit int.
    Long,
    /// 0x05 â€” 32-bit IEEE-754 float, big endian.
    Float,
    /// 0x06 â€” 64-bit IEEE-754 double, big endian.
    Double,
    /// 0x07 â€” Int length + raw bytes.
    ByteArray,
    /// 0x08 â€” unsigned short length + modified-UTF-8 bytes.
    String,
    /// 0x09 â€” element tag id + Int count + element payloads.
    List,
    /// 0x0A â€” named tags until [`TagType::End`].
    Compound,
    /// 0x0B â€” Int count + big-endian 32-bit ints.
    IntArray,
    /// 0x0C â€” Int count + big-endian 64-bit ints.
    LongArray,
}

impl TagType {
    /// Parse a tag id byte (0â€“12). Anything else is malformed input.
    ///
    /// RU: id Ñ‚ÐµÐ³Ð° Ð¸Ð· Ð±Ð°Ð¹Ñ‚Ð°; Ð²ÑÑ‘ Ð²Ð½Ðµ 0â€“12 â€” Ð¾ÑˆÐ¸Ð±ÐºÐ°.
    pub fn from_id(id: u8) -> Result<Self, ViaError> {
        match id {
            0x00 => Ok(TagType::End),
            0x01 => Ok(TagType::Byte),
            0x02 => Ok(TagType::Short),
            0x03 => Ok(TagType::Int),
            0x04 => Ok(TagType::Long),
            0x05 => Ok(TagType::Float),
            0x06 => Ok(TagType::Double),
            0x07 => Ok(TagType::ByteArray),
            0x08 => Ok(TagType::String),
            0x09 => Ok(TagType::List),
            0x0A => Ok(TagType::Compound),
            0x0B => Ok(TagType::IntArray),
            0x0C => Ok(TagType::LongArray),
            other => Err(ViaError::Malformed {
                context: format!("nbt tag id {} is not a known tag type", other),
                bytes_seen: 1,
            }),
        }
    }

    /// The wire id of this tag type.
    ///
    /// RU: Ð±Ð°Ð¹Ñ‚-id ÑÑ‚Ð¾Ð³Ð¾ Ñ‚Ð¸Ð¿Ð° Ñ‚ÐµÐ³Ð°.
    pub fn to_id(self) -> u8 {
        match self {
            TagType::End => 0x00,
            TagType::Byte => 0x01,
            TagType::Short => 0x02,
            TagType::Int => 0x03,
            TagType::Long => 0x04,
            TagType::Float => 0x05,
            TagType::Double => 0x06,
            TagType::ByteArray => 0x07,
            TagType::String => 0x08,
            TagType::List => 0x09,
            TagType::Compound => 0x0A,
            TagType::IntArray => 0x0B,
            TagType::LongArray => 0x0C,
        }
    }

    /// Size of a fixed-width primitive payload in bytes, `None` for the
    /// variable-length types (arrays, strings, containers).
    ///
    /// RU: Ñ€Ð°Ð·Ð¼ÐµÑ€ Ñ„Ð¸ÐºÑÐ¸Ñ€Ð¾Ð²Ð°Ð½Ð½Ð¾Ð³Ð¾ payload; `None` Ð´Ð»Ñ Ð¿ÐµÑ€ÐµÐ¼ÐµÐ½Ð½Ñ‹Ñ… Ñ‚Ð¸Ð¿Ð¾Ð².
    const fn fixed_size(self) -> Option<usize> {
        match self {
            TagType::End => Some(0),
            TagType::Byte => Some(1),
            TagType::Short => Some(2),
            TagType::Int | TagType::Float => Some(4),
            TagType::Long | TagType::Double => Some(8),
            _ => None,
        }
    }
}

/// Read `n` bytes at `*pos`, advancing it. Errors if the buffer is short.
fn take<'a>(buf: &'a [u8], pos: &mut usize, n: usize, context: &str) -> Result<&'a [u8], ViaError> {
    if n > buf.len().saturating_sub(*pos) {
        return Err(ViaError::Malformed {
            context: format!("{}: unexpected end of nbt buffer", context),
            bytes_seen: buf.len(),
        });
    }
    let slice = &buf[*pos..*pos + n];
    *pos += n;
    Ok(slice)
}

/// Read one big-endian `u16`.
fn read_u16(buf: &[u8], pos: &mut usize, context: &str) -> Result<u16, ViaError> {
    let b = take(buf, pos, 2, context)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

/// Read one big-endian `i32`.
fn read_i32(buf: &[u8], pos: &mut usize, context: &str) -> Result<i32, ViaError> {
    let b = take(buf, pos, 4, context)?;
    Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Skip the payload of `tag` starting at `*pos`, leaving `*pos` just past it.
///
/// Containers are walked recursively: a [`TagType::List`] reads the element
/// tag id and Int count first, a [`TagType::Compound`] consumes named tags
/// until [`TagType::End`]. Nesting deeper than 512 levels is rejected (see
/// [`MAX_DEPTH`]) so hostile input cannot blow the stack.
///
/// RU: Ð¿Ñ€Ð¾Ð¿ÑƒÑÑ‚Ð¸Ñ‚ÑŒ payload Ñ‚ÐµÐ³Ð°, ÑÐ´Ð²Ð¸Ð½ÑƒÐ² `pos` Ð·Ð° ÐµÐ³Ð¾ ÐºÐ¾Ð½ÐµÑ†. ÐšÐ¾Ð½Ñ‚ÐµÐ¹Ð½ÐµÑ€Ñ‹
/// Ð¾Ð±Ñ…Ð¾Ð´ÑÑ‚ÑÑ Ñ€ÐµÐºÑƒÑ€ÑÐ¸Ð²Ð½Ð¾, Ð²Ð»Ð¾Ð¶ÐµÐ½Ð½Ð¾ÑÑ‚ÑŒ Ð¾Ð³Ñ€Ð°Ð½Ð¸Ñ‡ÐµÐ½Ð°.
pub fn skip_payload(buf: &[u8], pos: &mut usize, tag: TagType) -> Result<(), ViaError> {
    skip_payload_depth(buf, pos, tag, 0)
}

/// Recursive worker for [`skip_payload`] carrying the current depth.
fn skip_payload_depth(
    buf: &[u8],
    pos: &mut usize,
    tag: TagType,
    depth: usize,
) -> Result<(), ViaError> {
    if depth > MAX_DEPTH {
        return Err(ViaError::Malformed {
            context: "nbt nesting deeper than 512 levels".to_string(),
            bytes_seen: *pos,
        });
    }
    if let Some(n) = tag.fixed_size() {
        take(buf, pos, n, "nbt fixed payload")?;
        return Ok(());
    }
    match tag {
        TagType::ByteArray => {
            let len = read_i32(buf, pos, "nbt byte array length")?;
            if len < 0 {
                return Err(ViaError::Malformed {
                    context: "nbt byte array with negative length".to_string(),
                    bytes_seen: *pos,
                });
            }
            take(buf, pos, len as usize, "nbt byte array body")?;
        }
        TagType::String => {
            let len = read_u16(buf, pos, "nbt string length")? as usize;
            take(buf, pos, len, "nbt string body")?;
        }
        TagType::List => {
            let elem_id = take(buf, pos, 1, "nbt list element type")?[0];
            let elem = TagType::from_id(elem_id)?;
            let count = read_i32(buf, pos, "nbt list count")?;
            // Java NBT treats count <= 0 as an empty list (writers emit -1 or 0).
            if count > 0 {
                if elem == TagType::End {
                    // End elements have no payload; a positive count would
                    // loop forever without consuming a byte.
                    return Err(ViaError::Malformed {
                        context: "nbt list of End tags with positive count".to_string(),
                        bytes_seen: *pos,
                    });
                }
                for _ in 0..count {
                    skip_payload_depth(buf, pos, elem, depth + 1)?;
                }
            }
        }
        TagType::Compound => loop {
            let child = skip_named_tag_depth(buf, pos, depth + 1)?;
            if child == TagType::End {
                break;
            }
        },
        TagType::IntArray => {
            let count = read_i32(buf, pos, "nbt int array count")?;
            if count < 0 {
                return Err(ViaError::Malformed {
                    context: "nbt int array with negative count".to_string(),
                    bytes_seen: *pos,
                });
            }
            let bytes = count as usize * 4;
            take(buf, pos, bytes, "nbt int array body")?;
        }
        TagType::LongArray => {
            let count = read_i32(buf, pos, "nbt long array count")?;
            if count < 0 {
                return Err(ViaError::Malformed {
                    context: "nbt long array with negative count".to_string(),
                    bytes_seen: *pos,
                });
            }
            let bytes = count as usize * 8;
            take(buf, pos, bytes, "nbt long array body")?;
        }
        // Fixed-size tags were handled by the early return above.
        _ => unreachable!("fixed-size tag reached variable-size branch"),
    }
    Ok(())
}

/// Skip one *named* tag (`type id + name + payload`) starting at `*pos`.
///
/// Returns the tag type read. A [`TagType::End`] byte consumes exactly one
/// byte (the id) and no name, per the format.
///
/// RU: Ð¿Ñ€Ð¾Ð¿ÑƒÑÑ‚Ð¸Ñ‚ÑŒ Ð¸Ð¼ÐµÐ½Ð¾Ð²Ð°Ð½Ð½Ñ‹Ð¹ Ñ‚ÐµÐ³ (id + Ð¸Ð¼Ñ + payload), Ð²ÐµÑ€Ð½ÑƒÐ² ÐµÐ³Ð¾ Ñ‚Ð¸Ð¿.
pub fn skip_named_tag(buf: &[u8], pos: &mut usize) -> Result<TagType, ViaError> {
    skip_named_tag_depth(buf, pos, 0)
}

/// Recursive worker for [`skip_named_tag`] carrying the current depth.
fn skip_named_tag_depth(buf: &[u8], pos: &mut usize, depth: usize) -> Result<TagType, ViaError> {
    let id = take(buf, pos, 1, "nbt tag id")?[0];
    let tag = TagType::from_id(id)?;
    if tag == TagType::End {
        return Ok(TagType::End);
    }
    let name_len = read_u16(buf, pos, "nbt tag name length")? as usize;
    take(buf, pos, name_len, "nbt tag name")?;
    skip_payload_depth(buf, pos, tag, depth)?;
    Ok(tag)
}

/// Convert pre-1.20.2 network NBT (`root type + root name + payload`) to the
/// modern root-name-less form (`root type + payload`).
///
/// `raw` must start with a tag id byte; the id, the root name's Short length
/// prefix and the name bytes are all skipped, so the returned slice is the
/// root *payload* (for the usual TAG_Compound root it begins with the first
/// child tag). Because a borrowed slice cannot drop leading bytes and keep
/// the root type byte at the front, the caller re-emits `raw[0]` itself when
/// rebuilding the modern form. A root tag id that is not a valid tag type, or
/// a name that runs past the end of the buffer, is [`ViaError::Malformed`].
///
/// RU: ÑƒÐ±Ñ€Ð°Ñ‚ÑŒ Ð¸Ð¼Ñ ÐºÐ¾Ñ€Ð½ÐµÐ²Ð¾Ð³Ð¾ Ñ‚ÐµÐ³Ð° (Ñ„Ð¾Ñ€Ð¼Ð° Ð´Ð¾ 1.20.2 -> ÑÐ¾Ð²Ñ€ÐµÐ¼ÐµÐ½Ð½Ð°Ñ).
/// Ð’Ð¾Ð·Ð²Ñ€Ð°Ñ‰Ð°ÐµÑ‚ÑÑ payload ÐºÐ¾Ñ€Ð½Ñ (Ð±ÐµÐ· Ð±Ð°Ð¹Ñ‚Ð° Ñ‚Ð¸Ð¿Ð° â€” ÐµÐ³Ð¾ Ð´Ð¾Ð¿Ð¸ÑˆÐµÑ‚ Ð²Ñ‹Ð·Ñ‹Ð²Ð°ÑŽÑ‰Ð¸Ð¹).
pub fn strip_root_name(raw: &[u8]) -> Result<&[u8], ViaError> {
    let mut pos = 0usize;
    let id = take(raw, &mut pos, 1, "nbt root tag id")?[0];
    let tag = TagType::from_id(id)?;
    if tag == TagType::End {
        // A root of TAG_End has no name and no payload in the old format
        // either; the payload is empty. (Servers never send this, but be
        // total and consistent with the "returns payload" contract.)
        return Ok(&raw[pos..]);
    }
    let name_len = read_u16(raw, &mut pos, "nbt root name length")? as usize;
    if name_len > raw.len().saturating_sub(pos) {
        return Err(ViaError::Malformed {
            context: "nbt root name runs past end of buffer".to_string(),
            bytes_seen: raw.len(),
        });
    }
    pos += name_len;
    Ok(&raw[pos..])
}

/// Convert modern network NBT (`root type + payload`) back to the pre-1.20.2
/// form (`root type + root name + payload`).
///
/// `raw` must be the modern form: a root tag id byte followed by the root
/// payload (NOT a bare payload slice as returned by [`strip_root_name`] â€”
/// prepend the tag id first). `name` is the root name to insert (usually
/// `""`). Names longer than 65535 bytes cannot be expressed with the Short
/// length prefix and are rejected.
///
/// RU: Ð²ÑÑ‚Ð°Ð²Ð¸Ñ‚ÑŒ Ð¸Ð¼Ñ ÐºÐ¾Ñ€Ð½ÐµÐ²Ð¾Ð³Ð¾ Ñ‚ÐµÐ³Ð° (ÑÐ¾Ð²Ñ€ÐµÐ¼ÐµÐ½Ð½Ð°Ñ Ñ„Ð¾Ñ€Ð¼Ð° -> Ð´Ð¾ 1.20.2).
/// ÐÐ° Ð²Ñ…Ð¾Ð´ â€” Ð±Ð°Ð¹Ñ‚ Ñ‚Ð¸Ð¿Ð° ÐºÐ¾Ñ€Ð½Ñ + payload, Ð½Ð° Ð²Ñ‹Ñ…Ð¾Ð´ â€” Ð»ÐµÐ³Ð°ÑÐ¸-Ñ„Ð¾Ñ€Ð¼Ð° Ñ Ð¸Ð¼ÐµÐ½ÐµÐ¼.
pub fn add_root_name(raw: &[u8], name: &str) -> Result<Vec<u8>, ViaError> {
    if raw.is_empty() {
        return Err(ViaError::Malformed {
            context: "nbt root: empty buffer has no tag id".to_string(),
            bytes_seen: 0,
        });
    }
    let tag = TagType::from_id(raw[0])?;
    if name.len() > u16::MAX as usize {
        return Err(ViaError::Malformed {
            context: "nbt root name longer than 65535 bytes".to_string(),
            bytes_seen: raw.len(),
        });
    }
    let mut out = Vec::with_capacity(raw.len() + name.len() + 2);
    out.push(tag.to_id());
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&raw[1..]);
    Ok(out)
}

/// Whether the protocol prefixes NBT inside item component slots with a
/// VarInt length (1.20.5 and later).
///
/// Used by the item pipeline to decide whether to read a VarInt before the
/// NBT blob. The NBT payload itself is unchanged â€” only the framing differs.
///
/// FIXME: verify exact boundary protocol number (assumed 765 = 1.20.5);
/// snapshots around it may need special-casing.
///
/// RU: ÐµÑÑ‚ÑŒ Ð»Ð¸ VarInt-Ð¿Ñ€ÐµÑ„Ð¸ÐºÑ Ð¿ÐµÑ€ÐµÐ´ NBT Ð² item-ÐºÐ¾Ð¼Ð¿Ð¾Ð½ÐµÐ½Ñ‚Ð°Ñ… (1.20.5+).
pub fn uses_component_nbt(protocol: i32) -> bool {
    // FIXME: verify exact boundary â€” 765 is believed to be 1.20.5.
    protocol >= 766 // 766 = 1.20.5 (fixed by reviewer)
}

/// Decode Java's *modified UTF-8* (as written by `DataOutput.writeUTF` and
/// used by 1.8â€“1.15 network NBT) into a Rust `String`.
///
/// Differences from standard UTF-8:
/// - U+0000 is encoded as `C0 80` (a bare `0x00` is not valid input, but is
///   accepted leniently and decoded to U+0000);
/// - supplementary code points are written as *two* 3-byte surrogate
///   sequences (CESU-8 style), which this decoder recombines;
/// - 4-byte sequences never appear.
///
/// Invalid or truncated sequences return [`ViaError::Malformed`]; no panic.
///
/// RU: Ð´ÐµÐºÐ¾Ð´Ð¸Ñ€Ð¾Ð²Ð°Ñ‚ÑŒ modified UTF-8 (Java DataOutput) Ð² ÑÑ‚Ñ€Ð¾ÐºÑƒ.
pub fn decode_modified_utf8(bytes: &[u8]) -> Result<String, ViaError> {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b0 = bytes[i];
        if b0 < 0x80 {
            // Bare NUL is invalid in strict modified UTF-8; accept it anyway.
            out.push(b0 as char);
            i += 1;
        } else if b0 == 0xC0 && i + 1 < bytes.len() && bytes[i + 1] == 0x80 {
            out.push('\u{0}');
            i += 2;
        } else if (0xC2..=0xDF).contains(&b0) {
            i += 1; // past the lead byte (orchestrator fix: was not advanced)
            let cp = read_cont(bytes, &mut i, 1, (b0 & 0x1F) as u32)?;
            out.push(char::from_u32(cp).ok_or_else(|| malformed_cp(cp, i))?);
        } else if (0xE0..=0xEF).contains(&b0) {
            i += 1; // past the lead byte (orchestrator fix)
            let cp = read_cont(bytes, &mut i, 2, (b0 & 0x0F) as u32)?;
            if (0xD800..=0xDFFF).contains(&cp) {
                // Surrogate: must be followed by a second 3-byte surrogate
                // sequence (lead byte 0xED in CESU-8); recombine the pair.
                let lead = cp;
                let next = *bytes.get(i).ok_or_else(|| truncated(i))?;
                if next != 0xED {
                    return Err(ViaError::Malformed {
                        context: "nbt modified utf-8: unpaired surrogate".to_string(),
                        bytes_seen: i,
                    });
                }
                i += 1; // past the second surrogate lead (0xED)
                let trail = read_cont(bytes, &mut i, 2, (next & 0x0F) as u32)?;
                if !(0xDC00..=0xDFFF).contains(&trail) {
                    return Err(ViaError::Malformed {
                        context: "nbt modified utf-8: surrogate not followed by low surrogate"
                            .to_string(),
                        bytes_seen: i,
                    });
                }
                let combined = 0x10000 + ((lead - 0xD800) << 10) + (trail - 0xDC00);
                out.push(char::from_u32(combined).ok_or_else(|| malformed_cp(combined, i))?);
            } else {
                out.push(char::from_u32(cp).ok_or_else(|| malformed_cp(cp, i))?);
            }
        } else {
            return Err(ViaError::Malformed {
                context: format!(
                    "nbt modified utf-8: unexpected lead byte 0x{:02X} (4-byte sequences are \
                     never valid here)",
                    b0
                ),
                bytes_seen: i,
            });
        }
    }
    Ok(out)
}

/// Read `n` continuation bytes (each `10xx_xxxx`) starting at `*i`, folding
/// them into `acc` at their shifted positions, and advance `*i`.
fn read_cont(bytes: &[u8], i: &mut usize, n: usize, acc: u32) -> Result<u32, ViaError> {
    let mut cp = acc;
    for k in 0..n {
        let idx = *i + k;
        let b = *bytes.get(idx).ok_or_else(|| truncated(idx))?;
        if b & 0xC0 != 0x80 {
            return Err(ViaError::Malformed {
                context: "nbt modified utf-8: missing continuation byte".to_string(),
                bytes_seen: idx,
            });
        }
        cp = (cp << 6) | (b & 0x3F) as u32;
    }
    *i += n;
    Ok(cp)
}

/// Shorthand for a truncated multi-byte sequence at `seen`.
fn truncated(seen: usize) -> ViaError {
    ViaError::Malformed {
        context: "nbt modified utf-8: truncated multi-byte sequence".to_string(),
        bytes_seen: seen,
    }
}

/// Shorthand for an invalid decoded code point.
fn malformed_cp(cp: u32, seen: usize) -> ViaError {
    ViaError::Malformed {
        context: format!("nbt modified utf-8: invalid code point 0x{:X}", cp),
        bytes_seen: seen,
    }
}

/// Encode a Rust `&str` as Java's *modified UTF-8* (for 1.8â€“1.15 network NBT
/// and other `DataOutput.writeUTF` sinks).
///
/// - U+0000 becomes `C0 80`;
/// - supplementary code points become CESU-8-style surrogate pairs (two
///   3-byte sequences), because Java's encoder cannot emit 4-byte sequences;
/// - everything else is standard UTF-8.
///
/// Infallible: a Rust `str` cannot contain lone surrogates, so every input
/// has a valid encoding.
///
/// RU: Ð·Ð°ÐºÐ¾Ð´Ð¸Ñ€Ð¾Ð²Ð°Ñ‚ÑŒ ÑÑ‚Ñ€Ð¾ÐºÑƒ Ð² modified UTF-8 (Java DataOutput).
pub fn encode_modified_utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp == 0 {
            // Embedded NUL: Java writes C0 80, never a bare 0x00.
            out.extend_from_slice(&[0xC0, 0x80]);
        } else if cp < 0x80 {
            out.push(cp as u8);
        } else if cp < 0x800 {
            out.push(0xC0 | (cp >> 6) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        } else if cp < 0x10000 {
            out.push(0xE0 | (cp >> 12) as u8);
            out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.push(0x80 | (cp & 0x3F) as u8);
        } else {
            // Supplementary plane: encode the UTF-16 surrogate pair as two
            // 3-byte sequences (CESU-8), exactly like DataOutputStream.
            let v = cp - 0x10000;
            let lead = 0xD800 + (v >> 10);
            let trail = 0xDC00 + (v & 0x3FF);
            for surr in [lead, trail] {
                out.push(0xE0 | (surr >> 12) as u8);
                out.push(0x80 | ((surr >> 6) & 0x3F) as u8);
                out.push(0x80 | (surr & 0x3F) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- strip_root_name / add_root_name --------------------------------

    /// Pre-1.20.2 root: TAG_Compound, name "", one child
    /// (named Byte "a" = 5), then End. Modern form starts at the child.
    #[test]
    fn strip_root_name_on_empty_named_compound() {
        // 0A 00 00 | 01 00 01 'a' 05 | 00
        let raw: &[u8] = &[0x0A, 0x00, 0x00, 0x01, 0x00, 0x01, b'a', 0x05, 0x00];
        let stripped = strip_root_name(raw).expect("valid root");
        assert_eq!(
            stripped,
            &[0x01, 0x00, 0x01, b'a', 0x05, 0x00],
            "returned slice is the root payload: first child tag + End"
        );
        // The payload must still be walkable as a compound body.
        let mut pos = 0usize;
        let tag = skip_named_tag(stripped, &mut pos).expect("child parses");
        assert_eq!(tag, TagType::Byte);
        assert_eq!(stripped[pos], 0x00, "next byte is the compound End");
    }

    #[test]
    fn strip_root_name_with_nonempty_root_name() {
        // 0A 00 03 'a' 'b' 'c' | payload...
        let raw: &[u8] = &[0x0A, 0x00, 0x03, b'a', b'b', b'c', 0x00];
        let stripped = strip_root_name(raw).expect("valid root");
        assert_eq!(stripped, &[0x00]);
    }

    #[test]
    fn strip_root_name_rejects_oversized_name() {
        // Name claims 5 bytes but only 1 byte follows.
        let raw: &[u8] = &[0x0A, 0x00, 0x05, 0x00];
        assert!(matches!(
            strip_root_name(raw),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn strip_root_name_rejects_bad_tag_id() {
        assert!(matches!(
            strip_root_name(&[0x0D, 0x00, 0x00]),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn strip_root_name_rejects_empty() {
        assert!(matches!(
            strip_root_name(&[]),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn add_root_name_round_trip() {
        let raw: &[u8] = &[0x0A, 0x00, 0x00, 0x01, 0x00, 0x01, b'a', 0x05, 0x00];
        let payload = strip_root_name(raw).expect("strip");
        assert_eq!(payload, &[0x01, 0x00, 0x01, b'a', 0x05, 0x00]);
        // Rebuild the modern form: root type id + payload.
        let mut modern = Vec::with_capacity(payload.len() + 1);
        modern.push(0x0A);
        modern.extend_from_slice(payload);
        let restored = add_root_name(&modern, "").expect("add");
        assert_eq!(restored.as_slice(), raw, "round trip must be identity");
    }

    #[test]
    fn add_root_name_inserts_name() {
        // Modern: 08 00 05 'h' 'e' 'l' 'l' 'o'  (String tag + payload).
        let modern: &[u8] = &[0x08, 0x00, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let legacy = add_root_name(modern, "root").expect("add");
        assert_eq!(
            legacy,
            &[0x08, 0x00, 0x04, b'r', b'o', b'o', b't', 0x00, 0x05, b'h', b'e', b'l', b'l', b'o']
        );
    }

    #[test]
    fn add_root_name_rejects_empty_and_long_names() {
        assert!(matches!(
            add_root_name(&[], ""),
            Err(ViaError::Malformed { .. })
        ));
        let long_name = "x".repeat(70_000);
        assert!(matches!(
            add_root_name(&[0x0A, 0x00], &long_name),
            Err(ViaError::Malformed { .. })
        ));
    }

    // --- uses_component_nbt ----------------------------------------------

    #[test]
    fn component_nbt_boundary() {
        assert!(!uses_component_nbt(47), "1.8 must not use component nbt");
        assert!(!uses_component_nbt(764), "1.20.2 boundary: still no");
        assert!(!uses_component_nbt(765), "1.20.4: still no");
        assert!(uses_component_nbt(766), "1.20.5 boundary: yes");
        assert!(uses_component_nbt(26_030));
    }

    // --- modified UTF-8 ---------------------------------------------------

    #[test]
    fn modified_utf8_embedded_nul_round_trip() {
        let s = "a\u{0}b";
        let enc = encode_modified_utf8(s);
        assert_eq!(enc, vec![b'a', 0xC0, 0x80, b'b']);
        let dec = decode_modified_utf8(&enc).expect("decode");
        assert_eq!(dec, s);
    }

    #[test]
    fn modified_utf8_ascii_is_identity() {
        let s = "plain ascii";
        assert_eq!(encode_modified_utf8(s), s.as_bytes());
        assert_eq!(
            decode_modified_utf8(s.as_bytes()).expect("decode"),
            s
        );
    }

    #[test]
    fn modified_utf8_two_three_byte_round_trip() {
        for s in ["\u{1DF}", "\u{E9}", "\u{414}", "\u{4F60}", "\u{1F600}", "\u{2603}"] {
            let enc = encode_modified_utf8(s);
            // None of these may contain a 4-byte sequence or C0 80.
            assert!(!enc.contains(&0xF0), "no 4-byte lead for {:?}", s);
            let dec = decode_modified_utf8(&enc).expect("decode");
            assert_eq!(dec, s, "round trip failed for {:?}", s);
        }
    }

    #[test]
    fn modified_utf8_supplementary_as_surrogate_pair() {
        // U+1F600 -> surrogates D83D DE00 -> two 3-byte sequences.
        let enc = encode_modified_utf8("\u{1F600}");
        assert_eq!(
            enc,
            vec![0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80],
            "must be CESU-8 style pair"
        );
        let dec = decode_modified_utf8(&enc).expect("decode");
        assert_eq!(dec, "\u{1F600}");
    }

    #[test]
    fn modified_utf8_rejects_truncated_sequence() {
        // 2-byte lead with no continuation.
        assert!(matches!(
            decode_modified_utf8(&[0xC3]),
            Err(ViaError::Malformed { .. })
        ));
        // 3-byte lead with only one continuation.
        assert!(matches!(
            decode_modified_utf8(&[0xE3, 0x81]),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn modified_utf8_rejects_four_byte_lead() {
        // Standard UTF-8 4-byte lead is never valid in modified UTF-8.
        assert!(matches!(
            decode_modified_utf8(&[0xF0, 0x9F, 0x98, 0x80]),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn modified_utf8_rejects_unpaired_surrogate() {
        // Lead surrogate D83D encoded alone (ED A0 BD), then ASCII.
        assert!(matches!(
            decode_modified_utf8(&[0xED, 0xA0, 0xBD, b'x']),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn modified_utf8_rejects_bad_continuation() {
        // 2-byte lead followed by a non-continuation byte.
        assert!(matches!(
            decode_modified_utf8(&[0xC3, 0x41]),
            Err(ViaError::Malformed { .. })
        ));
    }

    // --- TagType ------------------------------------------------------------

    #[test]
    fn tag_type_from_id_all_valid() {
        let expected = [
            TagType::End,
            TagType::Byte,
            TagType::Short,
            TagType::Int,
            TagType::Long,
            TagType::Float,
            TagType::Double,
            TagType::ByteArray,
            TagType::String,
            TagType::List,
            TagType::Compound,
            TagType::IntArray,
            TagType::LongArray,
        ];
        for (id, want) in expected.iter().enumerate() {
            assert_eq!(TagType::from_id(id as u8).expect("valid id"), *want);
        }
    }

    #[test]
    fn tag_type_from_id_rejects_out_of_range() {
        for bad in [13u8, 0xFF, 0x80] {
            assert!(
                matches!(TagType::from_id(bad), Err(ViaError::Malformed { .. })),
                "id {} must be rejected",
                bad
            );
        }
    }

    #[test]
    fn tag_type_to_id_inverts_from_id() {
        for id in 0u8..=12 {
            assert_eq!(TagType::from_id(id).expect("valid").to_id(), id);
        }
    }

    // --- skip_payload / skip_named_tag -----------------------------------

    /// Compound body: { "l": List<Int>[1, 2] } then End.
    /// Bytes: 09 00 01 'l' | 03 00 00 00 02 | 00 00 00 01 | 00 00 00 02 | 00
    #[test]
    fn skip_payload_nested_list_of_ints() {
        let buf: &[u8] = &[
            0x09, 0x00, 0x01, b'l', // named tag: type List, name "l"
            0x03, 0x00, 0x00, 0x00, 0x02, // element type Int, count 2
            0x00, 0x00, 0x00, 0x01, // int 1
            0x00, 0x00, 0x00, 0x02, // int 2
            0x00, // compound End
        ];
        // Walk the compound body: one named tag + End.
        let mut pos = 0usize;
        let tag = skip_named_tag(buf, &mut pos).expect("list tag parses");
        assert_eq!(tag, TagType::List);
        assert_eq!(pos, 17, "list tag consumed up to the End byte");
        assert_eq!(buf[pos], 0x00, "End byte left for the caller");
    }

    #[test]
    fn skip_payload_compound_with_nested_list() {
        // Build a full compound payload: { "l": List<Int>[1,2] } + End.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&[0x09, 0x00, 0x01, b'l']);
        buf.extend_from_slice(&[0x03, 0x00, 0x00, 0x00, 0x02]);
        buf.extend_from_slice(&0x01_i32.to_be_bytes());
        buf.extend_from_slice(&0x02_i32.to_be_bytes());
        buf.push(0x00);
        let mut pos = 0usize;
        skip_payload(&buf, &mut pos, TagType::Compound).expect("compound skips");
        assert_eq!(pos, buf.len(), "whole compound consumed");
    }

    #[test]
    fn skip_payload_all_primitive_types() {
        let buf: Vec<u8> = vec![0u8; 32];
        for (tag, want) in [
            (TagType::Byte, 1usize),
            (TagType::Short, 2),
            (TagType::Int, 4),
            (TagType::Float, 4),
            (TagType::Long, 8),
            (TagType::Double, 8),
        ] {
            let mut pos = 0usize;
            skip_payload(&buf, &mut pos, tag).expect("primitive skips");
            assert_eq!(pos, want, "{:?} payload size", tag);
        }
    }

    #[test]
    fn skip_payload_arrays_and_strings() {
        // ByteArray len 3.
        let mut pos = 0usize;
        skip_payload(&[0x00, 0x00, 0x00, 0x03, 1, 2, 3], &mut pos, TagType::ByteArray)
            .expect("byte array skips");
        assert_eq!(pos, 7);

        // String "hi".
        let mut pos = 0usize;
        skip_payload(&[0x00, 0x02, b'h', b'i'], &mut pos, TagType::String)
            .expect("string skips");
        assert_eq!(pos, 4);

        // IntArray count 2 -> 8 body bytes.
        let mut pos = 0usize;
        skip_payload(
            &[0x00, 0x00, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0],
            &mut pos,
            TagType::IntArray,
        )
        .expect("int array skips");
        assert_eq!(pos, 12);

        // LongArray count 1 -> 8 body bytes.
        let mut pos = 0usize;
        skip_payload(&[0x00, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0], &mut pos, TagType::LongArray)
            .expect("long array skips");
        assert_eq!(pos, 12);
    }

    #[test]
    fn skip_payload_empty_list_allows_zero_and_negative_count() {
        for count in [0i32, -1] {
            let mut buf: Vec<u8> = vec![TagType::Byte.to_id()];
            buf.extend_from_slice(&count.to_be_bytes());
            let mut pos = 0usize;
            skip_payload(&buf, &mut pos, TagType::List).expect("empty list skips");
            assert_eq!(pos, buf.len(), "count {}", count);
        }
    }

    #[test]
    fn skip_payload_rejects_list_of_end_with_positive_count() {
        let buf: Vec<u8> = vec![TagType::End.to_id(), 0, 0, 0, 5];
        let mut pos = 0usize;
        assert!(matches!(
            skip_payload(&buf, &mut pos, TagType::List),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn skip_payload_rejects_truncated_input() {
        // Int tag but no payload bytes.
        let mut pos = 0usize;
        assert!(matches!(
            skip_payload(&[0x03], &mut pos, TagType::Int),
            Err(ViaError::Malformed { .. })
        ));
        // Compound that never closes.
        let mut pos = 0usize;
        assert!(matches!(
            skip_payload(&[0x01, 0x00, 0x00, 0x01, b'a'], &mut pos, TagType::Compound),
            Err(ViaError::Malformed { .. })
        ));
    }

    #[test]
    fn skip_payload_rejects_excessive_nesting() {
        // 600 nested compounds (header 3 bytes each, then Ends).
        let mut buf: Vec<u8> = Vec::new();
        for _ in 0..600 {
            buf.extend_from_slice(&[0x0A, 0x00, 0x00]);
        }
        for _ in 0..600 {
            buf.push(0x00);
        }
        let mut pos = 0usize;
        let outcome = skip_payload(&buf, &mut pos, TagType::Compound);
        assert!(
            matches!(outcome, Err(ViaError::Malformed { .. })),
            "deep nesting must be rejected, not stack-overflow"
        );
    }

    #[test]
    fn skip_named_tag_end_consumes_one_byte() {
        let mut pos = 0usize;
        let tag = skip_named_tag(&[0x00, 0xFF], &mut pos).expect("end parses");
        assert_eq!(tag, TagType::End);
        assert_eq!(pos, 1, "End consumes only its id byte");
    }

    #[test]
    fn skip_named_tag_rejects_oversized_name() {
        // Name claims 4 bytes, only 2 present.
        let mut pos = 0usize;
        assert!(matches!(
            skip_named_tag(&[0x08, 0x00, 0x04, b'a', b'b'], &mut pos),
            Err(ViaError::Malformed { .. })
        ));
    }
}
