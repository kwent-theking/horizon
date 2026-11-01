use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use horizon_via::compression::{
    decode_compressed_packet, encode_compressed_packet, zlib_impl, SET_COMPRESSION_ID,
};
use horizon_via::errors::ViaError;
use horizon_via::frame::{encode_packet, read_varint, write_varint, FrameQueue};
use horizon_via::protocol_const::STATE_LOGIN;
use horizon_via::state::ProtocolState;

/// Bot nickname sent in the Login Hello packet.
/// RU: ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âº, ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â²ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â² Login Hello.
const BOT_NAME: &str = "BotTest";

/// Exit when no packet arrives for this long.
/// RU: ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ÃƒÂÃ‚Â½ÃƒÂÃ‚Â´.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Exit after this many packets received (runaway-server guard).
/// RU: ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Â¦ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â².
const MAX_PACKETS: usize = 200;

/// Read chunk size off the socket.
/// RU: Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂºÃƒÂÃ‚Â° Ãƒâ€˜Ã¢â‚¬Â¡Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â· Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°.
const READ_CHUNK: usize = 4096;

// ---------------------------------------------------------------------------
// Login-state packet ids (wire ids are stable 1.7 .. 26.x for login).
// EN: taken from the vanilla protocol; horizon-protocol::ids mirrors them.
// RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚Â±ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ id ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² login-Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â· ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â°.
// ---------------------------------------------------------------------------

/// Login clientbound: Disconnect (reason: chat text).
/// RU: login clientbound: Disconnect.
const CB_LOGIN_DISCONNECT: u32 = 0x00;
/// Login clientbound: Login Finished (success -> configuration/play).
/// RU: login clientbound: Login Finished.
const CB_LOGIN_FINISHED: u32 = 0x02;
/// Login serverbound: Hello (name + optional public key).
/// RU: login serverbound: Hello.
const SB_LOGIN_HELLO: u32 = 0x00;
/// Login serverbound: Login Acknowledged (1.20.2+ switch to configuration).
/// RU: login serverbound: Login Acknowledged.
const SB_LOGIN_ACKNOWLEDGED: u32 = 0x03;

/// The zlib compressor negotiated by Set Compression (threshold updated
/// live; `-1` until the server enables compression).
/// RU: Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¸: ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â· Set Compression, ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Â¦ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ -1.
struct Bot {
    /// Compression state: `threshold < 0` = raw frames on the wire.
    /// RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â³ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â; ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã…â€™Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â½Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸.
    threshold: i32,
}

impl Bot {
    /// Reads one wire frame off the stream, honoring the compression mode.
    ///
    /// EN: while compression is off, this is the plain
    /// `FrameQueue`-reassembled `VarInt(len) VarInt(id) body` frame; once
    /// enabled, the frame body is first unwrapped through
    /// `decode_compressed_packet` (the length prefix on the wire then
    /// counts the *compressed* form ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â handled by reassembling raw and
    /// decoding the raw frame's payload).
    ///
    /// RU: Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ Ãƒâ€˜Ã¢â‚¬Å¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¹ÃƒÂÃ‚Â¼ Ãƒâ€˜Ã‚Â Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â‚¬Â¡Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â: ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã¢â‚¬Å¾Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¹ÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â·
    /// ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸; Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â° Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· decode_compressed_packet.
    fn read_packet(
        &mut self,
        stream: &mut TcpStream,
        queue: &mut FrameQueue,
    ) -> Result<Option<(u32, Vec<u8>)>, ViaError> {
        loop {
            if self.threshold >= 0 {
                // Compressed mode: the whole frame payload (id included) is
                // `VarInt(uncompressed_size) [zlib blob | raw frame]`.
                // RU: Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¶ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼: ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â€šÂ¬ Ãƒâ€˜Ã‚Â id ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂºÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â.
                if let Some(result) = queue.pop_raw() {
                    let raw = result?;
                    let head: String = raw.iter().take(160).map(|b| format!("{b:02X}")).collect();
                    println!("[raw frame] len={} hex={}", raw.len(), head);
                    let backend = zlib_impl();
                    let (inner_id, inner_body) =
                        decode_compressed_packet(&raw, self.threshold, backend)?;
                    return Ok(Some((inner_id, inner_body)));
                }
            } else if let Some(result) = queue.pop() {
                let (id, body) = result?;
                return Ok(Some((id, body)));
            }
            let mut chunk = [0u8; READ_CHUNK];
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                return Err(ViaError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed by server",
                )));
            }
            queue.push(&chunk[..n]);
        }
    }

    /// Sends one packet, wrapping it in the compression format if enabled.
    /// RU: ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â²ÃƒÂÃ‚Â»Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡, ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â, ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾.
    fn send_packet(&mut self, stream: &mut TcpStream, id: u32, body: &[u8]) -> std::io::Result<()> {
        let mut wire = Vec::new();
        if self.threshold >= 0 {
            encode_compressed_packet(
                id,
                body,
                self.threshold,
                zlib_impl(),
                &mut wire,
            )
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("compression failed: {e}"),
                )
            })?;
        } else {
            encode_packet(id, body, &mut wire);
        }
        // Compressed wire still needs the outer packet-length prefix.
        // RU: Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã†â€™ ÃƒÂÃ‚Â½Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â¶ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ ÃƒÂÃ‚Â²ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒâ€˜Ã‹â€ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¹ length-ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¸ÃƒÂÃ‚ÂºÃƒâ€˜Ã‚Â.
        if self.threshold >= 0 {
            let mut framed = Vec::with_capacity(wire.len() + 5);
            write_varint(wire.len() as i32, &mut framed);
            framed.extend_from_slice(&wire);
            return stream.write_all(&framed);
        }
        stream.write_all(&wire)
    }
}

/// Prints one line per incoming packet: state, id, len, first 32 hex bytes.
/// RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â° ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Â¶ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¹ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡: Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°, id, ÃƒÂÃ‚Â´ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°, ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ 32 ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° hex.
fn print_packet(state: ProtocolState, id: u32, body: &[u8]) {
    let preview: Vec<String> = body
        .iter()
        .take(32)
        .map(|b| format!("{b:02X}"))
        .collect();
    let more = if body.len() > 32 { "ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦" } else { "" };
    println!(
        "[{}] id={:#04X} len={} bytes: {}{}",
        state.phase_name(),
        id,
        body.len(),
        preview.join(" "),
        more
    );
}

/// Reads a Minecraft chat component (text JSON) out of a Disconnect body:
/// best effort ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â prints the raw UTF-8 if it is not JSON-shaped.
/// RU: ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚ÂºÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â· Disconnect; ÃƒÂÃ‚Â½ÃƒÂÃ‚Âµ JSON ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Âº ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™.
fn print_disconnect_reason(state: ProtocolState, body: &[u8]) {
    let mut pos = 0;
    // Reason field: String (VarInt len + UTF-8).
    // RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°: VarInt(ÃƒÂÃ‚Â´ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°) + UTF-8.
    if let Ok(len) = read_varint(body, &mut pos) {
        if len >= 0 && pos + len as usize <= body.len() {
            if let Ok(text) = std::str::from_utf8(&body[pos..pos + len as usize]) {
                // The string is a JSON chat component; strip the obvious
                // wrapper instead of a full parse ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â good enough for a bot.
                // RU: ÃƒÂÃ‚Â²ÃƒÂÃ‚Â½Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸ JSON-ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡; Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°.
                let plain = text
                    .trim_start_matches("{\"text\":\"")
                    .trim_end_matches("\"}")
                    .trim_start_matches("{'text':'");
                println!(">> {state} DISCONNECT: {}", plain);
                return;
            }
        }
    }
    println!(">> {state} DISCONNECT: <unparsed {:?}>", body);
}

/// Builds the handshake packet body: protocol, host, port, next state.
/// RU: Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ handshake-ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°: ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â», Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡, ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡, Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âµ.
fn build_handshake(protocol: i32, host: &str, port: u16) -> Result<Vec<u8>, ViaError> {
    if !host.is_ascii() || host.len() > 255 {
        return Err(ViaError::Malformed {
            context: format!("invalid host {host:?} for handshake"),
            bytes_seen: host.len(),
        });
    }
    let mut body = Vec::with_capacity(host.len() + 12);
    write_varint(protocol, &mut body);
    write_varint(host.len() as i32, &mut body);
    body.extend_from_slice(host.as_bytes());
    body.extend_from_slice(&port.to_be_bytes());
    write_varint(STATE_LOGIN, &mut body); // next state = login
    Ok(body)
}

/// Builds the login Hello packet body (no profile key, offline-style).
/// RU: Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ login Hello: ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼Ãƒâ€˜Ã‚Â ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚ÂºÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â° (ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Âº ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¹ÃƒÂÃ‚Â½-ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´).
fn build_login_hello(name: &str) -> Result<Vec<u8>, ViaError> {
    if !name.is_ascii() || name.len() > 16 {
        return Err(ViaError::Malformed {
            context: format!("invalid bot name {name:?}"),
            bytes_seen: name.len(),
        });
    }
    let mut body = Vec::with_capacity(name.len() + 8);
    write_varint(name.len() as i32, &mut body);
    body.extend_from_slice(name.as_bytes());
    // NeoForge 1.21.1 reads name + uuid, no bool: match 26.x wire shape.
    // RU: NeoForge 1.21.1 Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ name + uuid ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂµÃƒÂÃ‚Â· ÃƒÂÃ‚Â±Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â»ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚Â° - ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°ÃƒÂÃ‚Âº 26.x.
    for _ in 0..16 {
        body.push(0x00);
    }
    Ok(body)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(address) = args.next() else {
        eprintln!("usage: connect_bot <host[:port]> [protocol]");
        return;
    };
    let protocol: i32 = args.next().and_then(|p| p.parse().ok()).unwrap_or(767);

    // Split the address (borrowing probe.rs's convention: default port 25565).
    // RU: Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·ÃƒÂÃ‚Â±ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬ ÃƒÂÃ‚Â°ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒÂÃ‚Â°, ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã†â€™ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸Ãƒâ€˜Ã…Â½ 25565.
    let (host, port) = match address.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => {
                eprintln!("ERR: bad port {p:?}");
                return;
            }
        },
        None => (address, 25565),
    };
    let addr = format!("{host}:{port}");

    match run(&addr, &host, port, protocol) {
        Ok(()) => println!("DONE"),
        Err(e) => println!("EXIT: {e}"),
    }
}

/// The whole session: connect, handshake, login, configuration, play.
/// RU: ÃƒÂÃ‚Â²Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã‚Â Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â: connect, handshake, login, configuration, play.
fn run(addr: &str, host: &str, port: u16, protocol: i32) -> Result<(), ViaError> {
    println!("connecting to {addr} with protocol {protocol} as {BOT_NAME:?}");
    let mut stream = TcpStream::connect(addr).map_err(ViaError::Io)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(ViaError::Io)?;
    stream
        .set_write_timeout(Some(IDLE_TIMEOUT))
        .map_err(ViaError::Io)?;

    let mut bot = Bot { threshold: -1 };
    let mut queue = FrameQueue::new();

    // --- Handshake + Login Hello -----------------------------------------
    // RU: handshake + Login Hello.
    let hs = build_handshake(protocol, host, port)?;
    bot.send_packet(&mut stream, 0x00, &hs)
        .map_err(ViaError::Io)?;
    let hello = build_login_hello(BOT_NAME)?;
    bot.send_packet(&mut stream, SB_LOGIN_HELLO, &hello)
        .map_err(ViaError::Io)?;
    println!("sent handshake (protocol {protocol}) and login hello");

    // --- Packet loop -------------------------------------------------------
    // RU: Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚ÂºÃƒÂÃ‚Â» ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬ËœÃƒÂÃ‚Â¼ÃƒÂÃ‚Â° ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â².
    let mut state = ProtocolState::Login;
    let mut packets = 0usize;
    let mut last_packet = Instant::now();
    let mut play_reached = false;

    while packets < MAX_PACKETS {
        if last_packet.elapsed() > IDLE_TIMEOUT {
            println!("EXIT: {IDLE_TIMEOUT:?} without a packet");
            break;
        }
        // Short (1 s) socket reads so the idle check above actually runs
        // between packets; would-block just means "still waiting".
        // RU: Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ÃƒÂÃ‚Â½ÃƒÂÃ‚Â´ÃƒÂÃ‚Âµ, Ãƒâ€˜Ã¢â‚¬Â¡Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂºÃƒÂÃ‚Â° Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹ Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»ÃƒÂÃ‚Â° Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â±ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â²ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã…â€™.
        let (id, body) = match bot.read_packet(&mut stream, &mut queue) {
            Ok(Some(p)) => p,
            Ok(None) => unreachable!("read_packet only returns Some"),
            Err(ViaError::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue; // no data yet within this read window
            }
            Err(e) => {
                if play_reached {
                    // The server may close the socket after we got what we
                    // came for: not an error for this bot.
                    // RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â»ÃƒÂÃ‚Âµ PLAY ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â° ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â½ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â±ÃƒÂÃ‚ÂºÃƒÂÃ‚Â°.
                    println!("connection ended after PLAY");
                    return Ok(());
                }
                return Err(e);
            }
        };
        packets += 1;
        last_packet = Instant::now();
        print_packet(state, id, &body);

        match state {
            ProtocolState::Login => {
                match id {
                    0x01 => {
                        // Login Hello (encryption): dump the should-authenticate byte.
                        // RU: дамп байта аутентификации.
                        if body.len() > 1 {
                            let tail: Vec<u8> = body.iter().rev().take(4).cloned().collect();
                            println!("[login hello] last4 (reversed): {:02X?}", tail);
                        }
                    }
                    CB_LOGIN_DISCONNECT => {
                        print_disconnect_reason(state, &body);
                        return Ok(());
                    }
                    SET_COMPRESSION_ID => {
                        // Set Compression: enable the wrapper for both
                        // directions from the next packet on.
                        // RU: Set Compression ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂºÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â±Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂºÃƒâ€˜Ã†â€™ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¶ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚Â.
                        let mut pos = 0;
                        let threshold = read_varint(&body, &mut pos)?;
                        bot.threshold = threshold;
                        println!("compression enabled (threshold {threshold})");
                    }
                    CB_LOGIN_FINISHED => {
                        if ProtocolState::has_configuration(protocol) {
                            // 1.20.2+: Login Acknowledged switches the
                            // server into the configuration state.
                            // RU: 1.20.2+: Login Acknowledged ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Å¡
                            // Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ ÃƒÂÃ‚Â² Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â½ÃƒÂÃ‚Â¸ÃƒÂÃ‚Âµ configuration.
                            bot.send_packet(&mut stream, SB_LOGIN_ACKNOWLEDGED, &[])
                                .map_err(ViaError::Io)?;
                            state = ProtocolState::Configuration;
                            println!("login finished -> configuration");
                            // Vanilla sends client information (locale,
                            // view distance, chat settings) FIRST in the
                            // configuration state. Without it the server
                            // never finishes the phase.
                            // RU: ÃƒÂÃ‚ÂºÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â²Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¼ Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Â»Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â‚¬Å¡ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸.
                            let mut info = Vec::new();
                            write_varint(5, &mut info); // locale "en_US"
                            info.extend_from_slice(b"en_US");
                            info.push(12); // view distance
                            write_varint(0, &mut info); // chat mode: enabled
                            info.push(1); // chat colors
                            info.push(0x7F); // displayed skin parts
                            write_varint(1, &mut info); // main hand: right
                            info.push(0); // filter text: no
                            info.push(1); // allow server listings
                            // (no particle status field in 767: 1.21.2+ only)
                            bot.send_packet(&mut stream, 0x00, &info)
                                .map_err(ViaError::Io)?;
                            println!("[cfg] client information sent");
                        } else {
                            // Older servers skip configuration entirely.
                            // RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Âµ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â´Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â² PLAY Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·Ãƒâ€˜Ã†â€™.
                            state = ProtocolState::Play;
                            play_reached = true;
                            println!("PLAY REACHED");
                        }
                    }
                    _ => {} // hello / custom query / cookie: nothing to do
                }
            }
            ProtocolState::Configuration => {
                // Configuration clientbound -> serverbound replies.
                // Ids from horizon-protocol::ids::configuration.
                // RU: ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â° ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â³Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â°Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¹ Ãƒâ€˜Ã¢â‚¬Å¾ÃƒÂÃ‚Â°ÃƒÂÃ‚Â·Ãƒâ€˜Ã¢â‚¬Â¹.
                const CB_CONFIG_DISCONNECT: u32 = 0x02;
                const CB_CONFIG_KEEP_ALIVE: u32 = 0x04;
                const CB_CONFIG_CUSTOM_PAYLOAD: u32 = 0x01;
                const CB_CONFIG_FINISH: u32 = 0x03;
                const CB_SELECT_KNOWN_PACKS: u32 = 0x0E;
                const SB_CONFIG_CUSTOM_PAYLOAD: u32 = 0x02;
                const SB_CONFIG_KEEP_ALIVE: u32 = 0x04;
                const SB_CONFIG_FINISH: u32 = 0x03;
                const SB_SELECT_KNOWN_PACKS: u32 = 0x07;
                match id {
                    CB_CONFIG_DISCONNECT => {
                        print_disconnect_reason(state, &body);
                        return Ok(());
                    }
                    0x0D => {
                        // Update Tags received: registry sync is over from
                        // our side - tell NeoForge its task is done.
                        // RU: Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸: Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¸ÃƒÂÃ‚Â½ÃƒÂÃ‚Âº Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚ÂµÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â² ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ - ACK.
                        let mut reply = Vec::new();
                        write_varint("neoforge:frozen_registry_sync_completed".len() as i32, &mut reply);
                        reply.extend_from_slice(b"neoforge:frozen_registry_sync_completed");
                        // ACK disabled: vanilla clients send nothing here.
                        // RU: ACK ÃÂ²Ã‘â€¹ÃÂºÃÂ»Ã‘Å½Ã‘â€¡ÃÂµÃÂ½: ÃÂ²ÃÂ°ÃÂ½ÃÂ¸ÃÂ»ÃÂ»ÃÂ° Ã‘â€šÃ‘Æ’Ã‘â€š ÃÂ½ÃÂ¸Ã‘â€¡ÃÂµÃÂ³ÃÂ¾ ÃÂ½ÃÂµ Ã‘Ë†ÃÂ»Ã‘â€˜Ã‘â€š.
                        println!("[cfg] tags received (no ACK, vanilla-like)");
                    }
                    CB_CONFIG_KEEP_ALIVE => {
                        // Echo the payload verbatim.
                        // RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾ keepalive-ÃƒÂÃ‚Â´ÃƒÂÃ‚Â°ÃƒÂÃ‚Â½ÃƒÂÃ‚Â½Ãƒâ€˜Ã¢â‚¬Â¹Ãƒâ€˜Ã¢â‚¬Â¦.
                        bot.send_packet(&mut stream, SB_CONFIG_KEEP_ALIVE, &body)
                            .map_err(ViaError::Io)?;
                    }
                    CB_CONFIG_CUSTOM_PAYLOAD => {
                        // NeoForge negotiation: answer the register query
                        // with the client's channel set.
                        // RU: ÃÂ½ÃÂµÃÂ³ÃÂ¾Ã‘â€šÃÂ¾ÃÂ²ÃÂ°Ã‘â€ ÃÂ¸Ã‘Â NeoForge: ÃÂ¾Ã‘â€šÃÂ²ÃÂµÃ‘â€š ÃÂ½ÃÂ° ÃÂ·ÃÂ°ÃÂ¿Ã‘â‚¬ÃÂ¾Ã‘Â ÃÂºÃÂ°ÃÂ½ÃÂ°ÃÂ»ÃÂ¾ÃÂ².
                        let mut pos = 0;
                        let _ch_len = read_varint(&body, &mut pos).unwrap_or(0) as usize;
                        let channel = String::from_utf8_lossy(
                            &body[pos..(pos + _ch_len).min(body.len())],
                        ).to_string();
                        pos += _ch_len.min(body.len() - pos);
                        println!("[cfg payload] channel: {channel}");

                        let mut reply = Vec::new();
                        let answered = if channel == "neoforge:register" {
                            // A real NeoForge client answers with the
                            // built-in channel registrations.
                            // RU: ÃÂ½ÃÂ°Ã‘ÂÃ‘â€šÃÂ¾Ã‘ÂÃ‘â€°ÃÂ¸ÃÂ¹ NeoForge-ÃÂºÃÂ»ÃÂ¸ÃÂµÃÂ½Ã‘â€š ÃÂ¾Ã‘â€šÃÂ²ÃÂµÃ‘â€¡ÃÂ°ÃÂµÃ‘â€š
                            // ÃÂ²Ã‘ÂÃ‘â€šÃ‘â‚¬ÃÂ¾ÃÂµÃÂ½ÃÂ½Ã‘â€¹ÃÂ¼ÃÂ¸ ÃÂºÃÂ°ÃÂ½ÃÂ°ÃÂ»ÃÂ°ÃÂ¼ÃÂ¸.
                            write_varint("neoforge:register".len() as i32, &mut reply);
                            reply.extend_from_slice(b"neoforge:register");
                            write_varint(1, &mut reply); // one protocol entry
                            write_varint(2, &mut reply); // CONFIGURATION ordinal
                            let conf: &[(&str, u8)] = &[
                                ("neoforge:config_file", 0),
                                ("neoforge:frozen_registry_sync_start", 0),
                                ("neoforge:frozen_registry", 0),
                                ("neoforge:frozen_registry_sync_completed", 2),
                                ("neoforge:known_registry_data_maps", 0),
                                ("neoforge:extensible_enum_data", 0),
                                ("neoforge:known_registry_data_maps_reply", 1),
                                ("neoforge:extensible_enum_ack", 1),
                            ];
                            write_varint(conf.len() as i32, &mut reply);
                            for (id, flow) in conf {
                                write_varint(id.len() as i32, &mut reply);
                                reply.extend_from_slice(id.as_bytes());
                                write_varint(1, &mut reply);
                                reply.push(b'1');
                                match flow {
                                    2 => reply.push(0x00),
                                    f => {
                                        reply.push(0x01);
                                        write_varint(*f as i32, &mut reply);
                                    }
                                }
                                reply.push(0x01);
                            }
                            true
                        } else if channel == "neoforge:known_registry_data_maps" {
                            write_varint("neoforge:known_registry_data_maps_reply".len() as i32, &mut reply);
                            reply.extend_from_slice(b"neoforge:known_registry_data_maps_reply");
                            write_varint(0, &mut reply);
                            true
                        } else if channel == "neoforge:extensible_enum_data" {
                            write_varint("neoforge:extensible_enum_ack".len() as i32, &mut reply);
                            reply.extend_from_slice(b"neoforge:extensible_enum_ack");
                            true
                        } else if channel == "neoforge:network" {
                            write_varint("minecraft:register".len() as i32, &mut reply);
                            reply.extend_from_slice(b"minecraft:register");
                            reply.push(0x00);
                            true
                        } else {
                            false
                        };
                        if answered {
                            bot.send_packet(&mut stream, SB_CONFIG_CUSTOM_PAYLOAD, &reply)
                                .map_err(ViaError::Io)?;
                            println!("[cfg] answered channel: {channel}");
                        }
                    }
                    0x05 => {
                        // PingRequest: echo the id back as Pong (0x05).
                        // NeoForge blocks configuration until Pong(0) arrives.
                        // RU: Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Â¦ÃƒÂÃ‚Â¾-ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â½ÃƒÂÃ‚Â³; NeoForge ÃƒÂÃ‚Â¶ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â‚¬ËœÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚ÂµÃƒÂÃ‚Â½ÃƒÂÃ‚Â½ÃƒÂÃ‚Â¾ ÃƒÂÃ‚ÂµÃƒÂÃ‚Â³ÃƒÂÃ‚Â¾.
                        let mut pong = Vec::new();
                        pong.extend_from_slice(&body);
                        bot.send_packet(&mut stream, 0x05, &pong)
                            .map_err(ViaError::Io)?;
                        println!("[cfg] pong sent (id {:?})", body.first());
                    }
                    CB_SELECT_KNOWN_PACKS => {
                        // Answer with an empty known-packs list (VarInt 0).
                        // RU: ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â‚¬Å¡ ÃƒÂÃ‚Â¿Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â‚¬Â¹ÃƒÂÃ‚Â¼ Ãƒâ€˜Ã‚ÂÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ known packs.
                        bot.send_packet(
                            &mut stream,
                            SB_SELECT_KNOWN_PACKS,
                            &select_known_packs_body(0, &[]),
                        )
                        .map_err(ViaError::Io)?;
                    }
                    CB_CONFIG_FINISH => {
                        // Acknowledge Finish Configuration -> PLAY.
                        // RU: ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â´Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â²ÃƒÂÃ‚ÂµÃƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¶ÃƒÂÃ‚Â´ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼ Finish Configuration -> PLAY.
                        bot.send_packet(&mut stream, SB_CONFIG_FINISH, &[])
                            .map_err(ViaError::Io)?;
                        state = ProtocolState::Play;
                        play_reached = true;
                        println!("PLAY REACHED");
                    }
                    _ => {} // registries, resource packs: nothing to do
                }
            }
            ProtocolState::Play => {
                // Nothing further to answer: the bot's goal is achieved.
                // RU: Ãƒâ€˜Ã¢â‚¬Â ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ ÃƒÂÃ‚Â´ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â³ÃƒÂÃ‚Â½Ãƒâ€˜Ã†â€™Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â°, ÃƒÂÃ‚Â´ÃƒÂÃ‚Â°ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™Ãƒâ€˜Ã‹â€ ÃƒÂÃ‚Âµ Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»Ãƒâ€˜Ã…â€™ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â°ÃƒÂÃ‚Â±ÃƒÂÃ‚Â»Ãƒâ€˜Ã…Â½ÃƒÂÃ‚Â´ÃƒÂÃ‚Â°ÃƒÂÃ‚ÂµÃƒÂÃ‚Â¼.
            }
            ProtocolState::Handshake | ProtocolState::Status => {
                // Not reachable in this bot's flow.
                // RU: ÃƒÂÃ‚Â² Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¾ÃƒÂÃ‚Â¼ ÃƒÂÃ‚Â±ÃƒÂÃ‚Â¾Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Âµ ÃƒÂÃ‚Â½ÃƒÂÃ‚ÂµÃƒÂÃ‚Â´ÃƒÂÃ‚Â¾Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¶ÃƒÂÃ‚Â¸ÃƒÂÃ‚Â¼ÃƒÂÃ‚Â¾.
            }
        }
    }
    if packets >= MAX_PACKETS {
        println!("EXIT: reached the {MAX_PACKETS} packet limit");
    }
    Ok(())
}

/// Select Known Packs body: VarInt(count) then per entry 3 strings.
/// EN: `count == 0` is the accepted "empty list" answer; extra entries
/// (namespace/name/version) are appended for future use.
/// RU: Ãƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚ÂµÃƒÂÃ‚Â»ÃƒÂÃ‚Â¾ Select Known Packs: VarInt(ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¾ÃƒÂÃ‚Â»ÃƒÂÃ‚Â¸Ãƒâ€˜Ã¢â‚¬Â¡ÃƒÂÃ‚ÂµÃƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡ÃƒÂÃ‚Â²ÃƒÂÃ‚Â¾) + ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¾ 3 Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã¢â‚¬Å¡Ãƒâ€˜Ã¢â€šÂ¬ÃƒÂÃ‚Â¾ÃƒÂÃ‚ÂºÃƒÂÃ‚Â¸ ÃƒÂÃ‚Â½ÃƒÂÃ‚Â° ÃƒÂÃ‚Â·ÃƒÂÃ‚Â°ÃƒÂÃ‚Â¿ÃƒÂÃ‚Â¸Ãƒâ€˜Ã‚ÂÃƒâ€˜Ã…â€™.
fn select_known_packs_body(count: i32, entries: &[[&str; 3]]) -> Vec<u8> {
    let mut body = Vec::new();
    write_varint(count, &mut body);
    for e in entries {
        for s in e {
            write_varint(s.len() as i32, &mut body);
            body.extend_from_slice(s.as_bytes());
        }
    }
    body
}
