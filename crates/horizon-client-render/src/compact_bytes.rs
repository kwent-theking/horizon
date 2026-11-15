//! Lossless in-memory backing for resources needed again after GPU upload.
use std::borrow::Cow;

#[derive(PartialEq, Eq, Hash)]
pub struct CompactBytes {
    bytes: Box<[u8]>,
    len: usize,
    compressed: bool,
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
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn stored_len(&self) -> usize {
        self.bytes.len()
    }
    pub fn decode(&self) -> Cow<'_, [u8]> {
        if self.compressed {
            // Only our own encoder can construct this backing, never a disk cache.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
