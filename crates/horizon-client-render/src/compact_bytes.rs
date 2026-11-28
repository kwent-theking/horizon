//! Lossless in-memory backing for resources needed again after GPU upload.
use std::borrow::Cow;

#[derive(Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct CompactBytes {
    #[serde(with = "byte_string")]
    bytes: Box<[u8]>,
    len: usize,
    compressed: bool,
}

#[derive(serde::Deserialize)]
struct CompactBytesWire {
    #[serde(with = "byte_string")]
    bytes: Box<[u8]>,
    len: usize,
    compressed: bool,
}

impl<'de> serde::Deserialize<'de> for CompactBytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = <CompactBytesWire as serde::Deserialize>::deserialize(deserializer)?;
        let value = Self {
            bytes: wire.bytes,
            len: wire.len,
            compressed: wire.compressed,
        };
        if value.is_valid() {
            Ok(value)
        } else {
            Err(serde::de::Error::custom("invalid compact texture backing"))
        }
    }
}
impl CompactBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        let packed = miniz_oxide::deflate::compress_to_vec(&bytes, 1);
        let len = bytes.len();
        if packed.len() < len {
            Self {
                bytes: packed.into_boxed_slice(),
                len,
                compressed: true,
            }
        } else {
            Self {
                bytes: bytes.into_boxed_slice(),
                len,
                compressed: false,
            }
        }
    }
    /// Compress a borrowed byte slice without cloning it first when compression shrinks it.
    pub fn from_slice(bytes: &[u8]) -> Self {
        let packed = miniz_oxide::deflate::compress_to_vec(bytes, 1);
        let len = bytes.len();
        if packed.len() < len {
            Self {
                bytes: packed.into_boxed_slice(),
                len,
                compressed: true,
            }
        } else {
            Self {
                bytes: bytes.to_vec().into_boxed_slice(),
                len,
                compressed: false,
            }
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn stored_len(&self) -> usize {
        self.bytes.len()
    }
    /// Validate disk backing with a hard expansion bound before any trusted use.
    pub fn is_valid(&self) -> bool {
        self.len <= MAX_COMPACT_BYTES
            && self.bytes.len() <= MAX_COMPACT_BYTES
            && if self.compressed {
                self.bytes.len() < self.len
                    && miniz_oxide::inflate::decompress_to_vec_with_limit(&self.bytes, self.len)
                        .is_ok_and(|bytes| bytes.len() == self.len)
            } else {
                self.bytes.len() == self.len
            }
    }
    pub fn decode(&self) -> Cow<'_, [u8]> {
        if self.compressed {
            // Disk snapshots validate this backing before exposing it to consumers.
            let bytes = miniz_oxide::inflate::decompress_to_vec(&self.bytes)
                .expect("in-memory resource snapshot");
            assert_eq!(bytes.len(), self.len);
            Cow::Owned(bytes)
        } else {
            Cow::Borrowed(&self.bytes)
        }
    }
    pub fn into_decoded(self) -> Vec<u8> {
        if self.compressed {
            self.decode().into_owned()
        } else {
            self.bytes.into_vec()
        }
    }
}

pub(crate) const MAX_COMPACT_BYTES: usize = 768 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_backing_rejects_invalid_compression_and_lengths() {
        let mut packed = CompactBytes::new(vec![7; 4096]);
        assert!(packed.is_valid());
        packed.len = 8;
        assert!(!packed.is_valid());
        packed.len = usize::MAX;
        assert!(!packed.is_valid());
        let mut raw = CompactBytes::new(vec![1, 2, 3]);
        raw.len = 4;
        assert!(!raw.is_valid());
        raw.compressed = true;
        assert!(!raw.is_valid());
    }

    #[test]
    fn compressed_and_raw_backings_preserve_every_byte_on_repeated_reads() {
        for bytes in [vec![], vec![1, 2, 3], [0, 12, 255, 128].repeat(16384)] {
            let packed = CompactBytes::new(bytes.clone());
            assert_eq!(packed.len(), bytes.len());
            assert!(packed.stored_len() <= bytes.len());
            for _ in 0..2 {
                assert_eq!(packed.decode().as_ref(), bytes);
            }
            assert_eq!(packed.into_decoded(), bytes);
        }
        let packed = CompactBytes::new(vec![99; 65536]);
        assert!(packed.stored_len() < 1024);
    }

    #[test]
    fn serde_round_trip_is_lossless_and_rejects_corrupt_or_oversized_backings() {
        for bytes in [vec![1, 2, 3], vec![7; 65536]] {
            let original = CompactBytes::new(bytes.clone());
            let encoded = postcard::to_allocvec(&original).unwrap();
            let restored: CompactBytes = postcard::from_bytes(&encoded).unwrap();
            assert_eq!(restored.decode().as_ref(), bytes);
            assert!(restored.is_valid());
        }

        for wire in [
            (vec![0xff, 0x00], 32usize, true),
            (vec![1u8], MAX_COMPACT_BYTES + 1, true),
            (vec![1u8, 2], 3usize, false),
        ] {
            let encoded = postcard::to_allocvec(&wire).unwrap();
            assert!(postcard::from_bytes::<CompactBytes>(&encoded).is_err());
        }
        let truncated = postcard::to_allocvec(&(vec![1u8], 8usize, true)).unwrap();
        assert!(postcard::from_bytes::<CompactBytes>(&truncated[..truncated.len() - 1]).is_err());
    }
}

// Keep pixel backing as one postcard byte string, rather than millions of
// single-byte serializer calls. No extra serialized pixel allocation is needed.
mod byte_string {
    use super::MAX_COMPACT_BYTES;
    pub fn serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<[u8]>, D::Error> {
        struct BoundedVisitor;
        impl<'de> serde::de::Visitor<'de> for BoundedVisitor {
            type Value = Box<[u8]>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded texture byte string")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|len| len > MAX_COMPACT_BYTES) {
                    return Err(serde::de::Error::custom(
                        "texture backing exceeds size limit",
                    ));
                }
                let mut bytes =
                    Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_COMPACT_BYTES));
                while let Some(byte) = seq.next_element()? {
                    if bytes.len() == MAX_COMPACT_BYTES {
                        return Err(serde::de::Error::custom(
                            "texture backing exceeds size limit",
                        ));
                    }
                    bytes.push(byte);
                }
                Ok(bytes.into_boxed_slice())
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                if bytes.len() > MAX_COMPACT_BYTES {
                    return Err(E::custom("texture backing exceeds size limit"));
                }
                Ok(bytes.into())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
                if bytes.len() > MAX_COMPACT_BYTES {
                    return Err(E::custom("texture backing exceeds size limit"));
                }
                Ok(bytes.into_boxed_slice())
            }
        }
        deserializer.deserialize_byte_buf(BoundedVisitor)
    }
}
