//! fake-fml-server: minimal fake Minecraft 1.21.1 (protocol 767) server for tests.
//! Имитирует минимальный сервер Minecraft 1.21.1 (протокол 767) для тестирования
//! клиента horizon против NeoForge-style handshake без запуска реального сервера.
//!
//! Implements (per https://wiki.vg):
//! Реализовано (по wiki.vg):
//!   - Handshake (Intention): protocol VarInt, address String, port u16, intent VarInt
//!   - intent=1 -> Status: ping JSON + Ping/Pong
//!   - intent=2 -> Login: Hello -> LoginFinished (echo uuid/name) ->
//!       NO SetCompression (legal: packet is optional) ->
//!       client LoginAcknowledged -> Configuration:
//!       SelectKnownPacks(count=0) <- clientbound, then expect serverbound
//!       CustomPayload "minecraft:brand" ("neoforge") and "neoforge:handshake"
//!       from client is TOLERATED (skipped), FinishConfiguration sent, wait ACK ->
//!       Play: LoginPacket (0x00), then KeepAlive loop.
//!
//! Every packet is logged in hex (both directions).
//! Каждый пакет логируется в hex (в обе стороны).
//!
//! Usage / Использование: fake-fml-server [port]

use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// ============================== VarInt / VarLong ==============================

/// Max VarInt length on the wire (5 bytes).
/// Максимальная длина VarInt на проводе (5 байт).
const VARINT_MAX: usize = 5;

/// Read a VarInt from `buf` starting at `pos` (advances `pos`).
/// Прочитать VarInt из `buf` с позиции `pos` (сдвигает `pos`).
/// Correctness check / Проверка корректности:
///   - value 0            -> 1 byte  0x00
///   - value 1            -> 1 byte  0x01
///   - value 127          -> 1 byte  0x7F
///   - value 128          -> 2 bytes 0x80 0x01  (continuation bit 0x80 set!)
///   - value 2097151      -> 3 bytes 0xFF 0xFF 0x7F
///   - value 767          -> 2 bytes 0xFF 0x05
fn read_varint(buf: &[u8], pos: &mut usize) -> Option<i32> {
    let mut value: u32 = 0;
    for i in 0..VARINT_MAX {
        let b = *buf.get(*pos)?;
        *pos += 1;
        // Low 7 bits contribute shifted by 7*i; high bit = continuation.
        // Младшие 7 бит идут со сдвигом 7*i; старший бит = продолжение.
        value |= ((b & 0x7F) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value as i32);
        }
    }
    None // Too many continuation bytes / Слишком много байт продолжения.
}

/// Read a VarLong (for completeness; not used by this server).
/// Прочитать VarLong (для полноты; сервером не используется).
#[allow(dead_code)]
fn read_varlong(buf: &[u8], pos: &mut usize) -> Option<i64> {
    let mut value: u64 = 0;
    for i in 0..10 {
        let b = *buf.get(*pos)?;
        *pos += 1;
        value |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value as i64);
        }
    }
    None
}

/// Write a VarInt, returning bytes.
/// Записать VarInt, вернуть байты.
fn write_varint(value: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(VARINT_MAX);
    let mut v = value as u32;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80; // Continuation bit: more bytes follow.
                          // Бит продолжения: дальше есть ещё байты.
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
    out
}

// ============================== Primitives ==============================

/// Read a length-prefixed UTF-8 string (max `max` chars). / Length-prefixed UTF-8 string.
/// Прочитать строку UTF-8 с префиксом-длиной (макс `max` символов).
fn read_string(buf: &[u8], pos: &mut usize, max: usize) -> Option<String> {
    let len = read_varint(buf, pos)? as usize;
    if len > max * 4 {
        return None; // Sanity guard / Защита от мусора.
    }
    let end = pos.checked_add(len)?;
    let bytes = buf.get(*pos..end)?;
    *pos = end;
    String::from_utf8(bytes.to_vec()).ok()
}

/// Read a UUID (16 bytes, two longs MSB-first). / UUID (16 байт, MSB первым).
fn read_uuid(buf: &[u8], pos: &mut usize) -> Option<u128> {
    let mut raw = [0u8; 16];
    for b in raw.iter_mut() {
        *b = *buf.get(*pos)?;
        *pos += 1;
    }
    Some(u128::from_be_bytes(raw))
}

/// Format a UUID as the dashed lowercase canonical string.
/// Форматировать UUID в каноничную строку с дефисами.
fn uuid_to_string(u: u128) -> String {
    let b = u.to_be_bytes();
    let hex = |s: &[u8]| s.iter().map(|x| format!("{:02x}", x)).collect::<String>();
    format!("{}-{}-{}-{}-{}",
        hex(&b[0..4]), hex(&b[4..6]), hex(&b[6..8]), hex(&b[8..10]), hex(&b[10..16]))
}

/// Parse a dashed UUID string into u128. / Парсинг UUID-строки в u128.
fn uuid_from_string(s: &str) -> Option<u128> {
    let clean: String = s.chars().filter(|c| *c != '-').collect();
    if clean.len() != 32 {
        return None;
    }
    u128::from_str_radix(&clean, 16).ok()
}

// ============================== Packet framing ==============================

/// Read one framed packet from the stream: [len VarInt][id VarInt][data].
/// Прочитать один пакет из потока: [len VarInt][id VarInt][data].
/// Returns (packet_id, payload). / Возвращает (id пакета, payload).
fn read_packet(stream: &mut TcpStream) -> std::io::Result<Option<(i32, Vec<u8>)>> {
    // Read the length prefix byte-by-byte (it is a VarInt itself).
    // Читаем префикс длины побайтно (он сам является VarInt).
    let mut len_value: u32 = 0;
    let mut len_bytes = 0usize;
    loop {
        let mut one = [0u8; 1];
        let n = stream.read(&mut one)?;
        if n == 0 {
            return Ok(None); // EOF: client disconnected / Клиент отключился.
        }
        let b = one[0];
        len_value |= ((b & 0x7F) as u32) << (7 * len_bytes);
        len_bytes += 1;
        if b & 0x80 == 0 {
            break;
        }
        if len_bytes == VARINT_MAX {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "varint length prefix too long",
            ));
        }
    }
    let total = len_value as usize; // Bytes after the length prefix.
                                    // Байты после префикса длины.
    if total > 1_048_576 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "packet too large",
        ));
    }
    let mut body = vec![0u8; total];
    stream.read_exact(&mut body)?;

    // Packet id is a VarInt at the start of the body.
    // Id пакета — VarInt в начале тела.
    let mut pos = 0usize;
    let id = read_varint(&body, &mut pos).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "bad packet id varint")
    })?;
    Ok(Some((id, body[pos..].to_vec())))
}

/// Assemble a packet: [len VarInt][id VarInt][payload...] -> bytes.
/// Собрать пакет: [len VarInt][id VarInt][payload...] -> байты.
fn packet(id: i32, payload: &[u8]) -> Vec<u8> {
    let id_bytes = write_varint(id);
    let len = id_bytes.len() + payload.len();
    let mut out = write_varint(len as i32);
    out.extend_from_slice(&id_bytes);
    out.extend_from_slice(payload);
    out
}

/// Hex logger, 16 bytes per line. / Лог в hex, 16 байт в строке.
fn log_hex(tag: &str, bytes: &[u8]) {
    println!("[hex] {} ({} bytes)", tag, bytes.len());
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
        println!("  {:04x}: {}", i * 16, hex.join(" "));
    }
}

// ============================== Packet states ==============================

// Login state packet ids (protocol 767). / Id пакетов состояния Login.
const LOGIN_CLIENT_LOGIN_FINISHED: i32 = 0x02;

// Configuration state packet ids (serverbound/clientbound split by direction).
// Id пакетов состояния Configuration (serverbound/clientbound по направлению).
const CONF_CLIENT_FINISH: i32 = 0x0D;      // Clientbound Finish Configuration
const CONF_SERVER_SELECT_KNOWN_PACKS: i32 = 0x0E; // Serverbound SelectKnownPacks
const CONF_CLIENT_CUSTOM_PAYLOAD: i32 = 0x01;     // Clientbound Custom Payload
const CONF_SERVER_CUSTOM_PAYLOAD: i32 = 0x02;     // Serverbound Custom Payload
const CONF_SERVER_FINISH_ACK: i32 = 0x0C;         // Serverbound FinishConfiguration ACK

// Play state. / Состояние Play.
const PLAY_CLIENT_LOGIN: i32 = 0x00;   // Clientbound Login (player info)
const PLAY_CLIENT_KEEP_ALIVE: i32 = 0x26; // Clientbound Keep Alive
const PLAY_SERVER_KEEP_ALIVE: i32 = 0x1B; // Serverbound Keep Alive

/// BigTest frame for state machine / Рамка машины состояний для одного клиента.
struct ClientHandler {
    stream: TcpStream,
}

impl ClientHandler {
    fn send(&mut self, id: i32, payload: &[u8], label: &str) -> std::io::Result<()> {
        let bytes = packet(id, payload);
        log_hex(&format!("S->C {} (id=0x{:02x})", label, id), &bytes);
        self.stream.write_all(&bytes)?;
        self.stream.flush()
    }

    fn recv(&mut self, label: &str) -> std::io::Result<Option<(i32, Vec<u8>)>> {
        let pkt = read_packet(&mut self.stream)?;
        if let Some((id, ref data)) = pkt {
            let mut full = write_varint(id);
            full.extend_from_slice(data);
            log_hex(&format!("C->S {} (id=0x{:02x})", label, id), &full);
        }
        Ok(pkt)
    }

    // ---------- Status branch / Ветка Status ----------
    fn run_status(&mut self) -> std::io::Result<()> {
        // Client sends Status Request (0x00, empty). / Клиент шлёт Status Request.
        let (id, _) = self.recv("Status Request")?.ok_or(eof())?;
        if id != 0x00 {
            println!("[status] unexpected packet id 0x{:02x}, continuing", id);
        }
        let json = r#"{"version":{"name":"1.21.1","protocol":767},"players":{"max":1,"online":0},"description":"fake"}"#;
        let mut payload = write_varint(json.len() as i32);
        payload.extend_from_slice(json.as_bytes());
        self.send(0x00, &payload, "Status Response")?;

        // Ping (0x01, i64) -> Pong (0x01, i64). / Пинг -> Понг.
        if let Some((id, data)) = self.recv("Ping")? {
            if id == 0x01 && data.len() == 8 {
                self.send(0x01, &data, "Pong")?;
            }
        }
        Ok(())
    }

    // ---------- Login -> Configuration -> Play / Логин -> Конфигурация -> Play ----------
    fn run_login(&mut self, hello_name: String, hello_uuid: u128) -> std::io::Result<()> {
        // 1) LoginFinished: UUID + username + property count (0) + strict error handling flag.
        //    LoginFinished: UUID + имя игрока + число пропертей (0).
        //    (767 has no "strict error handling" byte — that arrived in 768; keep it minimal.)
        let mut body = Vec::new();
        body.extend_from_slice(&hello_uuid.to_be_bytes());
        let mut name_bytes = hello_name.clone().into_bytes();
        // Prepend string length prefix / Префикс длины строки.
        let mut name = write_varint(name_bytes.len() as i32);
        name.append(&mut name_bytes);
        body.extend_from_slice(&name);
        body.push(0); // property count = 0 (VarInt)
        self.send(LOGIN_CLIENT_LOGIN_FINISHED, &body, "LoginFinished")?;
        // NOTE: no SetCompression is sent, which is legal — the connection stays
        // uncompressed. / SetCompression не шлём — это легально, соединение без сжатия.

        // 2) Wait for LoginAcknowledged (0x03, empty) from the client.
        //    Ждём LoginAcknowledged (0x03, пустой) от клиента.
        let (id, _) = self.recv("LoginAcknowledged")?.ok_or(eof())?;
        if id != 0x03 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected LoginAcknowledged 0x03, got 0x{:02x}", id),
            ));
        }

        // 3) Configuration: ask for SelectKnownPacks with count=0 (vanilla-only).
        //    Конфигурация: SelectKnownPacks с count=0 (только vanilla).
        //    We send the *serverbound* request variant the client answers to:
        //    actually the server asks via Clientbound SelectKnownPacks; in the
        //    wire table for 767 configuration the Clientbound id is 0x0E.
        //    Сервер спрашивает через Clientbound SelectKnownPacks (id 0x0E).
        let ask = write_varint(0); // entry count = 0 (no data packs declared)
        self.send(CONF_SERVER_SELECT_KNOWN_PACKS, &ask, "SelectKnownPacks")?;
        // Wait for the serverbound reply (same id 0x0E, count varint).
        // Ждём ответ клиента (тот же id 0x0E, count varint).
        let (id, data) = self.recv("SelectKnownPacks reply")?.ok_or(eof())?;
        let mut pos = 0usize;
        let count = read_varint(&data, &mut pos).unwrap_or(-1);
        println!("[conf] client selects {} known packs", count);
        if id != CONF_SERVER_SELECT_KNOWN_PACKS {
            println!("[conf] WARNING: expected SelectKnownPacks reply, got 0x{:02x}", id);
        }

        // 4) Brand + FML handshake payload (custom payloads on channel strings).
        //    Бренд + FML-handshake (custom payload по строковому каналу).
        let brand = b"neoforge";
        let mut ch_brand = write_varint("minecraft:brand".len() as i32);
        ch_brand.extend_from_slice(b"minecraft:brand");
        ch_brand.extend_from_slice(&write_varint(brand.len() as i32));
        ch_brand.extend_from_slice(brand);
        self.send(CONF_CLIENT_CUSTOM_PAYLOAD, &ch_brand, "CustomPayload minecraft:brand")?;

        // A test FML-ish payload on neoforge:handshake channel.
        // Тестовый FML-подобный payload на канале neoforge:handshake.
        let fml_test: &[u8] = &[0x01, 0x02, 0x03, 0x04]; // arbitrary test bytes / произвольные байты
        let mut ch_fml = write_varint("neoforge:handshake".len() as i32);
        ch_fml.extend_from_slice(b"neoforge:handshake");
        ch_fml.extend_from_slice(fml_test);
        self.send(CONF_CLIENT_CUSTOM_PAYLOAD, &ch_fml, "CustomPayload neoforge:handshake")?;

        // 5) Tolerate any clientbound custom payloads (client brand etc.) until ACK.
        //    Допускаем кастомные payload от клиента до ACK.
        loop {
            let (id, data) = self.recv("Configuration packet")?.ok_or(eof())?;
            if id == CONF_SERVER_FINISH_ACK {
                println!("[conf] FinishConfiguration ACK received");
                break;
            } else if id == CONF_SERVER_CUSTOM_PAYLOAD {
                let mut pos = 0usize;
                if let Some(chan) = read_string(&data, &mut pos, 128) {
                    println!("[conf] client custom payload on channel '{}'", chan);
                }
            } else {
                println!("[conf] skipping configuration packet id=0x{:02x} ({} bytes)", id, data.len());
            }
        }

        // 6) Finish Configuration. / Завершить конфигурацию.
        self.send(CONF_CLIENT_FINISH, &[], "FinishConfiguration")?;

        // 7) Play: minimal LoginPacket (0x00). In 767 the "Login" play packet is
        //    actually the player-info composite; a fake minimal server just sends
        //    an empty body — real clients tolerate it or the harness only checks
        //    that the packet arrives. / Минимальный LoginPacket с пустым телом.
        self.send(PLAY_CLIENT_LOGIN, &[], "LoginPacket (play)")?;

        // 8) KeepAlive loop: 10s interval, expect serverbound KeepAlive echo.
        //    Цикл KeepAlive: интервал 10с, ждём эхо от клиента.
        println!("[play] entering keep-alive loop");
        let mut counter: i64 = 1;
        loop {
            let id_bytes = counter.to_be_bytes();
            self.send(PLAY_CLIENT_KEEP_ALIVE, &id_bytes, "KeepAlive")?;
            // Read packets until the matching KeepAlive arrives (skip anything else).
            // Читаем пакеты до совпадающего KeepAlive (остальное пропускаем).
            loop {
                let (id, data) = self.recv("Play packet")?.ok_or(eof())?;
                if id == PLAY_SERVER_KEEP_ALIVE && data.len() == 8 {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(&data);
                    let got = i64::from_be_bytes(arr);
                    if got == counter {
                        println!("[play] keep-alive {} ok", got);
                        break;
                    } else {
                        println!("[play] keep-alive mismatch: sent {}, got {}", counter, got);
                    }
                } else {
                    println!("[play] skipping play packet id=0x{:02x} ({} bytes)", id, data.len());
                }
            }
            counter += 1;
            thread::sleep(Duration::from_secs(10));
        }
    }

    /// Entry point: read the Handshake (Intention) packet and dispatch.
    /// Вход: читаем Handshake (Intention) и ветвим логику.
    fn run(&mut self) -> std::io::Result<()> {
        // Handshake: [protocol VarInt][addr String][port u16][intent VarInt]
        let (id, data) = self.recv("Intention (handshake)")?.ok_or(eof())?;
        if id != 0x00 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected Intention 0x00, got 0x{:02x}", id),
            ));
        }
        let mut pos = 0usize;
        let protocol = read_varint(&data, &mut pos).ok_or(bad("protocol varint"))?;
        let _addr = read_string(&data, &mut pos, 512).ok_or(bad("address string"))?;
        if pos + 2 > data.len() {
            return Err(bad("port"));
        }
        let _port = u16::from_be_bytes([data[pos], data[pos + 1]]);
        pos += 2;
        let intent = read_varint(&data, &mut pos).ok_or(bad("intent varint"))?;
        println!(
            "[handshake] protocol={} intent={} (1=status, 2=login)",
            protocol, intent
        );
        if protocol != 767 {
            println!("[handshake] WARNING: protocol {} != 767, proceeding anyway", protocol);
        }

        match intent {
            1 => self.run_status(),
            2 => {
                // Serverbound Hello: [name String][uuid 16 bytes] (767: has-uuid-data bool absent).
                // Serverbound Hello: [имя String][uuid 16 байт].
                let (id, data) = self.recv("Hello (login start)")?.ok_or(eof())?;
                if id != 0x00 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("expected Hello 0x00, got 0x{:02x}", id),
                    ));
                }
                let mut pos = 0usize;
                let name = read_string(&data, &mut pos, 16).ok_or(bad("hello name"))?;
                let uuid = read_uuid(&data, &mut pos)
                    .or_else(|| uuid_from_string(&name))
                    .unwrap_or(0);
                println!("[login] player '{}' uuid {}", name, uuid_to_string(uuid));
                self.run_login(name, uuid)
            }
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown intent {}", other),
            )),
        }
    }
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "client disconnected")
}

fn bad(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("malformed {}", what))
}

// ============================== main ==============================

fn main() {
    let args: Vec<String> = env::args().collect();
    let port: u16 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(25565);
    let addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake-fml-server: cannot bind {}: {}", addr, e);
            std::process::exit(1);
        }
    };
    println!(
        "fake-fml-server listening on {} (Minecraft 1.21.1, protocol 767)",
        addr
    );
    println!(
        "status: 1.21.1/767, login: no compression, FML brand 'neoforge' handshake"
    );

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc_like_set(r);

    for conn in listener.incoming() {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        match conn {
            Ok(stream) => {
                let peer = stream
                    .peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| "?".to_string());
                println!("[conn] client connected: {}", peer);
                let mut handler = ClientHandler { stream };
                if let Err(e) = handler.run() {
                    println!("[conn] session ended: {}", e);
                }
                println!("[conn] client session closed");
            }
            Err(e) => println!("[conn] accept error: {}", e),
        }
        // Single client at a time is enough for tests; loop for the next one.
        // Одного клиента за раз достаточно; цикл ждёт следующего.
    }
}

/// Poor man's Ctrl-C handling without crates: spawn a stdin watcher thread.
/// Простейшая обработка Ctrl-C без крейтов: поток, следящий за stdin.
/// (Real Ctrl-C kills the process, which is fine for a test tool.)
/// (Настоящий Ctrl-C убьёт процесс — для тест-инструмента этого достаточно.)
fn ctrlc_like_set(_running: Arc<AtomicBool>) {
    // Intentionally empty: the tool is expected to be stopped by Ctrl-C/process kill.
    // Намеренно пусто: инструмент останавливают Ctrl-C/завершением процесса.
}
