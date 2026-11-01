//! Server version probe (ViaHorizonPlus step 1): status-ping handshake.
//! RU: проба версии сервера перед входом.
//!
//! EN: before the Horizon client logs into a server, this module asks the
//! server for its status over a short-lived TCP connection: a handshake with
//! intent "status", one empty status request, the JSON response, then an
//! immediate disconnect. The probe never touches game state, is bounded by a
//! caller-supplied timeout, and its failure is non-fatal by design — an
//! unreachable or legacy server just means "no probe answer", not "cannot
//! join". The answer feeds the client's "server runs <name> (protocol <n>)"
//! message shown when the server does not speak 26.3.
//!
//! The JSON response is parsed **by hand** (bounded substring scanning), on
//! purpose: `horizon-via` keeps serde out, so the scan here is dependency-
//! free. The scanner handles nested objects/arrays, quoted `"{"` inside
//! strings, and the common JSON escapes, and is unit-tested against golden
//! status responses. No socket is ever opened by a test.
//!
//! RU: probe перед входом: handshake с intent=status, пустой status request,
//! чтение JSON, разрыв. Игровое состояние не затрагивается, всё под
//! таймаутом, ошибка пробы не мешает входу. JSON разбирается вручную, без
//! serde; сканер понимает вложенность, скобки в строках и escape-последовательности.

use crate::errors::ViaError;
use crate::frame::{FrameQueue, MAX_PACKET_BYTES, encode_packet, read_varint, write_varint};
use crate::protocol_const::{HANDSHAKE_TYPE, STATE_STATUS};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Default Minecraft server port, used when the address has none.
/// RU: порт сервера по умолчанию.
const DEFAULT_PORT: u16 = 25565;

/// Hard cap on bytes read while waiting for the status response: one
/// maximum-size frame plus its length prefix. A server that streams more
/// than that without completing a frame is broken or hostile.
/// RU: лимит чтения статуса: один максимальный фрейм + префикс.
const STATUS_READ_LIMIT: usize = MAX_PACKET_BYTES + 16;

/// Size of the read chunks while draining the probe socket.
/// RU: размер чанка чтения.
const READ_CHUNK: usize = 4096;

/// The server's answer to a status ping, as far as the probe cares.
///
/// EN: `protocol` is the number the server reported in its status JSON
/// (what a handshake must echo back), `version_name` the human-readable
/// name of that protocol, `motd` the description text (possibly empty —
/// the field is optional in the status response).
///
/// RU: ответ сервера на status-ping: номер протокола, имя версии и MOTD
/// (может быть пустым — поле опционально).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerVersion {
    /// Protocol number the server reported (e.g. 763 for 1.20.1).
    /// RU: номер протокола из ответа сервера.
    pub protocol: i32,
    /// Display name of the server's version (e.g. "1.20.1").
    /// RU: отображаемое имя версии сервера.
    pub version_name: String,
    /// The MOTD (description text), empty when the server sent none.
    /// RU: MOTD сервера, пустая строка, если его нет.
    pub motd: String,
}

/// Connect, send a Status handshake, read the JSON response, disconnect.
/// Timeout-limited, no game state touched.
///
/// EN: `address` is `"host"` or `"host:port"` (default port 25565; a
/// bracketed IPv6 form `"[::1]:port"` is accepted too). `timeout_secs`
/// bounds the connect, the write and the read; `0` is clamped to 1. The
/// handshake echoes the client's own protocol (26.3 / 777), so a same-
/// version server answers normally and an older server still answers with
/// *its* real protocol number in the JSON — servers do not disconnect on a
/// protocol mismatch during status. Failures (DNS, connect, timeout, a
/// malformed or non-JSON answer) come back as `Err(ViaError)`; the caller
/// decides whether that blocks anything (normally it does not).
///
/// RU: подключиться, послать status-handshake, прочитать JSON-ответ,
/// разорвать соединение. Адрес — «host» или «host:port», порт по умолчанию
/// 25565; таймаут ограничивает и connect, и обмен. Ошибки — через
/// `Err(ViaError)`, решение об их последствиях за вызывающим.
pub fn probe_server(address: &str, timeout_secs: u64) -> Result<ServerVersion, ViaError> {
    let (host, port) = split_address(address)?;
    // A zero timeout would abort instantly; clamp to one second.
    // RU: нулевой таймаут мгновенно бы всё убил — поднимаем до секунды.
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let addrs: Vec<std::net::SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(ViaError::Io)?
        .collect();
    if addrs.is_empty() {
        return Err(ViaError::Io(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            format!("no address resolved for {host}:{port}"),
        )));
    }
    let mut stream = None;
    let mut last_err: Option<std::io::Error> = None;
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let mut stream = match stream {
        Some(s) => s,
        None => {
            let e = last_err.unwrap_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!("no address resolved for {host}:{port}"),
                )
            });
            return Err(ViaError::Io(e));
        }
    };
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    // The handshake echoes the client's own protocol (26.3 = 777): servers
    // answer a status ping regardless of the number, and their JSON carries
    // their real protocol.
    // RU: handshake эхом отдаёт протокол клиента (26.3 = 777); сервер всё
    // равно ответит, а в JSON будет его настоящий протокол.
    const HORIZON_PROTOCOL: i32 = 777;
    let request = build_probe_bytes(&host, port, HORIZON_PROTOCOL)?;
    stream.write_all(&request)?;
    let json = read_status_response(&mut stream)?;
    parse_status_json(&json)
}

/// Splits `"host"` / `"host:port"` / `"[v6]:port"` into host and port.
///
/// EN: the port defaults to 25565 when absent. An empty host, a non-numeric
/// or out-of-range port, or a malformed bracket form is `Malformed` —
/// never a panic. Unicode hostnames are out of scope (documented contract).
///
/// RU: разбор адреса «host[:port]»; порт по умолчанию 25565; битый адрес —
/// `Malformed`, не паника.
fn split_address(address: &str) -> Result<(String, u16), ViaError> {
    let bad = |what: &str| ViaError::Malformed {
        context: format!("invalid probe address {address:?}: {what}"),
        bytes_seen: 0,
    };
    if let Some(rest) = address.strip_prefix('[') {
        // Bracketed IPv6: "[::1]" or "[::1]:25565".
        let Some((host, tail)) = rest.split_once(']') else {
            return Err(bad("unterminated [ipv6] bracket"));
        };
        if host.is_empty() {
            return Err(bad("empty [ipv6] host"));
        }
        let port = match tail.strip_prefix(':') {
            Some(p) if !p.is_empty() => p
                .parse::<u16>()
                .map_err(|_| bad("port after [ipv6] is not 0..=65535"))?,
            None if tail.is_empty() => DEFAULT_PORT,
            _ => return Err(bad("garbage after [ipv6] bracket")),
        };
        return Ok((host.to_owned(), port));
    }
    match address.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() {
                return Err(bad("empty host"));
            }
            let port = port
                .parse::<u16>()
                .map_err(|_| bad("port is not 0..=65535"))?;
            Ok((host.to_owned(), port))
        }
        None => {
            if address.is_empty() {
                return Err(bad("empty address"));
            }
            Ok((address.to_owned(), DEFAULT_PORT))
        }
    }
}

/// Builds the two probe frames: the Handshake (id 0x00: VarInt(protocol),
/// string host, unsigned short port big-endian, VarInt(next state = status))
/// followed by the empty Status Request (id 0x00).
///
/// EN: host must be non-empty ASCII of at most 255 bytes (the hostname
/// limit); anything else is `Malformed` before a socket is opened.
///
/// RU: байты пробы: handshake + пустой status request; хост — ASCII
/// не длиннее 255 байт, иначе `Malformed`.
fn build_probe_bytes(host: &str, port: u16, protocol: i32) -> Result<Vec<u8>, ViaError> {
    if host.is_empty() {
        return Err(ViaError::Malformed {
            context: "probe host is empty".to_string(),
            bytes_seen: 0,
        });
    }
    if !host.is_ascii() {
        return Err(ViaError::Malformed {
            context: "probe host is not ASCII".to_string(),
            bytes_seen: 0,
        });
    }
    if host.len() > 255 {
        return Err(ViaError::Malformed {
            context: format!("probe host is {} bytes, limit is 255", host.len()),
            bytes_seen: host.len(),
        });
    }
    let mut body = Vec::with_capacity(host.len() + 10);
    write_varint(protocol, &mut body);
    // Minecraft string: VarInt(byte length) + UTF-8 bytes (ASCII here).
    // RU: строка протокола: VarInt(длина в байтах) + байты.
    write_varint(host.len() as i32, &mut body);
    body.extend_from_slice(host.as_bytes());
    body.extend_from_slice(&port.to_be_bytes());
    // Next state: 1 = status (same value as our canonical STATE_STATUS).
    // RU: следующее состояние: 1 = status.
    write_varint(STATE_STATUS, &mut body);
    let mut out = Vec::with_capacity(body.len() + 8);
    encode_packet(HANDSHAKE_TYPE as u32, &body, &mut out);
    // Status request: id 0x00, no body.
    // RU: status request: id 0x00 без тела.
    encode_packet(0x00, &[], &mut out);
    Ok(out)
}

/// Reads the status response off the probe socket: frames until the first
/// complete one, which must be the status response (id 0x00) carrying one
/// JSON string.
///
/// RU: читает ответ статуса: первый полный фрейм обязан быть id 0x00
/// с одной JSON-строкой внутри.
fn read_status_response(stream: &mut TcpStream) -> Result<String, ViaError> {
    let mut queue = FrameQueue::new();
    let mut chunk = [0u8; READ_CHUNK];
    let mut total = 0usize;
    loop {
        if let Some(result) = queue.pop() {
            let (id, body) = result?;
            if id != 0x00 {
                return Err(ViaError::Malformed {
                    context: format!("unexpected packet id {id:#x} in the status state"),
                    bytes_seen: total,
                });
            }
            return read_status_string(&body);
        }
        if total >= STATUS_READ_LIMIT {
            return Err(ViaError::Malformed {
                context: format!("status response exceeded the {STATUS_READ_LIMIT} byte read limit"),
                bytes_seen: total,
            });
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(ViaError::Malformed {
                context: "connection closed before a status response arrived".to_string(),
                bytes_seen: total,
            });
        }
        total += n;
        queue.push(&chunk[..n]);
    }
}

/// Extracts the single JSON string that is the status response's body:
/// `VarInt(len) utf8`, with `len` matching the body exactly.
///
/// RU: единственная строка JSON в теле ответа: VarInt(длина) + utf8.
fn read_status_string(body: &[u8]) -> Result<String, ViaError> {
    let mut pos = 0;
    let len = read_varint(body, &mut pos)?;
    if len < 0 {
        return Err(ViaError::Malformed {
            context: format!("negative status string length {len}"),
            bytes_seen: pos,
        });
    }
    let len = len as usize;
    if len > MAX_PACKET_BYTES {
        return Err(ViaError::OversizedPacket {
            size: len,
            limit: MAX_PACKET_BYTES,
        });
    }
    if body.len() - pos != len {
        return Err(ViaError::Malformed {
            context: format!(
                "status string length {} does not match the {} remaining body bytes",
                len,
                body.len() - pos
            ),
            bytes_seen: pos,
        });
    }
    String::from_utf8(body[pos..].to_vec()).map_err(|_| ViaError::Malformed {
        context: "status string is not valid UTF-8".to_string(),
        bytes_seen: pos,
    })
}

/// Parses a status JSON response into a [`ServerVersion`], by hand.
///
/// EN: requires the first `"version"` object to carry a numeric `protocol`
/// and a string `name` (missing or non-numeric → `Malformed`); the
/// `description` text is optional and defaults to an empty string. The
/// scanner respects nesting and quoted braces, so names like
/// `"Weird {Name}"` parse correctly.
///
/// RU: разбор JSON статуса вручную: в объекте `"version"` обязательны
/// числовой `protocol` и строка `name`; текст описания опционален.
pub fn parse_status_json(json: &str) -> Result<ServerVersion, ViaError> {
    let version = value_after_key(json, "version").ok_or_else(|| ViaError::Malformed {
        context: "status response has no \"version\" object".to_string(),
        bytes_seen: 0,
    })?;
    let version_obj = version
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
        .ok_or_else(|| ViaError::Malformed {
            context: "\"version\" is not a JSON object".to_string(),
            bytes_seen: 0,
        })?;
    let protocol = parse_int_field(version_obj, "protocol").ok_or_else(|| ViaError::Malformed {
        context: "\"version\" object has no numeric \"protocol\"".to_string(),
        bytes_seen: 0,
    })?;
    let version_name =
        parse_string_field(version_obj, "name").ok_or_else(|| ViaError::Malformed {
            context: "\"version\" object has no string \"name\"".to_string(),
            bytes_seen: 0,
        })?;
    let motd = parse_motd(json);
    Ok(ServerVersion {
        protocol,
        version_name,
        motd,
    })
}

/// The MOTD: a plain string description, or the first `"text"` field of a
/// description object/component array; empty when absent.
///
/// RU: MOTD: строка, либо первый `"text"` в объекте/массиве; если нет — пусто.
fn parse_motd(json: &str) -> String {
    match value_after_key(json, "description") {
        Some(raw) => {
            if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                return unescape_json_string(inner);
            }
            parse_string_field(raw, "text").unwrap_or_default()
        }
        None => String::new(),
    }
}

/// The raw value slice following the first `"key":` in `json`:
/// a balanced `{...}` / `[...]` block or a quoted string (quotes included),
/// or `None` when the key is absent or its value is neither.
///
/// EN: the scan is quote-aware, so braces inside string values do not end
/// an object early. This is a deliberate minimal parser, not a JSON
/// validator: malformed input degrades to `None`, never a panic.
///
/// RU: сырой срез значения по ключу; сканер учитывает кавычки, скобки в
/// строках не ломают поиск. Это минимальный парсер, не валидатор.
fn value_after_key<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let bytes = json.as_bytes();
    let start = json.find(&pat)?;
    let mut i = skip_ws(bytes, start + pat.len());
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i = skip_ws(bytes, i + 1);
    match bytes.get(i) {
        Some(b'{') | Some(b'[') => {
            let end = matching_delim(json, i)?;
            Some(&json[i..=end])
        }
        Some(b'"') => {
            let end = string_end(json, i)?;
            Some(&json[i..=end])
        }
        _ => None,
    }
}

/// A numeric field of a JSON object slice: `"key": <i32>`.
///
/// RU: числовое поле объекта: `"ключ": <i32>`.
fn parse_int_field(section: &str, key: &str) -> Option<i32> {
    let pat = format!("\"{key}\"");
    let bytes = section.as_bytes();
    let start = section.find(&pat)?;
    let mut i = skip_ws(bytes, start + pat.len());
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i = skip_ws(bytes, i + 1);
    // The number runs to the first non-number byte or to the end of the
    // slice; a trailing garbage byte after a valid prefix still parses.
    // RU: число идёт до первого не-числового байта или конца среза.
    let digits_len = bytes[i..]
        .iter()
        .position(|b| !b.is_ascii_digit() && *b != b'-')
        .unwrap_or(bytes.len() - i);
    section[i..i + digits_len].parse().ok()
}

/// A string field of a JSON object slice: `"key": "value"`, unescaped.
///
/// RU: строковое поле объекта, с раскодированием escape.
fn parse_string_field(section: &str, key: &str) -> Option<String> {
    let raw = value_after_key(section, key)?;
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    Some(unescape_json_string(inner))
}

/// Index of the delimiter closing the one at `open` (object or array),
/// ignoring delimiters inside JSON strings.
///
/// RU: индекс парной скобки для открывающей в `open`, скобки в строках
/// не считаются.
fn matching_delim(json: &str, open: usize) -> Option<usize> {
    let bytes = json.as_bytes();
    let opener = *bytes.get(open)?;
    let closer = match opener {
        b'{' => b'}',
        b'[' => b']',
        _ => return None,
    };
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return if b == closer { Some(i) } else { None };
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Index of the closing quote of the JSON string opening at `open`
/// (which must be `b'"'`), honoring backslash escapes.
///
/// RU: индекс закрывающей кавычки строки с учётом escape.
fn string_end(json: &str, open: usize) -> Option<usize> {
    let bytes = json.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Decodes JSON string escapes (`\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`,
/// `\t`, `\uXXXX`). Unknown or broken escapes degrade to U+FFFD instead
/// of failing: this is display text, not wire-critical data.
///
/// RU: декодирование escape-последовательностей JSON; битый escape
/// превращается в U+FFFD, а не в ошибку.
fn unescape_json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('b') => out.push('\u{0008}'),
            Some('f') => out.push('\u{000C}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(ch) => out.push(ch),
                    None => out.push('\u{FFFD}'),
                }
            }
            _ => out.push('\u{FFFD}'),
        }
    }
    out
}

/// First byte position at or after `i` that is not ASCII whitespace.
/// RU: позиция первого не-пробельного байта начиная с i.
fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic vanilla status answer (favicon truncated).
    const GOLDEN_STATUS: &str = concat!(
        r#"{"version":{"name":"1.20.1","protocol":763},"#,
        r#""players":{"max":100,"online":5,"sample":[{"name":"Notch","id":"0"}]},"#,
        r#""description":{"text":"A Minecraft Server"},"#,
        r#""enforcesSecureChat":true,"favicon":"data:image/png;base64,AAA="}"#
    );

    #[test]
    fn parse_golden_status() {
        let v = parse_status_json(GOLDEN_STATUS).unwrap();
        assert_eq!(v.protocol, 763);
        assert_eq!(v.version_name, "1.20.1");
        assert_eq!(v.motd, "A Minecraft Server");
    }

    #[test]
    fn parse_plain_string_description() {
        let json = r#"{"version":{"name":"1.8.9","protocol":47},"description":"Old but gold"}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.protocol, 47);
        assert_eq!(v.version_name, "1.8.9");
        assert_eq!(v.motd, "Old but gold");
    }

    #[test]
    fn parse_component_array_description() {
        // 26.3-style description: an array of components.
        let json = r#"{"version":{"name":"1.21.1","protocol":767},"description":[{"text":"Welcome "},{"text":"here","color":"red"}]}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.motd, "Welcome ");
    }

    #[test]
    fn parse_missing_description_is_empty_motd() {
        let json = r#"{"version":{"name":"1.12.2","protocol":340}}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.motd, "");
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_the_scanner() {
        let json = r#"{"version":{"name":"Weird {Name} v1.8","protocol":47},"description":{"text":"{motd}"}}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.version_name, "Weird {Name} v1.8");
        assert_eq!(v.motd, "{motd}");
    }

    #[test]
    fn parse_escapes_in_name_and_motd() {
        let json = r#"{"version":{"name":"A \"quoted\" \u00dc name","protocol":754},"description":{"text":"line1\nline2"}}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.version_name, "A \"quoted\" Ü name");
        assert_eq!(v.motd, "line1\nline2");
    }

    #[test]
    fn parse_horizon_native_version() {
        // The client's own version: same protocol number, no note expected.
        let json = r#"{"version":{"name":"26.3","protocol":777},"description":{"text":"same version"}}"#;
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.protocol, 777);
        assert_eq!(v.version_name, "26.3");
        assert_eq!(v.motd, "same version");
    }

    #[test]
    fn parse_missing_version_is_malformed() {
        assert!(parse_status_json("{}").is_err());
        assert!(parse_status_json(r#"{"players":{}}"#).is_err());
        assert!(parse_status_json("").is_err());
    }

    #[test]
    fn parse_version_without_protocol_or_name_is_malformed() {
        assert!(parse_status_json(r#"{"version":{"name":"1.20.1"}}"#).is_err());
        assert!(parse_status_json(r#"{"version":{"protocol":763}}"#).is_err());
        // protocol present but not a number
        assert!(parse_status_json(r#"{"version":{"name":"x","protocol":"763"}}"#).is_err());
    }

    #[test]
    fn parse_non_object_version_is_malformed() {
        assert!(parse_status_json(r#"{"version":"1.20.1"}"#).is_err());
        assert!(parse_status_json(r#"{"version":42}"#).is_err());
    }

    #[test]
    fn whitespace_around_colons_is_tolerated() {
        let json = "{ \"version\" : { \"name\" : \"1.16.5\" , \"protocol\" : 754 } }";
        let v = parse_status_json(json).unwrap();
        assert_eq!(v.protocol, 754);
        assert_eq!(v.version_name, "1.16.5");
    }

    #[test]
    fn probe_wire_bytes_are_golden() {
        // host "example.com", port 25565, protocol 777:
        //   handshake body = VarInt(777) VarInt(11) "example.com" u16be(25565) VarInt(1)
        //   frame          = VarInt(18) VarInt(0) body
        //   status request = VarInt(1) VarInt(0)
        let bytes = build_probe_bytes("example.com", 25565, 777).unwrap();
        let mut expected = vec![0x12, 0x00]; // frame length 18, handshake id
        expected.extend_from_slice(&[0x89, 0x06]); // VarInt 777
        expected.push(0x0B); // host byte length 11
        expected.extend_from_slice(b"example.com");
        expected.extend_from_slice(&25565u16.to_be_bytes()); // 0x63 0xDD
        expected.push(0x01); // next state = status
        expected.extend_from_slice(&[0x01, 0x00]); // status request frame
        assert_eq!(bytes, expected);
    }

    #[test]
    fn probe_wire_bytes_custom_port_and_protocol() {
        let bytes = build_probe_bytes("mc.example.net", 25566, 47).unwrap();
        // "mc.example.net" is 14 bytes: body = 1 (proto) + 1 (host len) + 14 + 2 (port) + 1 (state)
        // = 19, plus the id byte = frame length 20 (the test's original "21"
        // miscounted the host; fixed by the orchestrator).
        assert_eq!(bytes[0], 20);
        // protocol VarInt 47 is a single byte right after the packet id
        assert_eq!(bytes[2], 47);
        // Layout: [len][id][proto][hostlen][15 host bytes][port BE][state][status req]
        // port 25566 = 0x63DE big-endian; state 1 = status; then the request frame.
        let n = bytes.len();
        assert_eq!(&bytes[n - 5..n - 3], &[0x63, 0xDE]); // port
        assert_eq!(bytes[n - 3], 0x01); // next state = status
        assert_eq!(&bytes[n - 2..], &[0x01, 0x00]); // status request
    }

    #[test]
    fn probe_host_validation() {
        assert!(build_probe_bytes("", 25565, 777).is_err());
        assert!(build_probe_bytes("сервер", 25565, 777).is_err()); // non-ASCII
        let long = "a".repeat(256);
        assert!(build_probe_bytes(&long, 25565, 777).is_err());
        let ok = "a".repeat(255);
        assert!(build_probe_bytes(&ok, 25565, 777).is_ok());
    }

    #[test]
    fn status_string_roundtrip_and_trailing_garbage_rejected() {
        let json = r#"{"version":{"name":"1.20.1","protocol":763}}"#;
        let mut body = Vec::new();
        write_varint(json.len() as i32, &mut body);
        body.extend_from_slice(json.as_bytes());
        assert_eq!(read_status_string(&body).unwrap(), json);
        // Extra bytes after the declared string: malformed.
        body.push(0x00);
        assert!(read_status_string(&body).is_err());
        // Truncated string: malformed.
        let mut short = Vec::new();
        write_varint(100, &mut short);
        short.extend_from_slice(b"only a few");
        assert!(read_status_string(&short).is_err());
        // Negative length: malformed.
        assert!(read_status_string(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).is_err());
        // Invalid UTF-8: malformed.
        let mut bad = Vec::new();
        write_varint(2, &mut bad);
        bad.extend_from_slice(&[0xFE, 0xFF]);
        assert!(read_status_string(&bad).is_err());
    }

    #[test]
    fn split_address_forms() {
        assert_eq!(split_address("mc.example.com").unwrap(), ("mc.example.com".to_owned(), 25565));
        assert_eq!(split_address("mc.example.com:25566").unwrap(), ("mc.example.com".to_owned(), 25566));
        assert_eq!(split_address("127.0.0.1").unwrap(), ("127.0.0.1".to_owned(), 25565));
        assert_eq!(split_address("127.0.0.1:1234").unwrap(), ("127.0.0.1".to_owned(), 1234));
        assert_eq!(split_address("[::1]:25566").unwrap(), ("::1".to_owned(), 25566));
        assert_eq!(split_address("[::1]").unwrap(), ("::1".to_owned(), 25565));
    }

    #[test]
    fn split_address_rejects_garbage() {
        assert!(split_address("").is_err());
        assert!(split_address(":1234").is_err());
        assert!(split_address("host:").is_err());
        assert!(split_address("host:abc").is_err());
        assert!(split_address("host:99999").is_err());
        assert!(split_address("[::1").is_err()); // unterminated bracket
        assert!(split_address("[]").is_err()); // empty ipv6 host
        assert!(split_address("[::1]x").is_err()); // garbage after bracket
    }
}
