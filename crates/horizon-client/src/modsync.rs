//! Server-delivered content blocks ("ModSync", phase 1).
//!
//! A Horizon-aware server can push the block definitions of its content
//! mods to the client during the configuration phase, on the
//! `horizon:modsync` custom-payload channel — so the client can see what
//! the server runs even when it has no matching mod installed (phase 2
//! will turn the definitions into a real content overlay; see
//! `mod_content.rs` and docs/modpacks.md).
//!
//! Wire format (`HMS1`): the ASCII magic `HMS1`, then a VarInt count of
//! entries, then per entry a VarInt-length-prefixed UTF-8 namespace, a
//! VarInt-length-prefixed UTF-8 path and a VarInt-length-prefixed JSON
//! block definition (the `data/<id>/horizon/block/*.json` file of a
//! content mod: a vanilla `template` plus changed `properties`). Lengths
//! and UTF-8 are validated and every read is bounds-checked; trailing
//! bytes after the last entry are ignored, so the format can grow without
//! breaking older clients. The delivery is one-way: the client answers
//! nothing.
//!
//! RU: блоки контент-модов, присланные сервером в configuration-фазе по
//! каналу `horizon:modsync`. Фаза 1 только декодирует и докладывает
//! (`Event::ModSync`); реестр блоков не меняется.

use horizon_protocol::buf::Reader;

/// The custom-payload channel the server sends ModSync on (configuration
/// phase, like `content_identity::CHANNEL`).
pub const CHANNEL: &str = "horizon:modsync";

/// The payload magic: "Horizon ModSync, format 1".
const MAGIC: &[u8; 4] = b"HMS1";

/// The vanilla `Identifier` string cap, for the namespace and the path.
const MAX_ID: usize = 32767;

/// Bytes allowed for one JSON block definition. A real one is a template
/// id plus a handful of numbers (well under 1 KiB); 64 KiB stays far
/// beyond a legitimate definition and far below anything worth attacking.
const MAX_JSON: usize = 64 * 1024;

/// A decoded ModSync payload: one `(namespace, path, json)` per block,
/// where `namespace:path` is the block's id and `json` is its definition
/// in the content-mod format (`template` + `properties`).
#[derive(Debug)]
pub(crate) struct ModSyncPayload {
    pub blocks: Vec<(String, String, String)>,
}

/// Decode a `horizon:modsync` payload.
///
/// Every error is a reason to drop this one payload, never the
/// connection: the caller reports it as a status message and carries on.
pub(crate) fn decode(data: &[u8]) -> Result<ModSyncPayload, String> {
    let body = data
        .strip_prefix(MAGIC.as_slice())
        .ok_or("expected the HMS1 magic")?;
    let mut r = Reader::new(body);
    // An entry costs at least three bytes (namespace, path, json lengths),
    // so capping the count at the bytes left bounds the loop to the input.
    let n = r
        .count(r.remaining())
        .map_err(|e| format!("entry count: {e}"))?;
    let mut blocks = Vec::with_capacity(n.min(1024));
    for i in 0..n {
        let context = |e: String| format!("entry {i}: {e}");
        let namespace = r
            .string(MAX_ID)
            .map_err(|e| context(format!("namespace: {e}")))?;
        let path = r
            .string(MAX_ID)
            .map_err(|e| context(format!("path: {e}")))?;
        let json = r
            .byte_array(MAX_JSON)
            .map_err(|e| context(format!("block definition: {e}")))?;
        let json =
            String::from_utf8(json).map_err(|e| context(format!("block definition: {e}")))?;
        blocks.push((namespace, path, json));
    }
    Ok(ModSyncPayload { blocks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;

    /// Build a payload the way the server plugin will: magic, VarInt count,
    /// then length-prefixed entries.
    fn payload(entries: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(MAGIC);
        w.varint(entries.len() as i32);
        for (namespace, path, json) in entries {
            w.string(namespace);
            w.string(path);
            w.byte_array(json.as_bytes());
        }
        w.data
    }

    #[test]
    fn empty_payload_decodes_to_no_blocks() {
        let p = decode(b"HMS1\x00").unwrap();
        assert!(p.blocks.is_empty());
    }

    #[test]
    fn one_block_matches_the_golden_bytes() {
        // Golden bytes for the server-side contract:
        // "HMS1" | 01 | 04 "demo" | 08 "ruby_ore" | 1F {...json...}
        let json = r#"{"template":"minecraft:stone"}"#;
        // len({"template":"minecraft:stone"}) == 30 == 0x1E (the coder
        // originally wrote 31, then 26; both miscounts, fixed by the
        // orchestrator with a byte-level length check).
        let mut golden: Vec<u8> = b"HMS1\x01\x04demo\x08ruby_ore\x1E".to_vec();
        golden.extend_from_slice(json.as_bytes());
        assert_eq!(json.len(), 0x1E);
        let p = decode(&golden).unwrap();
        assert_eq!(
            p.blocks,
            vec![("demo".to_owned(), "ruby_ore".to_owned(), json.to_owned())]
        );
        // The same bytes built through the Writer decode identically.
        assert_eq!(payload(&[("demo", "ruby_ore", json)]), golden);
    }

    #[test]
    fn three_blocks_round_trip() {
        let entries = [
            ("demo", "ruby_ore", r#"{"template":"minecraft:stone"}"#),
            (
                "demo",
                "deep_lamp",
                r#"{"template":"minecraft:sea_lantern","properties":{"light":15}}"#,
            ),
            (
                "other",
                "misty_glass",
                r#"{"template":"minecraft:glass","item":false}"#,
            ),
        ];
        let p = decode(&payload(&entries)).unwrap();
        assert_eq!(p.blocks.len(), 3);
        for (i, (namespace, path, json)) in p.blocks.into_iter().enumerate() {
            assert_eq!(namespace, entries[i].0);
            assert_eq!(path, entries[i].1);
            assert_eq!(json, entries[i].2);
        }
    }

    #[test]
    fn trailing_bytes_are_ignored() {
        let mut bytes = payload(&[("demo", "ruby_ore", r#"{"template":"minecraft:stone"}"#)]);
        bytes.extend_from_slice(b"\x00\x00future");
        assert_eq!(decode(&bytes).unwrap().blocks.len(), 1);
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        assert!(decode(b"").is_err());
        assert!(decode(b"HMS").is_err());
        assert!(decode(b"HMS1").is_err()); // No count.
        let e = decode(b"HMS2\x00").unwrap_err();
        assert!(e.contains("magic"));
    }

    #[test]
    fn a_truncated_entry_is_rejected() {
        let bytes = payload(&[("demo", "ruby_ore", r#"{"template":"minecraft:stone"}"#)]);
        // Cut the JSON body: the declared length now runs past the end.
        let e = decode(&bytes[..bytes.len() - 4]).unwrap_err();
        assert!(e.contains("entry 0") && e.contains("block definition"));
        // Cut the path itself.
        let e = decode(&bytes[..7]).unwrap_err();
        assert!(e.contains("entry 0"));
        // A declared count with no entries behind it.
        let e = decode(b"HMS1\x05").unwrap_err();
        assert!(e.contains("entry count"));
    }

    #[test]
    fn a_length_overflow_is_rejected() {
        // A JSON length of 100 with two bytes behind it.
        let e = decode(b"HMS1\x01\x02ns\x01p\x64ab").unwrap_err();
        assert!(e.contains("entry 0") && e.contains("block definition"));
        // A length larger than the cap even if the bytes existed.
        let mut w = Writer::new();
        w.bytes(MAGIC);
        w.varint(1);
        w.string("ns");
        w.string("p");
        w.varint(i32::MAX); // A length no payload can satisfy.
        let e = decode(&w.data).unwrap_err();
        assert!(e.contains("entry 0"));
    }

    #[test]
    fn non_utf8_strings_are_rejected() {
        // The JSON is two invalid UTF-8 bytes (\xC3\x28).
        let e = decode(b"HMS1\x01\x02ns\x01p\x02\xc3\x28").unwrap_err();
        assert!(e.contains("entry 0") && e.contains("block definition"));
        // A namespace in invalid UTF-8: VarInt(2) then 0xC3 0x28.
        let e = decode(b"HMS1\x01\x02\xc3\x28\x01p\x00").unwrap_err();
        assert!(e.contains("entry 0") && e.contains("namespace"));
    }
}
