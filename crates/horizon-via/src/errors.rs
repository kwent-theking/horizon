//! Error type for all of `horizon-via`.
//!
//! Every module in this crate returns `Result<_, ViaError>`. The design rule
//! from `lib.rs` applies: a malformed or hostile packet must never panic the
//! client — it becomes a `ViaError`, the packet is dropped (or the session is
//! torn down, for the fatal variants), and the caller decides what to do.
//!
//! RU: тип ошибок для всего крейта. Искажённый пакет от враждебного сервера
//! никогда не должен паниковать — он превращается в `ViaError`, и вызывающий
//! решает: дропнуть пакет с логом или разорвать сессию.

use std::fmt;

/// Errors produced while translating packets between protocol versions.
///
/// Two variants are *fatal* (see [`ViaError::is_fatal`]): `Io` and
/// `OversizedPacket`. Everything else describes a single bad packet or a
/// translation dead end and is normally handled by dropping that packet and
/// logging — the session lives on.
///
/// RU: ошибки трансляции пакетов. Фатальны только `Io` и `OversizedPacket`;
/// прочее — дроп одного пакета с логом, сессия продолжается.
#[derive(Debug)]
pub enum ViaError {
    /// A packet failed to parse: the byte stream did not match the expected
    /// layout for the protocol version being decoded.
    ///
    /// `context` names what was being parsed (e.g. `"varint packet id"`),
    /// `bytes_seen` is how far the decoder got — enough to log a useful
    /// one-line diagnostic without dumping the whole buffer.
    ///
    /// RU: пакет не разобрался; контекст + сколько байт успели прочитать.
    Malformed {
        /// What the decoder was trying to read when it failed.
        context: String,
        /// Number of bytes consumed before the failure.
        bytes_seen: usize,
    },
    /// The connection announced a protocol version the translator has no
    /// step chain for (e.g. a snapshot between two known releases).
    ///
    /// RU: версия протокола, для которой нет цепочки шагов.
    UnsupportedVersion {
        /// The protocol number announced in the handshake.
        protocol: i32,
    },
    /// The packet id is unknown *in this state and version* — either the
    /// server is broken, or our id tables have a gap.
    ///
    /// RU: неизвестный id пакета в данном состоянии и версии.
    UnsupportedPacket {
        /// Connection state the packet arrived in, e.g. `"play"`.
        state: &'static str,
        /// Wire packet id (already unmapped from the 26.3 space if applicable).
        id: u32,
        /// Protocol version the packet was read at.
        version: i32,
    },
    /// A registry lookup failed (item/block/entity/channel names).
    /// Translation cannot continue for this packet without that entry.
    ///
    /// RU: отсутствующий ключ в реестре (предметы/блоки/каналы).
    MissingRegistry {
        /// The registry key that could not be resolved.
        key: String,
    },
    /// A packet (or a translated artifact) exceeded the size limit and was
    /// rejected before buffering it. Fatal: an unbounded peer could OOM the
    /// client otherwise.
    ///
    /// RU: пакет превысил лимит размера. Фатально, иначе пирами память.
    OversizedPacket {
        /// Actual size in bytes.
        size: usize,
        /// The configured maximum in bytes.
        limit: usize,
    },
    /// A translation step would produce more output than the buffer can hold
    /// (e.g. expanding a legacy string into 26.3 components). Distinct from
    /// `OversizedPacket`: this is our own arithmetic refusing, not the peer.
    ///
    /// RU: выход трансляции не влезает в буфер (наша арифметика отказала).
    TranslationOverflow {
        /// What was being expanded, e.g. `"legacy chat -> 26.3 components"`.
        what: &'static str,
    },
    /// Wrapped I/O error from the socket layer.
    /// Fatal: the transport is gone, the session must stop.
    ///
    /// RU: ошибка сокета. Фатально — транспорт умер.
    Io(std::io::Error),
}

impl From<std::io::Error> for ViaError {
    fn from(e: std::io::Error) -> Self {
        ViaError::Io(e)
    }
}

impl fmt::Display for ViaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViaError::Malformed { context, bytes_seen } => {
                write!(f, "malformed packet: {} ({} bytes consumed)", context, bytes_seen)
            }
            ViaError::UnsupportedVersion { protocol } => {
                write!(f, "unsupported protocol version {}", protocol)
            }
            ViaError::UnsupportedPacket { state, id, version } => {
                write!(
                    f,
                    "unsupported packet id {} in state {} at protocol {}",
                    id, state, version
                )
            }
            ViaError::MissingRegistry { key } => {
                write!(f, "missing registry entry: {}", key)
            }
            ViaError::OversizedPacket { size, limit } => {
                write!(f, "packet size {} exceeds limit {}", size, limit)
            }
            ViaError::TranslationOverflow { what } => {
                write!(f, "translation overflow while expanding {}", what)
            }
            ViaError::Io(e) => write!(f, "io error: {}", e),
        }
    }
}

impl std::error::Error for ViaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ViaError::Io(e) => Some(e),
            // No lower-level source: these are leaf diagnostics.
            _ => None,
        }
    }
}

impl ViaError {
    /// Whether the caller must tear down the session.
    ///
    /// Only [`ViaError::Io`] and [`ViaError::OversizedPacket`] are fatal: a
    /// dead transport cannot recover, and a peer that ignores size limits is
    /// hostile by definition. Every other variant describes one bad packet
    /// and is handled by dropping it with a log line.
    ///
    /// RU: фатально ли. Только `Io` и `OversizedPacket`; прочее — дроп пакета.
    pub fn is_fatal(&self) -> bool {
        matches!(self, ViaError::Io(_) | ViaError::OversizedPacket { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_malformed() {
        let e = ViaError::Malformed {
            context: "varint packet id".to_string(),
            bytes_seen: 3,
        };
        assert_eq!(
            e.to_string(),
            "malformed packet: varint packet id (3 bytes consumed)"
        );
    }

    #[test]
    fn display_unsupported_version() {
        let e = ViaError::UnsupportedVersion { protocol: 999_999 };
        assert_eq!(e.to_string(), "unsupported protocol version 999999");
    }

    #[test]
    fn display_unsupported_packet() {
        let e = ViaError::UnsupportedPacket {
            state: "play",
            id: 0x1f,
            version: 47,
        };
        assert_eq!(
            e.to_string(),
            "unsupported packet id 31 in state play at protocol 47"
        );
    }

    #[test]
    fn display_missing_registry() {
        let e = ViaError::MissingRegistry {
            key: "minecraft:ziglin".to_string(),
        };
        assert_eq!(e.to_string(), "missing registry entry: minecraft:ziglin");
    }

    #[test]
    fn display_oversized() {
        let e = ViaError::OversizedPacket {
            size: 20_000_000,
            limit: 2_097_151,
        };
        assert_eq!(
            e.to_string(),
            "packet size 20000000 exceeds limit 2097151"
        );
    }

    #[test]
    fn display_translation_overflow() {
        let e = ViaError::TranslationOverflow {
            what: "legacy chat -> 26.3 components",
        };
        assert_eq!(
            e.to_string(),
            "translation overflow while expanding legacy chat -> 26.3 components"
        );
    }

    #[test]
    fn display_io() {
        let e = ViaError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "eof",
        ));
        assert_eq!(e.to_string(), "io error: eof");
    }

    #[test]
    fn io_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let e: ViaError = io_err.into();
        assert!(matches!(e, ViaError::Io(_)));
    }

    #[test]
    fn io_is_fatal() {
        let e = ViaError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(e.is_fatal());
    }

    #[test]
    fn oversized_is_fatal() {
        let e = ViaError::OversizedPacket { size: 10, limit: 5 };
        assert!(e.is_fatal());
    }

    #[test]
    fn others_are_not_fatal() {
        let non_fatal = [
            ViaError::Malformed {
                context: "x".to_string(),
                bytes_seen: 0,
            },
            ViaError::UnsupportedVersion { protocol: 0 },
            ViaError::UnsupportedPacket {
                state: "login",
                id: 0,
                version: 0,
            },
            ViaError::MissingRegistry {
                key: "k".to_string(),
            },
            ViaError::TranslationOverflow { what: "w" },
        ];
        for e in &non_fatal {
            assert!(!e.is_fatal(), "{} should not be fatal", e);
        }
    }

    #[test]
    fn io_source_is_forwarded() {
        use std::error::Error;
        let e = ViaError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ));
        let src = e.source().expect("Io must have a source");
        assert_eq!(src.to_string(), "broken pipe");
    }

    #[test]
    fn leaf_errors_have_no_source() {
        use std::error::Error;
        let e = ViaError::Malformed {
            context: "c".to_string(),
            bytes_seen: 1,
        };
        assert!(e.source().is_none());
    }
}
