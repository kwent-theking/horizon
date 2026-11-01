//! Packet compression (zlib framing layer) for the Minecraft protocol.
//!
//! Protocol fact: once the server sends the login-state **Set Compression**
//! packet ([`SET_COMPRESSION_ID`]), every following packet in both
//! directions is wrapped as: `VarInt(uncompressed_size) [zlib blob]`, where
//! `uncompressed_size == 0` means "this packet was not compressed" and the
//! bytes directly after the prefix are the normal `VarInt(id) body` frame.
//! When the size is non-zero, the zlib blob inflates to exactly
//! `uncompressed_size` bytes of `VarInt(id) body`.
//!
//! This module keeps the DEFLATE work behind a pluggable
//! [`ZlibBackend`] (the production backend is the flate2-based
//! [`FflateBackend`] registered by [`zlib_impl`]); the pure pass/decode
//! helpers around it are std-only, so the crate itself stays dependency-
//! light apart from `flate2`.
//!
//! RU: обёртка сжатия. После пакета Set Compression (login, 0x03) оба
//! направления оборачивают пакеты как VarInt(несжатый размер) + zlib;
//! размер 0 = пакет не сжат. Сам zlib — за интерфейсом бэкенда
//! (боевой — flate2, см. `zlib_impl`).

use crate::errors::ViaError;
use crate::frame::{read_varint, write_varint, MAX_PACKET_BYTES};
use std::io::Read;

/// Packet id of the login-state **Set Compression** packet.
///
/// Protocol fact: the server sends it once, with a single VarInt threshold
/// payload. From the *next* packet on, both directions compress. A threshold
/// of `-1` disables compression (vanilla servers never send that after
/// enabling it, but the field is a signed VarInt).
///
/// RU: id пакета Set Compression (0x03, login); после него оба направления
/// сжимают пакеты.
pub const SET_COMPRESSION_ID: u32 = 0x03;

/// A zlib (RFC 1950/1951) backend: the one seam where a real DEFLATE
/// implementation plugs in.
///
/// The production instance is the flate2-based [`FflateBackend`] returned
/// by [`zlib_impl`]; the crate's own code never calls a compressor
/// directly, which keeps the compression layer swappable (a fake backend
/// still drives the tests).
///
/// RU: интерфейс zlib-бэкенда; боевой — flate2, тесты — фейковый.
pub trait ZlibBackend {
    /// Compresses `data` into a zlib stream (with header/checksum).
    /// Must be deterministic enough that `decompress` inverts it.
    /// RU: сжать данные в zlib-поток.
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, ViaError>;

    /// Inflates a zlib stream. `expected_max` is a hard output cap: if the
    /// decompressed size would exceed it, the backend must fail instead of
    /// materializing the output (zip-bomb defense).
    /// RU: разжать с жёстким лимитом выхода против zip-бомб.
    fn decompress(&self, data: &[u8], expected_max: usize) -> Result<Vec<u8>, ViaError>;
}

/// Returns the registered zlib backend.
///
/// Declared in the task contract as `pub fn zlib_impl() -> Option<&'static
/// dyn ZlibBackend>`; wiring day has arrived: the backend is the
/// [`FflateBackend`] below, built on the crate's `flate2` dependency, so
/// every compressed path is live. The `Option` shape is kept so the
/// contract (and the seam it documents) stays stable.
///
/// RU: зарегистрированный бэкенд. flate2-бэкенд подключён — сжатие работает.
pub fn zlib_impl() -> Option<&'static dyn ZlibBackend> {
    Some(&FflateBackend)
}

/// The production zlib backend: flate2's pure-Rust `rust_backend` build
/// (see Cargo.toml — no C dependency), streamed through
/// `flate2::read::ZlibEncoder`/`ZlibDecoder` with hard output caps.
// Private on purpose: users reach it through `zlib_impl()`.
// RU: боевой бэкенд на flate2; доступ только через zlib_impl().
struct FflateBackend;

impl ZlibBackend for FflateBackend {
    /// EN: collects the encoder's whole output in bounded chunks.
    /// RU: собирает выход энкодера ограниченными чанками.
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, ViaError> {
        let mut encoder = flate2::read::ZlibEncoder::new(data, flate2::Compression::default());
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = encoder.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            // A compression bomb guard on our own output: zlib never
            // expands, but a broken build (level 0 / stored blocks) could
            // in theory emit header + stored payload; the margin is huge.
            if out.len() + n > data.len() + 1024 {
                return Err(ViaError::TranslationOverflow {
                    what: "zlib compress output exceeded the input-sized budget",
                });
            }
            out.extend_from_slice(&chunk[..n]);
        }
        Ok(out)
    }

    /// EN: refuses to produce more than `expected_max` bytes, so a
    /// zip-bombing peer is cut off before its output is materialized
    /// (checked after every chunk; `out` never grows past the cap).
    /// RU: разжатие с жёстким лимитом выхода против zip-бомб.
    fn decompress(&self, data: &[u8], expected_max: usize) -> Result<Vec<u8>, ViaError> {
        let mut decoder = flate2::read::ZlibDecoder::new(data);
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            // A corrupt/foreign stream is a malformed packet, not a fatal
            // transport error: map flate2's io::Error to Malformed.
            // RU: битый стрим = битый пакет, не фатальная ошибка транспорта.
            let n = decoder
                .read(&mut chunk)
                .map_err(|e| ViaError::Malformed {
                    context: format!("zlib stream is corrupt: {e}"),
                    bytes_seen: out.len(),
                })?;
            if n == 0 {
                break;
            }
            // Post-check after every chunk: `out` never grows past the cap.
            // RU: проверка после каждого чанка — out не растёт выше лимита.
            if out.len() + n > expected_max {
                return Err(ViaError::Malformed {
                    context: format!(
                        "zlib output exceeds the {} byte limit (zip bomb?)",
                        expected_max
                    ),
                    bytes_seen: out.len(),
                });
            }
            out.extend_from_slice(&chunk[..n]);
        }
        Ok(out)
    }
}

/// Whether the connection uses compression, and with which policy.
///
/// RU: включено ли сжатие и какое.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Compression {
    /// No Set Compression received/sent: packets go out raw.
    /// RU: сжатие выключено.
    Off,
    /// zlib framing per the Set Compression contract.
    /// RU: zlib-обёртка.
    Zlib,
}

/// Per-session compression state: mode plus the negotiated threshold.
///
/// The threshold comes from the Set Compression packet's VarInt: packets
/// whose uncompressed size is **below** `threshold` are sent uncompressed
/// (size prefix `0`); those at or above it are deflated. `threshold < 0`
/// means compression is effectively off.
///
/// RU: состояние сжатия сессии: режим + порог из Set Compression.
#[derive(Copy, Clone, Debug)]
pub struct Compressor {
    /// Negotiated threshold from Set Compression (`-1` = off).
    /// RU: порог сжатия из Set Compression (-1 = выключено).
    pub threshold: i32,
}

impl Compressor {
    /// Creates a compressor with the given threshold.
    /// RU: создаёт компрессор с данным порогом.
    pub fn new(threshold: i32) -> Self {
        Compressor { threshold }
    }

    /// Compresses `data` (a raw `VarInt(id) body` payload) into a zlib
    /// stream, using the registered backend.
    ///
    /// The backend cannot be absent in practice ([`zlib_impl`] returns
    /// one); the `None` arm keeps the original contract shape.
    ///
    /// RU: сжимает полезную нагрузку пакета через бэкенд.
    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>, ViaError> {
        match zlib_impl() {
            Some(backend) => backend.compress(data),
            None => Err(ViaError::Malformed {
                context: "zlib backend not wired yet".to_string(),
                bytes_seen: data.len(),
            }),
        }
    }

    /// Inflates a zlib stream, refusing outputs larger than
    /// `expected_max` (zip-bomb defense).
    ///
    /// The backend cannot be absent in practice ([`zlib_impl`] returns
    /// one); the `None` arm keeps the original contract shape.
    ///
    /// RU: разжимает zlib-поток с лимитом против zip-бомб.
    pub fn decompress(&self, data: &[u8], expected_max: usize) -> Result<Vec<u8>, ViaError> {
        match zlib_impl() {
            Some(backend) => backend.decompress(data, expected_max),
            None => Err(ViaError::Malformed {
                context: "zlib backend not wired yet".to_string(),
                bytes_seen: data.len(),
            }),
        }
    }
}

/// Decodes one *compressed* packet: `VarInt(size) [zlib blob | id+body]`.
///
/// Spec: `size` is the uncompressed length of the `VarInt(id) body` frame
/// that follows. `size == 0` means the packet was sent uncompressed and the
/// remaining bytes *are* the raw frame. A positive `size` means the
/// remaining bytes are one zlib stream that inflates to exactly `size`
/// bytes — inflated through `backend`, capped at `expected_max` (with the
/// declared `size` itself already checked against
/// [`MAX_PACKET_BYTES`], so a lying peer cannot make us buffer megabytes).
///
/// Returns `(id, body)` with the body copied out. When `backend` is `None`
/// and `size > 0`, the packet is undecodable —
/// [`ViaError::Malformed`] `"zlib backend not wired yet"`.
///
/// RU: разбирает сжатый пакет; префикс 0 = пакет несжат.
pub fn decode_compressed_packet(
    buf: &[u8],
    threshold: i32,
    backend: Option<&dyn ZlibBackend>,
) -> Result<(u32, Vec<u8>), ViaError> {
    // `threshold` participates only in the decision to compress on the
    // send side; on the receive side it documents policy. Kept in the
    // signature per contract.
    let _ = threshold;

    let mut pos = 0;
    let size = read_varint(buf, &mut pos)?;
    let rest = &buf[pos..];

    let frame: Vec<u8> = if size == 0 {
        // Uncompressed packet: the rest is the raw id+body frame.
        rest.to_vec()
    } else {
        if size < 0 {
            return Err(ViaError::Malformed {
                context: "negative uncompressed size prefix".to_string(),
                bytes_seen: pos,
            });
        }
        let size = size as usize;
        if size > MAX_PACKET_BYTES {
            return Err(ViaError::OversizedPacket {
                size,
                limit: MAX_PACKET_BYTES,
            });
        }
        let backend = backend.ok_or_else(|| ViaError::Malformed {
            context: "zlib backend not wired yet".to_string(),
            bytes_seen: pos,
        })?;
        let inflated = backend.decompress(rest, size)?;
        if inflated.len() != size {
            return Err(ViaError::Malformed {
                context: format!(
                    "decompressed size {} != declared {}",
                    inflated.len(),
                    size
                ),
                bytes_seen: buf.len(),
            });
        }
        inflated
    };

    // Parse id + body out of the (now) uncompressed frame.
    let mut id_pos = 0;
    let id = read_varint(&frame, &mut id_pos)?;
    if id < 0 {
        return Err(ViaError::Malformed {
            context: "negative packet id".to_string(),
            bytes_seen: id_pos,
        });
    }
    Ok((id as u32, frame[id_pos..].to_vec()))
}

/// Encodes one packet into the *compressed* wire format.
///
/// Given the raw `id` + `body`, produces `VarInt(prefix) data`:
/// - if the uncompressed frame would be **below** `threshold` (or the
///   threshold is negative / no backend is available), the packet is sent
///   raw with a `0` size prefix (still legal to the spec, and the only
///   option without a backend);
/// - otherwise `data` is the zlib stream of the full frame and `prefix`
///   is its uncompressed length.
///
/// Note: with no backend wired this never fails — it always falls back to
/// the uncompressed form, which every peer accepts.
///
/// RU: собирает сжатый пакет; без бэкенда — несжатый вариант (префикс 0).
pub fn encode_compressed_packet(
    id: u32,
    body: &[u8],
    threshold: i32,
    backend: Option<&dyn ZlibBackend>,
    out: &mut Vec<u8>,
) -> Result<(), ViaError> {
    let mut frame = Vec::with_capacity(body.len() + 5);
    write_varint(id as i32, &mut frame);
    frame.extend_from_slice(body);

    if threshold < 0 {
        write_varint(0, out); // 0 = uncompressed
        out.extend_from_slice(&frame);
        return Ok(());
    }

    let uncompressed_len = frame.len();
    let backend = match backend {
        Some(b) if (uncompressed_len as i64) >= threshold as i64 => b,
        // Below threshold, or no backend available: raw form (prefix 0).
        _ => {
            write_varint(0, out);
            out.extend_from_slice(&frame);
            return Ok(());
        }
    };

    let compressed = backend.compress(&frame)?;
    write_varint(uncompressed_len as i32, out);
    out.extend_from_slice(&compressed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{read_varint, write_varint};

    #[test]
    fn zlib_impl_is_now_wired() {
        // Wiring day: the flate2 backend is registered, so the Compressor
        // can actually compress/decompress.
        // RU: бэкенд подключён — сжатие живое.
        assert!(zlib_impl().is_some());
        let c = Compressor::new(256);
        let compressed = c.compress(b"repetitive repetitive repetitive").unwrap();
        let plain = c.decompress(&compressed, 1024).unwrap();
        assert_eq!(plain, b"repetitive repetitive repetitive");
    }

    #[test]
    fn flate2_roundtrip_through_wire_format() {
        // Real zlib: encode -> decode over the full compressed packet shape,
        // threshold 0 (always compress).
        // RU: настоящий zlib: полный цикл через формат провода, порог 0.
        let backend = zlib_impl();
        let mut wire_bytes = Vec::new();
        encode_compressed_packet(0x07, b"compressible payload", 0, backend, &mut wire_bytes)
            .unwrap();
        let (id, body) = decode_compressed_packet(&wire_bytes, 0, backend).unwrap();
        assert_eq!(id, 0x07);
        assert_eq!(body, b"compressible payload");
        // The prefix must be the uncompressed length, not 0.
        let mut pos = 0;
        let prefix = read_varint(&wire_bytes, &mut pos).unwrap();
        assert_eq!(prefix as usize, 1 + b"compressible payload".len());
    }

    #[test]
    fn flate2_incompressible_data_roundtrips_too() {
        // Random-ish bytes: zlib "stored" blocks still round-trip.
        let backend = zlib_impl();
        let data: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        let mut wire_bytes = Vec::new();
        encode_compressed_packet(0x2A, &data, 0, backend, &mut wire_bytes).unwrap();
        let (id, body) = decode_compressed_packet(&wire_bytes, 0, backend).unwrap();
        assert_eq!(id, 0x2A);
        assert_eq!(body, data);
    }

    #[test]
    fn flate2_empty_body_roundtrips() {
        let backend = zlib_impl();
        let mut wire_bytes = Vec::new();
        encode_compressed_packet(0x00, b"", 0, backend, &mut wire_bytes).unwrap();
        let (id, body) = decode_compressed_packet(&wire_bytes, 0, backend).unwrap();
        assert_eq!(id, 0x00);
        assert!(body.is_empty());
    }

    #[test]
    fn flate2_zip_bomb_is_cut_by_expected_max() {
        // 10 KB of zeros compress to a tiny stream; a 16-byte cap must
        // refuse to inflate it.
        // RU: 10 КБ нулей в крошечном стриме; лимит 16 байт — отказ.
        let backend = zlib_impl().expect("flate2 backend is wired");
        let compressed = backend.compress(&[0u8; 10_240]).unwrap();
        let err = backend.decompress(&compressed, 16).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn flate2_garbage_stream_is_malformed() {
        let backend = zlib_impl().expect("flate2 backend is wired");
        let err = backend.decompress(&[0xFF, 0xFE, 0x00, 0x11], 1024).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    /// A controllable fake backend for tests: identity or bomb.
    /// RU: фейковый бэкенд для тестов.
    struct FakeBackend {
        /// If Some(n), decompress "inflates" its input to n zero bytes.
        /// RU: если задано, разжимание выдаёт n нулей.
        bomb_size: Option<usize>,
    }

    impl FakeBackend {
        fn honest() -> Self {
            FakeBackend { bomb_size: None }
        }
    }

    impl ZlibBackend for FakeBackend {
        fn compress(&self, data: &[u8]) -> Result<Vec<u8>, ViaError> {
            // Fake "zlib stream": a marker byte + the data reversed.
            let mut out = vec![0xFA];
            out.extend(data.iter().rev());
            Ok(out)
        }

    /// EN: refuses to produce more than `expected_max` bytes, so a
    /// zip-bombing peer is cut off before its output is materialized (the
    /// decoder is dropped mid-stream, not drained).
    /// RU: разжатие с жёстким лимитом выхода против zip-бомб.
    fn decompress(&self, data: &[u8], expected_max: usize) -> Result<Vec<u8>, ViaError> {
            if let Some(bomb) = self.bomb_size {
                if bomb > expected_max {
                    return Err(ViaError::Malformed {
                        context: format!(
                            "zip bomb: {} > {} limit",
                            bomb, expected_max
                        ),
                        bytes_seen: data.len(),
                    });
                }
                return Ok(vec![0u8; bomb]);
            }
            // Honest inverse of compress(): strip marker, reverse.
            if data.first() != Some(&0xFA) {
                return Err(ViaError::Malformed {
                    context: "fake backend: not a compressed stream".to_string(),
                    bytes_seen: data.len(),
                });
            }
            let plain: Vec<u8> = data[1..].iter().rev().copied().collect();
            if plain.len() > expected_max {
                return Err(ViaError::Malformed {
                    context: "output above limit".to_string(),
                    bytes_seen: data.len(),
                });
            }
            Ok(plain)
        }
    }

    /// Builds the wire form of a compressed packet with a *given* prefix,
    /// bypassing encode (for hostile-input tests).
    fn wire(prefix: i32, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(prefix, &mut out);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn set_compression_id_is_0x03() {
        // Protocol fact: Set Compression is 0x03 in the login state.
        assert_eq!(SET_COMPRESSION_ID, 0x03);
    }

    // NOTE: `compressor_requires_backend` from the pre-wiring era was
    // deleted together with the `None` seam: `zlib_impl()` now returns the
    // flate2 backend (see `zlib_impl_is_now_wired`). The no-backend code
    // paths themselves stay covered by the explicit `None` arguments below.
    // RU: тест «бэкенд ещё не подключён» удалён вместе с заглушкой;
    // пути без бэкенда по-прежнему покрыты тестами с аргументом None.

    #[test]
    fn decode_uncompressed_packet_prefix_zero() {
        // Size prefix 0: rest is the raw frame; no backend involved.
        let mut raw = Vec::new();
        write_varint(0x2A, &mut raw);
        raw.extend_from_slice(b"plain");
        let bytes = wire(0, &raw);

        let (id, body) = decode_compressed_packet(&bytes, 256, None).unwrap();
        assert_eq!(id, 0x2A);
        assert_eq!(body, b"plain");
    }

    #[test]
    fn decode_compressed_roundtrip_with_backend() {
        // Honest fake backend: encode -> decode restores (id, body).
        let backend = FakeBackend::honest();
        let mut wire_bytes = Vec::new();
        encode_compressed_packet(0x07, b"compressible payload", 4, Some(&backend), &mut wire_bytes)
            .unwrap();
        let (id, body) = decode_compressed_packet(&wire_bytes, 4, Some(&backend)).unwrap();
        assert_eq!(id, 0x07);
        assert_eq!(body, b"compressible payload");
    }

    #[test]
    fn decode_compressed_with_none_backend_is_error() {
        // Positive prefix but no backend: the agreed Malformed error.
        let backend = FakeBackend::honest();
        let mut raw = Vec::new();
        write_varint(1, &mut raw);
        raw.push(0x61);
        let fake_stream = backend.compress(&raw).unwrap();
        let bytes = wire(raw.len() as i32, &fake_stream);

        let err = decode_compressed_packet(&bytes, 0, None).unwrap_err();
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. }
                if context == "zlib backend not wired yet"),
            "{}",
            err
        );
    }

    #[test]
    fn decode_rejects_size_mismatch() {
        // Declared size 3, backend yields 2 bytes: malformed.
        struct LyingBackend;
        impl ZlibBackend for LyingBackend {
            fn compress(&self, _data: &[u8]) -> Result<Vec<u8>, ViaError> {
                Ok(vec![0xFA])
            }
            fn decompress(&self, _data: &[u8], _m: usize) -> Result<Vec<u8>, ViaError> {
                Ok(vec![0x00, 0x01])
            }
        }
        let bytes = wire(3, &[0xFA]);
        let err = decode_compressed_packet(&bytes, 0, Some(&LyingBackend)).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn decode_rejects_oversized_declared_size() {
        // Declared uncompressed size above MAX_PACKET_BYTES: fatal,
        // rejected *before* any decompression.
        struct PanicBackend;
        impl ZlibBackend for PanicBackend {
            fn compress(&self, _data: &[u8]) -> Result<Vec<u8>, ViaError> {
                panic!("compress must not be called")
            }
            fn decompress(&self, _data: &[u8], _m: usize) -> Result<Vec<u8>, ViaError> {
                panic!("decompress must not be called")
            }
        }
        let bytes = wire(MAX_PACKET_BYTES as i32 + 1, &[0xFA]);
        let err = decode_compressed_packet(&bytes, 0, Some(&PanicBackend)).unwrap_err();
        assert!(matches!(err, ViaError::OversizedPacket { .. }), "{}", err);
        assert!(err.is_fatal());
    }

    #[test]
    fn decode_zip_bomb_is_cut_by_expected_max() {
        // Fake backend that "inflates" to 1 MB regardless of input; the
        // declared size (and thus expected_max) is tiny: must be rejected
        // without materializing the megabyte.
        let bomb = FakeBackend {
            bomb_size: Some(1 << 20),
        };
        let bytes = wire(4, &[0xFA, 0x00]); // claims 4 bytes
        match decode_compressed_packet(&bytes, 0, Some(&bomb)) {
            Err(ViaError::Malformed { context, .. }) => {
                assert!(context.contains("bomb"), "{}", context);
            }
            other => panic!("expected zip-bomb Malformed, got {:?}", other.map(|(i, b)| (i, b.len()))),
        }
    }

    #[test]
    fn decode_negative_prefix_rejected() {
        let err = decode_compressed_packet(&wire(-1, &[0x00]), 0, None).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn encode_below_threshold_sends_uncompressed() {
        // Frame smaller than threshold: prefix 0, raw body, backend unused.
        struct PanicBackend;
        impl ZlibBackend for PanicBackend {
            fn compress(&self, _data: &[u8]) -> Result<Vec<u8>, ViaError> {
                panic!("below threshold must not call compress")
            }
            fn decompress(&self, _data: &[u8], _m: usize) -> Result<Vec<u8>, ViaError> {
                panic!("not used")
            }
        }
        let mut out = Vec::new();
        encode_compressed_packet(0x01, b"tiny", 256, Some(&PanicBackend), &mut out).unwrap();
        // Prefix must be 0 and the payload must decode as a plain frame
        // (uncompressed form decodes fine with no backend).
        let (id, body) = decode_compressed_packet(&out, 256, None).unwrap();
        assert_eq!((id, body.as_slice()), (0x01, b"tiny".as_slice()));
        let mut pos = 0;
        assert_eq!(read_varint(&out, &mut pos).unwrap(), 0, "prefix must be 0");
    }

    #[test]
    fn encode_with_threshold_and_backend_compresses() {
        let backend = FakeBackend::honest();
        let mut out = Vec::new();
        encode_compressed_packet(0x05, b"0123456789ABCDEF", 2, Some(&backend), &mut out).unwrap();
        // Round-trip proves the compressed path was actually taken.
        let (id, body) = decode_compressed_packet(&out, 2, Some(&backend)).unwrap();
        assert_eq!((id, body.as_slice()), (0x05, b"0123456789ABCDEF".as_slice()));
        // And the prefix must be the uncompressed length (not 0).
        let mut pos = 0;
        let prefix = read_varint(&out, &mut pos).unwrap();
        assert_eq!(prefix as usize, 1 + 16, "prefix must be the uncompressed size");
    }

    #[test]
    fn encode_with_no_backend_falls_back_to_uncompressed() {
        // threshold >= 0 but no backend: legal uncompressed form, no error.
        let mut out = Vec::new();
        encode_compressed_packet(0x09, b"fallback", 1, None, &mut out).unwrap();
        // Legal uncompressed form: decodes with no backend needed.
        let (id, body) = decode_compressed_packet(&out, 1, None).unwrap();
        assert_eq!((id, body.as_slice()), (0x09, b"fallback".as_slice()));
    }

    #[test]
    fn encode_negative_threshold_means_off() {
        let mut out = Vec::new();
        encode_compressed_packet(0x02, b"off", -1, None, &mut out).unwrap();
        // Threshold -1: raw form, decodes without any backend.
        let (id, body) = decode_compressed_packet(&out, -1, None).unwrap();
        assert_eq!((id, body.as_slice()), (0x02, b"off".as_slice()));
    }

    #[test]
    fn threshold_at_boundary_compresses() {
        // Spec: packets *at or above* the threshold are compressed;
        // "below" is raw. Frame here is exactly `threshold` bytes.
        let backend = FakeBackend::honest();
        let body = b"1234"; // frame = 1 (id) + 4 = 5 bytes
        let mut out = Vec::new();
        encode_compressed_packet(0x01, body, 5, Some(&backend), &mut out).unwrap();
        let mut pos = 0;
        let prefix = read_varint(&out, &mut pos).unwrap();
        assert_eq!(prefix, 5, "frame size == threshold must compress");
        let (id, got) = decode_compressed_packet(&out, 5, Some(&backend)).unwrap();
        assert_eq!((id, got.as_slice()), (0x01, body.as_slice()));
    }
}
