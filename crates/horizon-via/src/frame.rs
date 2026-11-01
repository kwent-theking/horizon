//! Low-level frame codec for the Minecraft wire protocol.
//!
//! Every packet on the wire is a *frame*: `VarInt(length) VarInt(id) body`,
//! where `length` counts the bytes of `id + body` (not the length varint
//! itself, and not the whole TCP chunk — TCP may split and coalesce frames
//! arbitrarily). [`FrameQueue`] reassembles frames from arbitrary socket
//! chunks; [`decode_packet`]/[`encode_packet`] work on single complete
//! buffers.
//!
//! VarInt encoding (Minecraft flavor): 7 data bits per byte, little-endian
//! order, high bit set = "more bytes follow", at most 5 bytes total. The
//! 5th byte may carry only 4 data bits (bits 28..31); anything beyond that
//! is an overflow. A VarInt holds a full 32-bit pattern — read back as
//! [`i32`] it may be negative (two's complement), which is valid on the
//! wire for some fields but never valid as a packet length or id (rejected
//! here as malformed).
//!
//! RU: низкоуровневый кодек фреймов: VarInt(длина) + VarInt(id) + тело.
//! `FrameQueue` собирает фреймы из произвольных кусков TCP-потока.
//! Длина 0 и длина свыше [`MAX_PACKET_BYTES`] отвергаются как
//! `OversizedPacket` (фатально — защита от враждебного сервера).

use crate::errors::ViaError;

/// Hard cap on one packet's declared length: `2_097_151` (`0x1FFFFF`, the
/// largest value that fits in a 3-byte VarInt). Same order of magnitude as
/// the vanilla client's own limit; a peer announcing more is either broken
/// or hostile, and buffering that much (or more) would let it OOM us.
///
/// RU: жёсткий лимит длины пакета; превышение фатально.
pub const MAX_PACKET_BYTES: usize = 2_097_151;

/// Reads one Minecraft VarInt starting at `*pos`, advancing `*pos` past it.
///
/// Strict parser: at most 5 bytes; a 6th continuation byte or data bits
/// beyond bit 31 is [`ViaError::Malformed`] (overflow); running out of bytes
/// mid-VarInt is [`ViaError::Malformed`] (truncated).
///
/// RU: строгий парсер VarInt: максимум 5 байт, переполнение — ошибка.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Result<i32, ViaError> {
    let mut value: u32 = 0;
    let mut i = 0;
    while i < 5 {
        if *pos >= buf.len() {
            return Err(ViaError::Malformed {
                context: "varint truncated (buffer ended mid-varint)".to_string(),
                bytes_seen: *pos,
            });
        }
        let byte = buf[*pos];
        *pos += 1;
        if i == 4 {
            // Fifth byte: only the low 4 bits are valid data (bits 28..31).
            if byte & 0x80 != 0 {
                return Err(ViaError::Malformed {
                    context: "varint longer than 5 bytes".to_string(),
                    bytes_seen: *pos,
                });
            }
            if byte & 0x70 != 0 {
                return Err(ViaError::Malformed {
                    context: "varint overflows 32 bits".to_string(),
                    bytes_seen: *pos,
                });
            }
            value |= u32::from(byte & 0x0F) << 28;
            return Ok(value as i32);
        }
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(value as i32);
        }
        i += 1;
    }
    // Unreachable: the loop always returns at i == 4 or earlier.
    Err(ViaError::Malformed {
        context: "varint parser unreachable state".to_string(),
        bytes_seen: *pos,
    })
}

/// Appends the VarInt encoding of `value` to `out`.
///
/// Negative values are encoded via their unsigned bit pattern (5 bytes),
/// which is what the Minecraft format prescribes.
///
/// RU: записывает VarInt в буфер; отрицательные — как беззнаковый паттерн.
pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    let mut v = value as u32;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

/// Number of bytes [`write_varint`] will emit for `value` (1..=5).
///
/// RU: длина VarInt-кодировки значения в байтах.
pub fn varint_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut len = 1;
    while v >= 0x80 {
        v >>= 7;
        len += 1;
    }
    len
}

/// A decoded packet: its wire id and a borrowed slice of the body.
///
/// RU: разобранный фрейм: id пакета и тело (заимствованный срез).
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// Wire packet id (never negative on a valid frame).
    /// RU: id пакета.
    pub id: u32,
    /// Packet body: everything after the id VarInt, up to the frame end.
    /// RU: тело пакета после VarInt-а id.
    pub body: &'a [u8],
}

/// Decodes one complete frame from the front of `buf`.
///
/// Layout: `VarInt(length) [VarInt(id) body]` where `length` counts
/// `id + body` bytes only. Returns the frame and the total number of bytes
/// consumed (length varint + `length`), so the caller can advance past it.
///
/// Errors:
/// - length `0` or `> `[`MAX_PACKET_BYTES`] — [`ViaError::OversizedPacket`]
///   (fatal by contract; a zero-length frame carries no id and is treated
///   as a broken/hostile peer, not as a wait-for-more-bytes signal);
/// - a negative length or id, a bad varint, or a truncated body —
///   [`ViaError::Malformed`].
///
/// RU: разбирает один полный фрейм; возвращает фрейм и сколько байт он занял.
pub fn decode_packet(buf: &[u8]) -> Result<(Frame<'_>, usize), ViaError> {
    let mut pos = 0;
    let length = read_varint(buf, &mut pos)?;
    if length < 0 {
        return Err(ViaError::Malformed {
            context: "negative packet length".to_string(),
            bytes_seen: pos,
        });
    }
    let length = length as usize;
    if length == 0 || length > MAX_PACKET_BYTES {
        return Err(ViaError::OversizedPacket {
            size: length,
            limit: MAX_PACKET_BYTES,
        });
    }
    let end = pos + length; // pos <= 5, length <= 2_097_151: no overflow.
    if buf.len() < end {
        return Err(ViaError::Malformed {
            context: "packet body truncated".to_string(),
            bytes_seen: buf.len(),
        });
    }
    let inner = &buf[pos..end];
    let mut id_pos = 0;
    let id = read_varint(inner, &mut id_pos)?;
    if id < 0 {
        return Err(ViaError::Malformed {
            context: "negative packet id".to_string(),
            bytes_seen: pos + id_pos,
        });
    }
    Ok((
        Frame {
            id: id as u32,
            body: &inner[id_pos..],
        },
        end,
    ))
}

/// Appends one complete frame (`VarInt(length) VarInt(id) body`) to `out`.
///
/// `length` is computed automatically. Infallible by signature: the only
/// overflow risk is `id + body` exceeding [`MAX_PACKET_BYTES`], which the
/// *caller* must guard against before calling (we never generate frames
/// that large ourselves).
///
/// RU: собирает полный фрейм в буфер; длину считает сам.
pub fn encode_packet(id: u32, body: &[u8], out: &mut Vec<u8>) {
    let payload_len = varint_len(id as i32) + body.len();
    write_varint(payload_len as i32, out);
    write_varint(id as i32, out);
    out.extend_from_slice(body);
}

/// Outcome of peeking at a leading VarInt without consuming anything.
/// Internal helper for [`FrameQueue::pop`].
/// RU: результат «подглядывания» VarInt без consumption байтов.
enum Peeked {
    /// Not enough bytes yet to decide anything — wait for more.
    Incomplete,
    /// The bytes present can never form a valid VarInt.
    Invalid,
    /// A complete VarInt: its value and its encoded length in bytes.
    Value(i32, usize),
}

/// Reads the VarInt at the front of `buf` without consuming it.
///
/// Differs from [`read_varint`]: a truncated varint is [`Peeked::Incomplete`]
/// (the queue just needs more socket bytes), while an overlong/overflowing
/// one is [`Peeked::Invalid`].
fn peek_varint(buf: &[u8]) -> Peeked {
    let limit = buf.len().min(5);
    let mut value: u32 = 0;
    for i in 0..limit {
        let byte = buf[i];
        if i == 4 {
            if byte & 0x80 != 0 || byte & 0x70 != 0 {
                return Peeked::Invalid;
            }
            value |= u32::from(byte & 0x0F) << 28;
            return Peeked::Value(value as i32, 5);
        }
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Peeked::Value(value as i32, i + 1);
        }
    }
    Peeked::Incomplete
}

/// Reassembles Minecraft frames from arbitrary TCP chunks.
///
/// TCP delivers an undifferentiated byte stream: one `push` may contain
/// half a frame, three frames and a quarter of a fourth. Push whatever
/// arrives; [`pop`](FrameQueue::pop) then yields each complete frame in
/// order, or `None` while waiting for more bytes.
///
/// Error handling: `pop` returns the error of a bad frame **once** and
/// consumes the offending bytes, so the queue keeps making progress —
/// a bad length varint costs only those varint bytes (the frame boundary
/// is unknowable), a bad frame body costs the whole frame. Callers should
/// still tear the session down on fatal errors
/// ([`ViaError::OversizedPacket`], [`ViaError::Io`]).
///
/// RU: сборщик фреймов из кусков TCP-потока; pop выдаёт готовые пакеты
/// по одному или None, пока байтов не хватает.
#[derive(Debug, Default)]
pub struct FrameQueue {
    /// Bytes received but not yet consumed by a popped frame.
    /// RU: ещё не разобранные байты.
    buf: Vec<u8>,
}

impl FrameQueue {
    /// Creates an empty queue.
    /// RU: создаёт пустую очередь.
    pub fn new() -> Self {
        FrameQueue { buf: Vec::new() }
    }

    /// Number of buffered bytes not yet consumed by a popped frame.
    /// Useful for spotting a stuck stream.
    /// RU: сколько байтов ждёт в очереди.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Feeds raw bytes received from the socket into the queue.
    /// RU: кладёт пришедшие с сокета байты в очередь.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pops the next **complete** frame.
    ///
    /// - `None` — no complete frame is buffered yet (need more bytes);
    /// - `Some(Ok((id, body)))` — one full frame, body copied out;
    /// - `Some(Err(e))` — the next frame is broken; its bytes are consumed
    ///   so a subsequent `pop` looks at what follows.
    ///
    /// RU: вытаскивает следующий полный пакет, либо None — ждём байты.
    /// Pop one raw frame's payload *including* its id VarInt: the shape
    /// `decode_compressed_packet` expects when compression is on. Only the
    /// length prefix is consumed. `None` when a full frame has not arrived.
    /// RU: снять сырой кадр с id (без вырезания id): формат для decode_compressed_packet.
    pub fn pop_raw(&mut self) -> Option<Result<Vec<u8>, ViaError>> {
        if self.buf.is_empty() {
            return None;
        }
        let (length, varint_bytes) = match peek_varint(&self.buf) {
            Peeked::Incomplete => return None,
            Peeked::Invalid => {
                let n = self.buf.len().min(5);
                self.buf.drain(..n);
                return Some(Err(ViaError::Malformed {
                    context: "packet length varint invalid".to_string(),
                    bytes_seen: n,
                }));
            }
            Peeked::Value(v, n) => (v, n),
        };
        if length < 0 {
            self.buf.drain(..varint_bytes);
            return Some(Err(ViaError::Malformed {
                context: "negative packet length".to_string(),
                bytes_seen: varint_bytes,
            }));
        }
        let length = length as usize;
        if length == 0 || length > MAX_PACKET_BYTES {
            self.buf.drain(..varint_bytes);
            return Some(Err(ViaError::OversizedPacket {
                size: length,
                limit: MAX_PACKET_BYTES,
            }));
        }
        let total = varint_bytes + length;
        if self.buf.len() < total {
            return None;
        }
        // Drain the whole frame (prefix included), then keep the payload.
        // RU: съесть кадр целиком и вернуть payload без length.
        let frame_bytes: Vec<u8> = self.buf.drain(..total).collect();
        Some(Ok(frame_bytes[varint_bytes..].to_vec()))
    }
    pub fn pop(&mut self) -> Option<Result<(u32, Vec<u8>), ViaError>> {
        if self.buf.is_empty() {
            return None;
        }
        let (length, varint_bytes) = match peek_varint(&self.buf) {
            Peeked::Incomplete => return None,
            Peeked::Invalid => {
                let n = self.buf.len().min(5);
                self.buf.drain(..n);
                return Some(Err(ViaError::Malformed {
                    context: "packet length varint invalid".to_string(),
                    bytes_seen: n,
                }));
            }
            Peeked::Value(v, n) => (v, n),
        };
        if length < 0 {
            self.buf.drain(..varint_bytes);
            return Some(Err(ViaError::Malformed {
                context: "negative packet length".to_string(),
                bytes_seen: varint_bytes,
            }));
        }
        let length = length as usize;
        if length == 0 || length > MAX_PACKET_BYTES {
            self.buf.drain(..varint_bytes);
            return Some(Err(ViaError::OversizedPacket {
                size: length,
                limit: MAX_PACKET_BYTES,
            }));
        }
        let total = varint_bytes + length;
        if self.buf.len() < total {
            // Length is sane, the body has not fully arrived yet.
            return None;
        }
        // Consume the whole frame, then parse id + body out of the copy.
        // The copy still starts with the length VarInt: the id parse must
        // begin at `varint_bytes`, not at 0 (fixed by the orchestrator's
        // review pass — the original parsed the length prefix as the id).
        let frame_bytes: Vec<u8> = self.buf.drain(..total).collect();
        let mut id_pos = 0;
        let id = match read_varint(&frame_bytes[varint_bytes..], &mut id_pos) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        if id < 0 {
            return Some(Err(ViaError::Malformed {
                context: "negative packet id".to_string(),
                bytes_seen: total,
            }));
        }
        let body_start = varint_bytes + id_pos;
        Some(Ok((id as u32, frame_bytes[body_start..].to_vec())))
    }
}


#[test]
fn pop_raw_reads_big_frame() {
    use crate::frame::FrameQueue;
    let mut payload = vec![0x8F, 0x10, 0x78, 0x9C];
    payload.extend(std::iter::repeat(0xAB).take(812));
    let mut q = FrameQueue::new();
    q.push(&[0xB0, 0x06]);
    q.push(&payload);
    match q.pop_raw() {
        Some(Ok(raw)) => assert!(raw.len() == 816, "first got {}", raw.len()),
        other => panic!("unexpected {:?}", other.map(|r| r.map(|v| v.len()))),
    }
    // Followed by a small frame: len 28 -> payload 28.
    q.push(&[0x1C]);
    q.push(&vec![0x00u8; 28]);
    match q.pop_raw() {
        Some(Ok(raw)) => assert!(raw.len() == 28, "second got {}", raw.len()),
        other => panic!("unexpected second {:?}", other.map(|r| r.map(|v| v.len()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden VarInt encodings from the Minecraft protocol wiki.
    /// RU: золотые значения VarInt.
    const GOLDEN: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (2_097_151, &[0xFF, 0xFF, 0x7F]),
        (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
    ];

    #[test]
    fn read_varint_golden() {
        for &(value, bytes) in GOLDEN {
            let mut pos = 0;
            let got = read_varint(bytes, &mut pos).unwrap_or_else(|e| {
                panic!("read_varint({:?}) failed: {}", bytes, e)
            });
            assert_eq!(got, value, "value mismatch for {:?}", bytes);
            assert_eq!(pos, bytes.len(), "pos mismatch for {:?}", bytes);
        }
    }

    #[test]
    fn write_varint_golden() {
        for &(value, bytes) in GOLDEN {
            let mut out = Vec::new();
            write_varint(value, &mut out);
            assert_eq!(out, bytes, "encoding mismatch for {}", value);
        }
    }

    #[test]
    fn varint_len_golden() {
        for &(value, bytes) in GOLDEN {
            assert_eq!(varint_len(value), bytes.len(), "len mismatch for {}", value);
        }
        assert_eq!(varint_len(0), 1);
        // Negative values have their top bits set: always 5 bytes.
        assert_eq!(varint_len(-2_097_152), 5);
        assert_eq!(varint_len(-2_097_151), 5);
        assert_eq!(varint_len(-1), 5);
        assert_eq!(varint_len(2_097_152), 4);
    }

    #[test]
    fn read_varint_truncated() {
        let mut pos = 0;
        let err = read_varint(&[0x80], &mut pos).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
        assert_eq!(pos, 1);
    }

    #[test]
    fn read_varint_too_many_bytes() {
        // Six continuation bytes: a varint may never exceed 5.
        let mut pos = 0;
        let err = read_varint(&[0x80; 6], &mut pos).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn read_varint_bit_overflow() {
        // Fifth byte with data bits above bit 31: overflow.
        let mut pos = 0;
        let err = read_varint(&[0x80, 0x80, 0x80, 0x80, 0x40], &mut pos).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn read_varint_leaves_rest_untouched() {
        // VarInt followed by payload: only the varint's bytes consumed.
        let buf = [0xFF, 0x01, 0xAA, 0xBB];
        let mut pos = 0;
        assert_eq!(read_varint(&buf, &mut pos).unwrap(), 255);
        assert_eq!(pos, 2);
        assert_eq!(&buf[pos..], &[0xAA, 0xBB]);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let cases: &[(u32, &[u8])] = &[
            (0x00, b""),
            (0x01, b"x"),
            (0x2A, b"hello frame codec"),
            (0x7F, &[0u8; 64]),
            (0x1234, &[0xFF; 300]),
        ];
        for &(id, body) in cases {
            let mut wire = Vec::new();
            encode_packet(id, body, &mut wire);
            let (frame, consumed) = decode_packet(&wire).unwrap_or_else(|e| {
                panic!("decode_packet(id {}) failed: {}", id, e)
            });
            assert_eq!(frame.id, id);
            assert_eq!(frame.body, body);
            assert_eq!(consumed, wire.len());
        }
    }

    #[test]
    fn decode_packet_zero_length_rejected() {
        // Length 0: no id, no body — broken/hostile frame.
        let err = decode_packet(&[0x00]).unwrap_err();
        assert!(
            matches!(err, ViaError::OversizedPacket { size: 0, .. }),
            "{}",
            err
        );
    }

    #[test]
    fn decode_packet_oversized_rejected() {
        // Declared length 2_097_152 = MAX + 1: fatal.
        let mut wire = Vec::new();
        write_varint(MAX_PACKET_BYTES as i32 + 1, &mut wire);
        wire.extend_from_slice(&[0x00]);
        let err = decode_packet(&wire).unwrap_err();
        assert!(
            matches!(err, ViaError::OversizedPacket { ref size, .. } if *size == MAX_PACKET_BYTES + 1),
            "{}",
            err
        );
        assert!(err.is_fatal());
    }

    #[test]
    fn decode_packet_negative_length_rejected() {
        // VarInt -1 as a length: malformed, not fatal.
        let err = decode_packet(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
    }

    #[test]
    fn decode_packet_truncated_body_rejected() {
        // Declares 10 bytes of id+body, provides 1.
        let err = decode_packet(&[0x0A, 0x01]).unwrap_err();
        assert!(
            matches!(err, ViaError::Malformed { ref context, .. } if context.contains("truncated")),
            "{}",
            err
        );
    }

    #[test]
    fn frame_queue_partial_packet() {
        let mut wire = Vec::new();
        encode_packet(0x2A, b"split me", &mut wire);
        let (first, second) = wire.split_at(wire.len() / 2);

        let mut queue = FrameQueue::new();
        queue.push(first);
        assert!(queue.pop().is_none(), "half a frame must not pop");

        queue.push(second);
        match queue.pop() {
            Some(Ok((id, body))) => {
                assert_eq!(id, 0x2A);
                assert_eq!(body, b"split me");
            }
            other => panic!("expected the completed frame, got {:?}", other.map(|r| r.map(|(i, b)| (i, b.len())))),
        }
        assert_eq!(queue.pending(), 0);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn frame_queue_multiple_packets_in_one_chunk() {
        let mut wire = Vec::new();
        encode_packet(1, b"first", &mut wire);
        encode_packet(2, b"second", &mut wire);
        encode_packet(0xFF, &[0xAB; 40], &mut wire);

        let mut queue = FrameQueue::new();
        queue.push(&wire);

        let (id1, body1) = queue.pop().unwrap().unwrap();
        assert_eq!((id1, body1.as_slice()), (1, b"first".as_slice()));
        let (id2, body2) = queue.pop().unwrap().unwrap();
        assert_eq!((id2, body2.as_slice()), (2, b"second".as_slice()));
        let (id3, body3) = queue.pop().unwrap().unwrap();
        assert_eq!((id3, body3.as_slice()), (0xFF, [0xAB; 40].as_slice()));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn frame_queue_byte_at_a_time() {
        // Worst-case TCP fragmentation: one byte per push.
        let mut wire = Vec::new();
        encode_packet(0x42, b"drip", &mut wire);

        let mut queue = FrameQueue::new();
        for i in 0..wire.len() {
            assert!(queue.pop().is_none(), "pop must wait until complete ({} bytes in)", i + 1);
            queue.push(&wire[i..i + 1]);
        }
        let (id, body) = queue.pop().unwrap().unwrap();
        assert_eq!((id, body.as_slice()), (0x42, b"drip".as_slice()));
    }

    #[test]
    fn frame_queue_zero_length_is_error_not_wait() {
        // A declared 0-length frame is a broken peer, not a data deficit.
        let mut queue = FrameQueue::new();
        queue.push(&[0x00, 0x00]);
        let err = queue.pop().unwrap().unwrap_err();
        assert!(matches!(err, ViaError::OversizedPacket { size: 0, .. }), "{}", err);
    }

    #[test]
    fn frame_queue_oversized_is_error() {
        let mut wire = Vec::new();
        write_varint(MAX_PACKET_BYTES as i32 + 1, &mut wire);
        wire.extend_from_slice(&[0x00]);

        let mut queue = FrameQueue::new();
        queue.push(&wire);
        let err = queue.pop().unwrap().unwrap_err();
        assert!(matches!(err, ViaError::OversizedPacket { .. }), "{}", err);
        assert!(err.is_fatal());
    }

    #[test]
    fn frame_queue_invalid_length_varint_is_error_then_restarts() {
        // Five bytes with the continuation bit set in the fifth: never a
        // valid varint, must not hang. All five get consumed.
        let mut queue = FrameQueue::new();
        queue.push(&[0x80, 0x80, 0x80, 0x80, 0x80]);
        let err = queue.pop().unwrap().unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
        assert_eq!(queue.pending(), 0);

        // The queue consumed the garbage and can read a fresh frame after it.
        let mut wire = Vec::new();
        encode_packet(7, b"after garbage", &mut wire);
        queue.push(&wire);
        let (id, body) = queue.pop().unwrap().unwrap();
        assert_eq!((id, body.as_slice()), (7, b"after garbage".as_slice()));
    }

    #[test]
    fn frame_queue_bad_frame_consumed_and_next_read() {
        // Frame declares 3 bytes but its id varint is truncated inside
        // (0x80 continuation then frame end): whole bad frame consumed
        // (5 = 1 length + 4... here 1 + 2), the next one still decodes.
        let mut wire = Vec::new();
        let bad: [u8; 3] = [0x02, 0x80, 0x80]; // length 2, id varint dangling
        wire.extend_from_slice(&bad);
        encode_packet(9, b"next", &mut wire);

        let mut queue = FrameQueue::new();
        queue.push(&wire);
        let err = queue.pop().unwrap().unwrap_err();
        assert!(matches!(err, ViaError::Malformed { .. }), "{}", err);
        assert_eq!(queue.pending(), wire.len() - bad.len(), "bad frame fully consumed");
        let (id, body) = queue.pop().unwrap().unwrap();
        assert_eq!((id, body.as_slice()), (9, b"next".as_slice()));
    }

    #[test]
    fn frame_queue_length_ready_body_pending() {
        // Full length varint arrived (length = varint_len(id 3) + 20 = 21,
        // itself 1 byte), body not yet: None, not an error.
        let mut wire = Vec::new();
        encode_packet(3, &[0u8; 20], &mut wire);
        let cut = varint_len(21);
        let mut queue = FrameQueue::new();
        queue.push(&wire[..cut]);
        assert!(queue.pop().is_none());
        queue.push(&wire[cut..]);
        assert!(queue.pop().is_some());
    }
}
