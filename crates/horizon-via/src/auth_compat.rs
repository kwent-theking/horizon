//! Login/auth compatibility for older protocols (online-mode servers).
//!
//! EN: this module knows the *shape* differences of the login phase between
//! old protocols and the client's 26.3 — nothing more. Two areas:
//!
//! 1. **Login Success profile shape.** Servers before 1.16 sent the player
//!    profile inside Login Success as two plain strings (uuid + name); 1.16+
//!    moved the UUID to a 4-int array; 1.19+ added a properties array
//!    (skin/cape textures). The *session* itself (the `join_server` POST to
//!    minecraftservices) is identical for every protocol — one and the same
//!    endpoint — so it lives outside this module. Offline mode simply skips
//!    all of this; that decision also belongs to the caller, not here.
//!
//! 2. **Keep-alive field type.** 1.8–1.11.2 carry a VarInt, 1.12+ a Long.
//!
//! RU: модуль знает различия фазы входа между старыми протоколами и 26.3:
//! форма профиля в Login Success (строки против int-array + properties) и тип
//! поля keep-alive (VarInt против Long). Сама сессия (join_server,
//! POST minecraftservices) одинакова для всех версий и живёт вне модуля;
//! offline-режим тоже решает вызывающий.

use crate::errors::ViaError;
use crate::frame::{read_varint, write_varint};

/// First protocol whose Login Success carries the UUID as an int array (1.16).
///
/// EN: 1.16 (735) switched the Login Success UUID from a dashed string to a
/// 4-int array. FIXME(orchestrator): verify the exact boundary — some snapshot
/// between 1.15.2 (578) and 1.16 (735) may share 735's shape.
///
/// RU: первый протокол с UUID int-array в Login Success.
pub const PROTOCOL_UUID_INT_ARRAY_MIN: i32 = 735;

/// First protocol whose Login Success carries a properties array (1.19).
///
/// EN: 1.19 (759) added the properties list (textures) to Login Success.
/// FIXME(orchestrator): verify the exact boundary (snapshots 75x).
///
/// RU: первый протокол с массивом properties в Login Success.
pub const PROTOCOL_LOGIN_PROPERTIES_MIN: i32 = 759;

/// Last protocol whose keep-alive carries a VarInt (1.11.2 = 316).
///
/// EN: 1.12 (335) changed the keep-alive payload from VarInt to Long.
/// FIXME(orchestrator): verify the exact boundary — the wiki table for the
/// 1.11.2 -> 1.12 step should be checked against ViaVersion's mappings.
///
/// RU: последний протокол с VarInt в keep-alive.
pub const PROTOCOL_KEEPALIVE_VARINT_MAX: i32 = 316;

/// First protocol whose keep-alive may carry an extra UID field (1.20.2).
///
/// EN: 1.20.2 (764) kept the Long payload but play-phase servers of the
/// newer era additionally send a client UID in *some* keep-alive packets.
/// This module deliberately implements only the Long/VarInt payloads; the
/// optional UID is the caller's business (see [`KeepaliveShape::LongAndUid`]).
///
/// RU: с 1.20.2 keep-alive опционально несёт ещё UID — не реализовано здесь.
pub const PROTOCOL_KEEPALIVE_UID_MIN: i32 = 764;

/// A player profile as old servers deliver it inside Login Success.
///
/// EN: pre-1.19 servers do not push the properties array at all, so
/// `properties` is simply empty for them; the caller may fill it from the
/// session API instead. Each property pair is `(name, value)` — in vanilla
/// the value is base64-encoded JSON, but this struct stores it verbatim and
/// does not interpret it.
///
/// RU: профиль игрока из Login Success старых серверов. До 1.19 массив
/// properties пуст — его может дополнить вызывающий из session API. Пары
/// `(имя, значение)` хранятся как есть (в ванилле значение — base64-JSON).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyProfile {
    /// Player name, e.g. `"Notch"`.
    /// RU: имя игрока.
    pub name: String,
    /// Player UUID, dashed lowercase, e.g. `"069a79f4-44e9-4726-a5be-fca90e38aaf5"`.
    /// RU: UUID игрока в dashed-lowercase формате.
    pub uuid: String,
    /// Profile properties (textures etc.), `(name, value)` pairs.
    /// RU: свойства профиля (текстуры и пр.).
    pub properties: Vec<(String, String)>,
}

/// The wire shape of the Login Success packet for a given protocol.
///
/// EN: selects how the UUID (and, implicitly, the profile) is laid out on
/// the wire in the login state. Used by the login-phase translation step to
/// rewrite a server-shaped Login Success into the client's 26.3 shape.
///
/// RU: форма пакета Login Success на проводе для данного протокола.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LoginSuccessShape {
    /// 1.7–1.15: both UUID and name are plain (dashed) strings.
    /// RU: UUID и имя — строки (до 1.16).
    StringNameAndUuid,
    /// 1.16–1.18: UUID as a 4-int array, name as a string, no properties.
    /// RU: UUID int-array + имя строкой, без properties (1.16–1.18).
    UuidIntArray,
    /// 1.19+: UUID as a 4-int array, name as a string, plus a properties
    /// array (skin/cape textures) — the modern shape, closest to 26.3.
    /// RU: UUID int-array + имя строкой + массив properties (1.19+).
    StringNameUuidIntArray,
}

/// Login Success shape for a protocol number.
///
/// EN: 1.7–1.15 (`< 735`) use two strings; 1.16–1.18 (`735..=758`) use an
/// int-array UUID plus a string name; 1.19+ (`>= 759`) add properties.
/// Unknown protocols (below the 1.8 era, negative sentinels) map to
/// [`LoginSuccessShape::StringNameAndUuid`] — the most permissive shape, so
/// a garbled number degrades to a parse error instead of a wrong layout.
///
/// RU: форма Login Success по номеру протокола; unknown — обе строки
/// (самая безопасная деградация).
///
/// FIXME(orchestrator): exact 1.15.2/1.16 and 1.18.2/1.19 boundaries must
/// be verified against ViaVersion's mappings; the constants above carry
/// the current best-effort numbers.
pub fn login_success_fields(protocol: i32) -> LoginSuccessShape {
    if protocol >= PROTOCOL_LOGIN_PROPERTIES_MIN {
        LoginSuccessShape::StringNameUuidIntArray
    } else if protocol >= PROTOCOL_UUID_INT_ARRAY_MIN {
        LoginSuccessShape::UuidIntArray
    } else {
        // Includes every protocol below 735 and all unknown/sentinel values.
        LoginSuccessShape::StringNameAndUuid
    }
}

/// Whether the Login Success UUID travels as a 4-int array at this protocol.
///
/// EN: true for 1.16+ (protocol `>= 735`), false otherwise (string UUID).
///
/// RU: true, если UUID в Login Success идёт int-array (1.16+).
pub fn uses_int_array_uuid(protocol: i32) -> bool {
    protocol >= PROTOCOL_UUID_INT_ARRAY_MIN
}

/// Encode a UUID for the wire at a given protocol.
///
/// EN: for int-array shapes the string must be a valid UUID — dashed or
/// undashed, exactly 32 hex digits — and is emitted as 16 raw bytes in
/// standard UUID byte order. For the string shape the input is emitted as
/// its ASCII bytes verbatim (the legacy wire format carries the dashed
/// string as a normal chat-style string; non-ASCII input is rejected as
/// malformed user data, since a legacy login string cannot carry it).
///
/// RU: для int-array — 16 байт из hex (с дэшами или без, 32 hex-символа,
/// невалидное → `Malformed`); для строки — ASCII-байты как есть.
pub fn encode_uuid(uuid: &str, protocol: i32) -> Result<Vec<u8>, ViaError> {
    if !uses_int_array_uuid(protocol) {
        // Legacy string shape: bytes as-is, but only ASCII is representable.
        if uuid.is_ascii() {
            Ok(uuid.as_bytes().to_vec())
        } else {
            Err(ViaError::Malformed {
                context: "legacy login uuid is not ASCII".to_string(),
                bytes_seen: 0,
            })
        }
    } else {
        hex_to_uuid_bytes(uuid)
    }
}

/// Decode a UUID from the wire at a given protocol.
///
/// EN: for int-array shapes the input must be exactly 16 bytes and is
/// rendered as a dashed lowercase UUID string. For the string shape the
/// bytes are read as UTF-8 and trimmed (some old servers pad the field).
///
/// RU: 16 байт → dashed-lowercase UUID; строка → UTF-8 с обрезкой пробелов.
pub fn decode_uuid(bytes: &[u8], protocol: i32) -> Result<String, ViaError> {
    if !uses_int_array_uuid(protocol) {
        let s = String::from_utf8(bytes.to_vec()).map_err(|_| ViaError::Malformed {
            context: "legacy login uuid is not valid UTF-8".to_string(),
            bytes_seen: bytes.len(),
        })?;
        Ok(s.trim().to_string())
    } else {
        if bytes.len() != 16 {
            return Err(ViaError::Malformed {
                context: format!(
                    "int-array login uuid must be 16 bytes, got {}",
                    bytes.len()
                ),
                bytes_seen: bytes.len(),
            });
        }
        let mut out = String::with_capacity(36);
        for (i, b) in bytes.iter().enumerate() {
            if i == 4 || i == 6 || i == 8 || i == 10 {
                out.push('-');
            }
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0F));
        }
        Ok(out)
    }
}

/// The wire shape of the keep-alive payload for a given protocol.
///
/// EN: only the Long/VarInt payloads are implemented here (see
/// [`KeepaliveShape::LongAndUid`] for the optional extra field).
///
/// RU: форма поля keep-alive; реализованы только Long/VarInt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeepaliveShape {
    /// 1.8–1.11.2: a VarInt payload.
    /// RU: VarInt (1.8–1.11.2).
    VarInt,
    /// 1.12–1.20.1: a Long (i64) payload.
    /// RU: Long (1.12–1.20.1).
    Long,
    /// 1.20.2+: still a Long payload, but newer play-phase servers
    /// additionally send a client UID in some keep-alives. That field is
    /// OPTIONAL and is deliberately not implemented in this module —
    /// encode/decode treat this shape exactly like [`KeepaliveShape::Long`],
    /// and the caller-side login/uid policy lives elsewhere.
    ///
    /// RU: Long + опциональное поле UID новых серверов; здесь не реализовано,
    /// кодируется/декодируется как Long.
    LongAndUid,
}

/// Keep-alive shape for a protocol number.
///
/// EN: `< 335` (1.8–1.11.2, and anything older) → VarInt; `335..=763`
/// (1.12–1.20.1) → Long; `>= 764` (1.20.2+) → LongAndUid (payload Long,
/// optional UID ignored here). Unknown protocols fall into the Long-family
/// buckets by number comparison — the modern default.
///
/// RU: форма keep-alive по номеру протокола; unknown попадает в
/// Long-семейство по диапазону числа.
///
/// FIXME(orchestrator): the 1.11.2/1.12 boundary (316 vs 335) must be
/// checked against ViaVersion's keep-alive mappings.
pub fn keepalive_shape(protocol: i32) -> KeepaliveShape {
    if protocol <= PROTOCOL_KEEPALIVE_VARINT_MAX {
        KeepaliveShape::VarInt
    } else if protocol < PROTOCOL_KEEPALIVE_UID_MIN {
        KeepaliveShape::Long
    } else {
        KeepaliveShape::LongAndUid
    }
}

/// Encode a keep-alive payload value for the wire at a given protocol.
///
/// EN: the VarInt branch requires the value to fit in `i32` — a larger
/// server timestamp is a translation overflow, reported as
/// [`ViaError::TranslationOverflow`]. The Long/LongAndUid branches emit 8
/// big-endian bytes (LongAndUid carries no UID here, see
/// [`KeepaliveShape::LongAndUid`]).
///
/// RU: VarInt-ветка: значение не влезает в i32 → `TranslationOverflow`;
/// Long — 8 байт big-endian.
pub fn encode_keepalive(value: i64, protocol: i32) -> Result<Vec<u8>, ViaError> {
    match keepalive_shape(protocol) {
        KeepaliveShape::VarInt => {
            let v = i32::try_from(value).map_err(|_| ViaError::TranslationOverflow {
                what: "keepalive varint payload does not fit i32",
            })?;
            let mut out = Vec::with_capacity(5);
            write_varint(v, &mut out);
            Ok(out)
        }
        KeepaliveShape::Long | KeepaliveShape::LongAndUid => {
            Ok(value.to_be_bytes().to_vec())
        }
    }
}

/// Decode a keep-alive payload from the wire at a given protocol.
///
/// EN: the VarInt branch reads exactly one VarInt and rejects trailing
/// bytes; the Long/LongAndUid branches require exactly 8 big-endian bytes.
///
/// RU: VarInt — ровно один VarInt без хвоста; Long — ровно 8 байт BE.
pub fn decode_keepalive(bytes: &[u8], protocol: i32) -> Result<i64, ViaError> {
    match keepalive_shape(protocol) {
        KeepaliveShape::VarInt => {
            let mut pos = 0usize;
            let v = read_varint(bytes, &mut pos)?;
            if pos != bytes.len() {
                return Err(ViaError::Malformed {
                    context: format!(
                        "keepalive varint has {} trailing byte(s)",
                        bytes.len() - pos
                    ),
                    bytes_seen: pos,
                });
            }
            Ok(i64::from(v))
        }
        KeepaliveShape::Long | KeepaliveShape::LongAndUid => {
            if bytes.len() != 8 {
                return Err(ViaError::Malformed {
                    context: format!(
                        "keepalive long must be 8 bytes, got {}",
                        bytes.len()
                    ),
                    bytes_seen: bytes.len(),
                });
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(bytes);
            Ok(i64::from_be_bytes(arr))
        }
    }
}

/// Parse a UUID string (dashed or undashed, 32 hex digits) into 16 bytes.
///
/// EN: internal helper for the int-array shapes. Any other shape, length or
/// character is [`ViaError::Malformed`] — never a panic.
///
/// RU: 32 hex-символа (с дэшами или без) → 16 байт; иначе `Malformed`.
fn hex_to_uuid_bytes(uuid: &str) -> Result<Vec<u8>, ViaError> {
    let mut digits = Vec::with_capacity(32);
    for c in uuid.chars() {
        if c == '-' {
            continue;
        }
        match c.to_digit(16) {
            Some(d) => digits.push(d as u8),
            None => {
                return Err(ViaError::Malformed {
                    context: format!("invalid hex digit in login uuid: {:?}", c),
                    bytes_seen: 0,
                })
            }
        }
    }
    if digits.len() != 32 {
        return Err(ViaError::Malformed {
            context: format!(
                "login uuid must have 32 hex digits, got {}",
                digits.len()
            ),
            bytes_seen: 0,
        });
    }
    let mut out = Vec::with_capacity(16);
    for pair in digits.chunks(2) {
        out.push((pair[0] << 4) | pair[1]);
    }
    Ok(out)
}

/// Lowercase hex digit for a nibble (0..=15).
/// RU: hex-символ младшей половины байта.
fn hex_digit(nibble: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    HEX[nibble as usize] as char
}

#[cfg(test)]
mod tests {
    use super::*;

    const P_1_8: i32 = 47;
    const P_1_16_5: i32 = 754;
    const P_MODERN: i32 = 999_999; // VIA_TARGET placeholder (26.3)
    const UUID_DASHED: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
    const UUID_UNDASHED: &str = "069a79f444e94726a5befca90e38aaf5";
    const UUID_BYTES: [u8; 16] = [
        0x06, 0x9a, 0x79, 0xf4, 0x44, 0xe9, 0x47, 0x26, 0xa5, 0xbe, 0xfc, 0xa9, 0x0e, 0x38,
        0xaa, 0xf5,
    ];

    // ---- login_success_fields ------------------------------------------

    #[test]
    fn login_success_shape_per_era() {
        assert_eq!(
            login_success_fields(P_1_8),
            LoginSuccessShape::StringNameAndUuid
        );
        assert_eq!(
            login_success_fields(P_1_16_5),
            LoginSuccessShape::UuidIntArray
        );
        assert_eq!(
            login_success_fields(P_MODERN),
            LoginSuccessShape::StringNameUuidIntArray
        );
    }

    #[test]
    fn login_success_shape_boundaries() {
        // 1.15.2 -> 1.16 boundary
        assert_eq!(
            login_success_fields(578),
            LoginSuccessShape::StringNameAndUuid
        );
        assert_eq!(login_success_fields(735), LoginSuccessShape::UuidIntArray);
        // 1.18.2 -> 1.19 boundary
        assert_eq!(login_success_fields(758), LoginSuccessShape::UuidIntArray);
        assert_eq!(
            login_success_fields(759),
            LoginSuccessShape::StringNameUuidIntArray
        );
        // unknown -> safest shape
        assert_eq!(
            login_success_fields(-1),
            LoginSuccessShape::StringNameAndUuid
        );
        assert_eq!(
            login_success_fields(0),
            LoginSuccessShape::StringNameAndUuid
        );
    }

    #[test]
    fn uses_int_array_uuid_matches_shape() {
        assert!(!uses_int_array_uuid(P_1_8));
        assert!(!uses_int_array_uuid(578));
        assert!(uses_int_array_uuid(735));
        assert!(uses_int_array_uuid(P_1_16_5));
        assert!(uses_int_array_uuid(P_MODERN));
    }

    // ---- uuid encode/decode ---------------------------------------------

    #[test]
    fn uuid_roundtrip_int_array_dashed() {
        let enc = encode_uuid(UUID_DASHED, P_1_16_5).expect("encode must succeed");
        assert_eq!(enc, UUID_BYTES.to_vec());
        let dec = decode_uuid(&enc, P_1_16_5).expect("decode must succeed");
        assert_eq!(dec, UUID_DASHED);
    }

    #[test]
    fn uuid_roundtrip_int_array_undashed() {
        let enc = encode_uuid(UUID_UNDASHED, P_MODERN).expect("encode must succeed");
        assert_eq!(enc, UUID_BYTES.to_vec());
        let dec = decode_uuid(&enc, P_MODERN).expect("decode must succeed");
        // decode always produces the dashed canonical form
        assert_eq!(dec, UUID_DASHED);
    }

    #[test]
    fn uuid_roundtrip_from_raw_bytes() {
        let dec = decode_uuid(&UUID_BYTES, P_1_16_5).expect("decode must succeed");
        assert_eq!(dec, UUID_DASHED);
        let enc = encode_uuid(&dec, P_MODERN).expect("encode must succeed");
        assert_eq!(enc, UUID_BYTES.to_vec());
    }

    #[test]
    fn uuid_string_shape_roundtrip() {
        let enc = encode_uuid(UUID_DASHED, P_1_8).expect("encode must succeed");
        assert_eq!(enc, UUID_DASHED.as_bytes().to_vec());
        let dec = decode_uuid(&enc, P_1_8).expect("decode must succeed");
        assert_eq!(dec, UUID_DASHED);
    }

    #[test]
    fn uuid_string_shape_trims_padding() {
        let dec = decode_uuid(b"  069a79f4-44e9-4726-a5be-fca90e38aaf5  ", P_1_8)
            .expect("decode must succeed");
        assert_eq!(dec, UUID_DASHED);
    }

    #[test]
    fn uuid_invalid_hex_rejected() {
        let err = encode_uuid("zzzz79f4-44e9-4726-a5be-fca90e38aaf5", P_1_16_5)
            .expect_err("non-hex must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    #[test]
    fn uuid_wrong_length_rejected() {
        // 31 hex digits
        let err = encode_uuid("069a79f444e94726a5befca90e38aaf", P_1_16_5)
            .expect_err("wrong length must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
        // double uuid (64 digits) is also wrong
        let err = encode_uuid(&format!("{}{}", UUID_UNDASHED, UUID_UNDASHED), P_MODERN)
            .expect_err("too long must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    #[test]
    fn uuid_decode_wrong_byte_count_rejected() {
        let err = decode_uuid(&UUID_BYTES[..15], P_1_16_5).expect_err("15 bytes must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
        let err = decode_uuid(&[0u8; 17], P_MODERN).expect_err("17 bytes must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    #[test]
    fn uuid_non_ascii_string_shape_rejected() {
        let err = encode_uuid("не-ascii", P_1_8).expect_err("non-ASCII must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    #[test]
    fn uuid_decode_bad_utf8_string_shape_rejected() {
        let err = decode_uuid(&[0xff, 0xfe], P_1_8).expect_err("bad UTF-8 must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    // ---- keepalive -------------------------------------------------------

    #[test]
    fn keepalive_shape_per_era() {
        assert_eq!(keepalive_shape(P_1_8), KeepaliveShape::VarInt);
        assert_eq!(keepalive_shape(316), KeepaliveShape::VarInt); // 1.11.2
        assert_eq!(keepalive_shape(335), KeepaliveShape::Long); // 1.12
        assert_eq!(keepalive_shape(763), KeepaliveShape::Long); // 1.20.1
        assert_eq!(keepalive_shape(764), KeepaliveShape::LongAndUid); // 1.20.2
        assert_eq!(keepalive_shape(P_MODERN), KeepaliveShape::LongAndUid);
    }

    #[test]
    fn keepalive_varint_roundtrip() {
        for v in [0i64, 1, 127, 128, 255, 300, 32_767, 1 << 20, i32::MAX as i64] {
            let enc = encode_keepalive(v, P_1_8).expect("encode must succeed");
            let dec = decode_keepalive(&enc, P_1_8).expect("decode must succeed");
            assert_eq!(dec, v, "roundtrip failed for {}", v);
        }
    }

    #[test]
    fn keepalive_varint_wire_bytes() {
        // golden bytes: VarInt 1 = [0x01]
        assert_eq!(encode_keepalive(1, P_1_8).unwrap(), vec![0x01]);
        // VarInt 128 = [0x80, 0x01]
        assert_eq!(encode_keepalive(128, P_1_8).unwrap(), vec![0x80, 0x01]);
    }

    #[test]
    fn keepalive_varint_overflow() {
        let err = encode_keepalive(i32::MAX as i64 + 1, P_1_8)
            .expect_err("value above i32 must overflow");
        assert!(
            matches!(err, ViaError::TranslationOverflow { .. }),
            "got {:?}",
            err
        );
        let err = encode_keepalive(i64::MIN, 316).expect_err("i64::MIN must overflow");
        assert!(
            matches!(err, ViaError::TranslationOverflow { .. }),
            "got {:?}",
            err
        );
    }

    #[test]
    fn keepalive_varint_trailing_bytes_rejected() {
        let err = decode_keepalive(&[0x01, 0x00], P_1_8).expect_err("trailing byte must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    #[test]
    fn keepalive_long_roundtrip() {
        for v in [0i64, 1, 1_000_000, i64::MAX, i64::MIN, -42] {
            let enc = encode_keepalive(v, P_1_16_5).expect("encode must succeed");
            assert_eq!(enc.len(), 8);
            let dec = decode_keepalive(&enc, P_1_16_5).expect("decode must succeed");
            assert_eq!(dec, v, "roundtrip failed for {}", v);
        }
    }

    #[test]
    fn keepalive_long_wire_bytes() {
        // golden bytes: Long 1 = 8x big-endian
        assert_eq!(
            encode_keepalive(1, P_1_16_5).unwrap(),
            vec![0, 0, 0, 0, 0, 0, 0, 1]
        );
    }

    #[test]
    fn keepalive_longanduid_behaves_like_long() {
        // LongAndUid: only the Long payload is implemented; the optional UID
        // is not emitted or consumed.
        let enc = encode_keepalive(777, P_MODERN).expect("encode must succeed");
        assert_eq!(enc.len(), 8);
        let dec = decode_keepalive(&enc, P_MODERN).expect("decode must succeed");
        assert_eq!(dec, 777);
    }

    #[test]
    fn keepalive_long_wrong_byte_count_rejected() {
        let err = decode_keepalive(&[0u8; 7], P_1_16_5).expect_err("7 bytes must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
        let err = decode_keepalive(&[0u8; 9], P_MODERN).expect_err("9 bytes must fail");
        assert!(matches!(err, ViaError::Malformed { .. }), "got {:?}", err);
    }

    // ---- LegacyProfile sanity --------------------------------------------

    #[test]
    fn legacy_profile_holds_fields() {
        let p = LegacyProfile {
            name: "Notch".to_string(),
            uuid: UUID_DASHED.to_string(),
            properties: vec![(
                "textures".to_string(),
                "eyJ0ZXh0dXJlcyI6e30".to_string(),
            )],
        };
        assert_eq!(p.name, "Notch");
        assert_eq!(p.uuid, UUID_DASHED);
        assert_eq!(p.properties.len(), 1);
        assert_eq!(p.properties[0].0, "textures");
    }

    #[test]
    fn legacy_profile_pre_1_19_has_no_properties_on_wire() {
        // Documented behavior: below 1.19 the wire shape has no properties
        // array, so a decoded profile carries an empty vec.
        assert!(!matches!(
            login_success_fields(758),
            LoginSuccessShape::StringNameUuidIntArray
        ));
    }
}
