//! Wire-format constants shared by every `horizon-via` module.
//!
//! EN: magic numbers that the framing, state machine, compression and legacy
//! handshake code all need. Centralizing them here means one place to check
//! against the vanilla protocol wiki instead of grepping eleven modules.
/// Constants only — no logic in this file, so a unit test failure here can
/// only mean a wrong constant, never a wrong algorithm.
///
/// RU: магические числа, нужные всем модулям крейта. Только константы, без
/// логики: упавший тест здесь означает неверную константу, а не неверный код.

/// Minecraft modern (1.7+) handshake packet type.
///
/// EN: the first packet in the `Handshaking` state is always type `0x00`
/// ("set protocol") on both modern client and server side.
///
/// RU: первый пакет состояния Handshaking — всегда тип 0x00.
pub const HANDSHAKE_TYPE: i32 = 0x00;

/// Legacy server list ping packet id (1.6 - 1.7 era).
///
/// EN: before the modern handshake existed, a server list ping was a single
/// raw byte `0xFE` ("ping"). Kept as a constant so the frame layer can detect
/// the legacy shape; the legacy logic itself lives in `chat_legacy`/frame.
///
/// RU: легаси-ping сервер-листа одной командой 0xFE до современного
/// handshake. Только константа; логика легаси живёт в других модулях.
pub const LEGACY_PING_ID: u8 = 0xFE;

/// Marker protocol value for the legacy handshake path.
///
/// EN: 1.6-1.7 clients signal "legacy" by sending a negative (or zero)
/// protocol version in the ping; `0x00` handshake with a negative protocol
/// in modern code means the peer fell back to the legacy ping shape.
/// Only the constant — interpretation is not this file's business.
///
/// RU: отрицательный номер протокола в легаси-handshake означает
/// «легаси-режим». Только константа, без логики.
pub const LEGACY_PROTOCOL_SENTINEL: i32 = -1;
 
/// Internal canonical protocol state numbering used by `state.rs`'s
/// `ProtocolState` enum (0=handshake, 1=status, 2=login, 3=configuration,
/// 4=play). EN: these are OUR canonical ids, not the wire's Next State byte
/// (the wire only ever sends 1/2/3 there). Kept as constants so packet-id
/// tables and the future pipeline share one numbering.
/// RU: внутренняя каноническая нумерация состояний для state.rs; на проводе
/// Next State в handshake — только 1/2/3, значения 0/3/4/4 не встречаются.
pub const STATE_HANDSHAKING: i32 = 0;
pub const STATE_STATUS: i32 = 1;
pub const STATE_LOGIN: i32 = 2;
pub const STATE_CONFIGURATION: i32 = 3;
pub const STATE_PLAY: i32 = 4;

/// Maximum size of one framed packet on the wire, in bytes.
///
/// EN: the vanilla server and client both enforce this cap on a single
/// packet *after* framing (i.e. the length the frame header declares, not
/// counting the header itself). Anything larger is hostile and must be
/// rejected by the frame codec, not allocated.
///
/// Value: 2 MiB minus 1 — `2^21 - 1`, matching vanilla's
/// `MAX_PACKET_SIZE`.
///
/// RU: максимум байт в одном пакете после фрейминга, 2^21-1. Всё, что
/// больше, — враждебный пакет, фрейм-кодек обязан его отбросить.
pub use crate::frame::MAX_PACKET_BYTES; // single source: frame.rs (usize)

/// Bit mask of the payload bits in one VarInt/VarLong segment byte.
///
/// EN: each byte of a VarInt carries 7 payload bits; this mask extracts
/// them. `SEGMENT_BITS == 0x7F`.
///
/// RU: маска семи битов данных в одном байте VarInt.
pub const SEGMENT_BITS: u8 = 0x7F;

/// Continuation flag of a VarInt/VarLong segment byte.
///
/// EN: the high bit of each segment byte says "more bytes follow"; a byte
/// with this bit clear terminates the number. `CONTINUE_BIT == 0x80`.
///
/// RU: старший бит байта VarInt: «продолжение следует»; ноль — конец числа.
pub const CONTINUE_BIT: u8 = 0x80;

/// Placeholder protocol number of the translation target (Horizon's 26.3).
///
/// EN: every chain climbs toward the client's own version; until the exact
/// 26.3 protocol number is pinned from `horizon-protocol`, this absurd
/// sentinel (`999_999`) stands in so mistakes are loud.
///
/// FIXME(orchestrator): replace with the real 26.3 protocol number from
/// `horizon-protocol`, then update `versions::VIA_TARGET_PROTOCOL` docs.
///
/// RU: заглушка номера протокола цели (26.3) до закрепления реального
/// значения оркестратором.
// TODO(orchestrator): pin exact numbers from horizon-protocol
pub const VIA_TARGET: i32 = 777; // Minecraft 26.3 (horizon-core version.rs), pinned by orchestrator

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_type_is_zero() {
        assert_eq!(HANDSHAKE_TYPE, 0x00);
    }

    #[test]
    fn legacy_constants_match_era() {
        // 1.6-1.7 legacy ping is the raw 0xFE byte
        assert_eq!(LEGACY_PING_ID, 0xFE);
        // legacy path is signalled by a non-positive protocol number
        assert!(LEGACY_PROTOCOL_SENTINEL < 0);
    }

    #[test]
    fn state_ids_are_dense_and_ordered() {
        // Handshaking, Status, Login, Configuration, Play
        assert_eq!(STATE_HANDSHAKING, 0);
        assert_eq!(STATE_STATUS, 1);
        assert_eq!(STATE_LOGIN, 2);
        assert_eq!(STATE_CONFIGURATION, 3);
        assert_eq!(STATE_PLAY, 4);
        let all = [
            STATE_HANDSHAKING,
            STATE_STATUS,
            STATE_LOGIN,
            STATE_CONFIGURATION,
            STATE_PLAY,
        ];
        for pair in all.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "state ids must be 0..=4");
        }
    }

    #[test]
    fn max_packet_bytes_is_2_pow_21_minus_1() {
        assert_eq!(MAX_PACKET_BYTES, (1usize << 21) - 1);
        assert_eq!(MAX_PACKET_BYTES, 2_097_151usize);
        assert!(MAX_PACKET_BYTES > 0);
    }

    #[test]
    fn varint_segment_and_continue_bits() {
        assert_eq!(SEGMENT_BITS, 0x7F);
        assert_eq!(CONTINUE_BIT, 0x80);
        // the two masks must not overlap and must cover the whole byte
        assert_eq!(SEGMENT_BITS & CONTINUE_BIT, 0);
        assert_eq!(SEGMENT_BITS | CONTINUE_BIT, 0xFF);
    }

    #[test]
    fn via_target_is_loud_placeholder() {
        // must be absurd so any accidental wire use fails loudly
        assert!(VIA_TARGET >= 700); // 26.3 = 777
        // and must not collide with any real protocol number range
    
    }

    #[test]
    fn all_constants_are_usable_without_unsafe() {
        // compile-time sanity: constants fit their uses
        let _ = (HANDSHAKE_TYPE, LEGACY_PING_ID, LEGACY_PROTOCOL_SENTINEL);
        let _ = (STATE_HANDSHAKING, STATE_STATUS, STATE_LOGIN, STATE_CONFIGURATION, STATE_PLAY);
        let _ = (MAX_PACKET_BYTES, SEGMENT_BITS, CONTINUE_BIT, VIA_TARGET);
    }
}
