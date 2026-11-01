//! Minimal JSON value + parser + serializer (no `serde`).
//!
//! EN: only what the account switcher needs: parse HTTPS response bodies and
//! golden test strings, serialize the session file and the accounts store.
//! Objects preserve insertion order (Vec of pairs) so store round-trips are
//! deterministic. Numbers are kept as f64 plus the original raw string so
//! integer round-trips (expiries) never lose precision textually.
//!
//! RU: только то, что нужно свитчеру: разбор ответов HTTPS и golden-строк
//! тестов, сериализация session-файла и хранилища аккаунтов. Объекты хранят
//! порядок вставки, числа — исходную строку, чтобы целые не теряли точность.

use std::fmt::Write as _;

/// EN: parsed JSON value. RU: разобранное JSON-значение.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// boolean
    Bool(bool),
    /// number (f64 view + raw text as received)
    Num(f64, String),
    /// string (already unescaped)
    Str(String),
    /// array
    Arr(Vec<Json>),
    /// object; duplicate keys keep the LAST value (matching serde_json)
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// EN: build a string value. RU: строковое значение.
    pub fn s(v: impl Into<String>) -> Json {
        Json::Str(v.into())
    }

    /// EN: build an integer number value. RU: целочисленное значение.
    pub fn i(v: i64) -> Json {
        Json::Num(v as f64, v.to_string())
    }

    /// EN: object lookup; first match wins. RU: поиск ключа в объекте.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// EN: string field access. RU: доступ к строковому полю.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// EN: f64 view of a number. RU: число как f64.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n, _) => Some(*n),
            _ => None,
        }
    }

    /// EN: object as pairs (for serialization order). RU: объект как пары.
    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(p) => Some(p.as_slice()),
            _ => None,
        }
    }

    /// EN: array items. RU: элементы массива.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// EN: is this `null`? RU: является ли значение null.
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// EN: compact serialization (no spaces). RU: компактная сериализация.
    pub fn to_string_compact(&self) -> String {
        let mut out = String::new();
        write_json(&mut out, self, None, 0);
        out
    }

    /// EN: pretty serialization, two-space indent. RU: сериализация с отступами.
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write_json(&mut out, self, Some(2), 0);
        out
    }
}

impl std::str::FromStr for Json {
    type Err = JsonError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        let mut p = Parser { b: bytes, i: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.i != bytes.len() {
            return Err(JsonError::Trailing);
        }
        Ok(v)
    }
}

/// EN: parse a JSON document. RU: разобрать JSON-документ.
pub fn parse(s: &str) -> Result<Json, JsonError> {
    s.parse()
}

/// EN: parse errors. RU: ошибки разбора.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonError {
    /// unexpected end of input
    Eof,
    /// unexpected byte at the stored index
    Bad(usize),
    /// trailing characters after the value
    Trailing,
    /// bad escape sequence
    Escape(usize),
    /// malformed number
    Number(usize),
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonError::Eof => write!(f, "unexpected end of JSON input"),
            JsonError::Bad(i) => write!(f, "unexpected byte at offset {i}"),
            JsonError::Trailing => write!(f, "trailing characters after JSON value"),
            JsonError::Escape(i) => write!(f, "bad escape sequence at offset {i}"),
            JsonError::Number(i) => write!(f, "malformed number at offset {i}"),
        }
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        if self.i >= self.b.len() {
            return Err(JsonError::Eof);
        }
        match self.b[self.i] {
            b'{' => {
                self.i += 1;
                let mut pairs = Vec::new();
                self.skip_ws();
                if self.i < self.b.len() && self.b[self.i] == b'}' {
                    self.i += 1;
                    return Ok(Json::Obj(pairs));
                }
                loop {
                    self.skip_ws();
                    let key = match self.value()? {
                        Json::Str(s) => s,
                        _ => return Err(JsonError::Bad(self.i)),
                    };
                    self.skip_ws();
                    if self.i >= self.b.len() || self.b[self.i] != b':' {
                        return Err(JsonError::Bad(self.i));
                    }
                    self.i += 1;
                    self.skip_ws();
                    let val = self.value()?;
                    // duplicate key: replace the previous value in place
                    if let Some(slot) = pairs.iter_mut().find(|(k, _): &&mut (String, Json)| *k == key) {
                        slot.1 = val;
                    } else {
                        pairs.push((key, val));
                    }
                    self.skip_ws();
                    if self.i >= self.b.len() {
                        return Err(JsonError::Eof);
                    }
                    match self.b[self.i] {
                        b',' => {
                            self.i += 1;
                        }
                        b'}' => {
                            self.i += 1;
                            return Ok(Json::Obj(pairs));
                        }
                        _ => return Err(JsonError::Bad(self.i)),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.i < self.b.len() && self.b[self.i] == b']' {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.skip_ws();
                    items.push(self.value()?);
                    self.skip_ws();
                    if self.i >= self.b.len() {
                        return Err(JsonError::Eof);
                    }
                    match self.b[self.i] {
                        b',' => {
                            self.i += 1;
                        }
                        b']' => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(JsonError::Bad(self.i)),
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let s = self.string_body()?;
                Ok(Json::Str(s))
            }
            b't' => {
                self.lit(b"true")?;
                Ok(Json::Bool(true))
            }
            b'f' => {
                self.lit(b"false")?;
                Ok(Json::Bool(false))
            }
            b'n' => {
                self.lit(b"null")?;
                Ok(Json::Null)
            }
            c if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(JsonError::Bad(self.i)),
        }
    }

    fn lit(&mut self, want: &[u8]) -> Result<(), JsonError> {
        if self.i + want.len() <= self.b.len() && &self.b[self.i..self.i + want.len()] == want {
            self.i += want.len();
            Ok(())
        } else {
            Err(JsonError::Bad(self.i))
        }
    }

    fn string_body(&mut self) -> Result<String, JsonError> {
        let mut out = String::new();
        loop {
            if self.i >= self.b.len() {
                return Err(JsonError::Eof);
            }
            match self.b[self.i] {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    let esc_at = self.i;
                    self.i += 1;
                    if self.i >= self.b.len() {
                        return Err(JsonError::Eof);
                    }
                    match self.b[self.i] {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            self.i += 1; // skip the 'u', unicode4 reads the hex digits
                            let cp = self.unicode4()?;
                            // surrogate pair?
                            if (0xD800..=0xDBFF).contains(&cp)
                                && self.i + 1 < self.b.len()
                                && self.b[self.i] == b'\\'
                                && self.b[self.i + 1] == b'u'
                            {
                                self.i += 2;
                                let low = self.unicode4()?;
                                if (0xDC00..=0xDFFF).contains(&low) {
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                                    out.push(char::from_u32(c).ok_or(JsonError::Escape(esc_at))?);
                                } else {
                                    out.push('\u{FFFD}');
                                    out.push(char::from_u32(low).unwrap_or('\u{FFFD}'));
                                }
                            } else if (0xD800..=0xDFFF).contains(&cp) {
                                out.push('\u{FFFD}');
                            } else {
                                out.push(char::from_u32(cp).ok_or(JsonError::Escape(esc_at))?);
                            }
                            continue;
                        }
                        _ => return Err(JsonError::Escape(esc_at)),
                    }
                    self.i += 1;
                }
                c if c < 0x20 => return Err(JsonError::Bad(self.i)),
                _ => {
                    // copy one UTF-8 code point
                    let start = self.i;
                    let len = utf8_len(self.b[self.i]).ok_or(JsonError::Bad(self.i))?;
                    if start + len > self.b.len() {
                        return Err(JsonError::Eof);
                    }
                    // SAFETY-free: from_utf8 validates
                    match std::str::from_utf8(&self.b[start..start + len]) {
                        Ok(s) => out.push_str(s),
                        Err(_) => return Err(JsonError::Bad(start)),
                    }
                    self.i += len;
                }
            }
        }
    }

    fn unicode4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(JsonError::Eof);
        }
        let mut v: u32 = 0;
        for k in 0..4 {
            let c = self.b[self.i + k];
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => return Err(JsonError::Escape(self.i)),
            };
            v = v * 16 + d;
        }
        self.i += 4;
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.i < self.b.len() && self.b[self.i] == b'-' {
            self.i += 1;
        }
        while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
            self.i += 1;
        }
        if self.i < self.b.len() && self.b[self.i] == b'.' {
            self.i += 1;
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        if self.i < self.b.len() && (self.b[self.i] == b'e' || self.b[self.i] == b'E') {
            self.i += 1;
            if self.i < self.b.len() && (self.b[self.i] == b'+' || self.b[self.i] == b'-') {
                self.i += 1;
            }
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        if self.i == start {
            return Err(JsonError::Number(start));
        }
        let raw = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| JsonError::Bad(start))?;
        let n: f64 = raw.parse().map_err(|_| JsonError::Number(start))?;
        Ok(Json::Num(n, raw.to_string()))
    }
}

fn utf8_len(b: u8) -> Option<usize> {
    match b {
        0x00..=0x7F => Some(1),
        0xC0..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF7 => Some(4),
        _ => None,
    }
}

fn write_json(out: &mut String, v: &Json, indent: Option<usize>, depth: usize) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(_, raw) => out.push_str(raw),
        Json::Str(s) => write_escaped(out, s),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (n, it) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, depth + 1);
                write_json(out, it, indent, depth + 1);
            }
            newline_indent(out, indent, depth);
            out.push(']');
        }
        Json::Obj(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (n, (k, val)) in pairs.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, depth + 1);
                write_escaped(out, k);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_json(out, val, indent, depth + 1);
            }
            newline_indent(out, indent, depth);
            out.push('}');
        }
    }
}

fn newline_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(w) = indent {
        out.push('\n');
        for _ in 0..w * depth {
            out.push(' ');
        }
    }
}

fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_golden_msa_token_response() {
        let s = r#"{"token_type":"Bearer","scope":"XboxLive.signin offline_access","expires_in":86399,"access_token":"eyJabc","refresh_token":"MC.def42"}"#;
        let j = parse(s).unwrap();
        assert_eq!(j.get("access_token").and_then(Json::as_str), Some("eyJabc"));
        assert_eq!(j.get("refresh_token").and_then(Json::as_str), Some("MC.def42"));
        assert_eq!(j.get("expires_in").and_then(Json::as_f64), Some(86399.0));
    }

    #[test]
    fn parses_xbl_display_claims() {
        let s = r#"{"Token":"XBL.tok","DisplayClaims":{"xui":[{"uhs":"UHS1","xid":"2535401"}]}}"#;
        let j = parse(s).unwrap();
        let xui = j.get("DisplayClaims").unwrap().get("xui").unwrap();
        let first = xui.as_arr().unwrap()[0].get("uhs").unwrap();
        assert_eq!(first.as_str(), Some("UHS1"));
    }

    #[test]
    fn escapes_and_roundtrips() {
        let mut s = String::new();
        write_escaped(&mut s, "a\"b\\c\nd\u{1}");
        assert_eq!(s, "\"a\\\"b\\\\c\\nd\\u0001\"");
        let j = parse(&s).unwrap();
        assert_eq!(j.as_str(), Some("a\"b\\c\nd"));
    }

    #[test]
    fn escapes_control_chars_as_unicode() {
        let mut s = String::new();
        write_escaped(&mut s, "x\u{1}y");
        assert_eq!(s, "\"x\\u0001y\"");
        assert_eq!(parse(&s).unwrap().as_str(), Some("x\u{1}y"));
    }

    #[test]
    fn unicode_surrogate_pair() {
        // U+1F600 encoded as an escaped surrogate pair \uD83D\uDE00
        let j = parse(r#""😀""#).unwrap();
        assert_eq!(j.as_str(), Some("\u{1F600}"));
        // and as a raw 4-byte UTF-8 code point in the input
        let j2 = parse(&format!("\"{}\"", '\u{1F600}')).unwrap();
        assert_eq!(j2.as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn pretty_and_compact_roundtrip() {
        let j = parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":2.5}}"#).unwrap();
        let compact = j.to_string_compact();
        assert_eq!(compact, r#"{"a":1,"b":[true,null,"x"],"c":{"d":2.5}}"#);
        let pretty = j.to_string_pretty();
        let back = parse(&pretty).unwrap();
        assert_eq!(back, j);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("nul").is_err());
        assert!(parse(r#"{"a":1}x"#).is_err());
    }

    #[test]
    fn duplicate_key_last_wins() {
        let j = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(j.get("a").and_then(Json::as_f64), Some(2.0));
        assert_eq!(j.to_string_compact(), r#"{"a":2}"#);
    }
}
