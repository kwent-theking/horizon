//! Disposable derived tables for an immutable, stamped content root.
//! Missing/stale/corrupt caches and failed writes use the source loader.
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::UNIX_EPOCH,
};

const MAGIC: &[u8; 8] = b"HZTABLE1";
const MAX_BYTES: u64 = 1 << 30;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(PartialEq, Eq)]
pub struct Cache {
    path: PathBuf,
    key: [u8; 32],
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

impl Cache {
    pub fn read_value<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        postcard::from_bytes(&self.read()?).ok()
    }

    pub fn write_value<T: serde::Serialize>(&self, value: &T) -> io::Result<()> {
        let bytes = postcard::to_allocvec(value).map_err(|e| io::Error::other(e.to_string()))?;
        self.write(&bytes)
    }

    /// `name` includes the table format version. The content stamp, build,
    /// source file metadata and caller's extra identity all participate in the key.
    /// Unstamped roots (including editable development resources) are never cached.
    pub fn new(root: &Path, name: &str, sources: &[&Path], identity: &[u8]) -> Option<Self> {
        let mut hash = Sha256::new();
        field(
            &mut hash,
            &fs::read(root.join("horizon-mod-content.stamp")).ok()?,
        );
        field(&mut hash, name.as_bytes());
        field(&mut hash, identity);
        let exe = std::env::current_exe().ok()?;
        for path in std::iter::once(exe.as_path()).chain(sources.iter().copied()) {
            let meta = fs::metadata(path).ok()?;
            field(&mut hash, path.to_str()?.as_bytes());
            hash.update(meta.len().to_le_bytes());
            hash.update(
                meta.modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_nanos()
                    .to_le_bytes(),
            );
        }
        Some(Self {
            path: root.join(".horizon-derived").join(name),
            key: hash.finalize().into(),
        })
    }

    pub fn read(&self) -> Option<Vec<u8>> {
        let mut file = fs::File::open(&self.path).ok()?;
        let mut header = [0; 72];
        file.read_exact(&mut header).ok()?;
        // Reject stale keys before allocating or reading a large old payload.
        if &header[..8] != MAGIC || header[8..40] != self.key {
            return None;
        }
        let len = file.metadata().ok()?.len().checked_sub(72)?;
        if len > MAX_BYTES {
            return None;
        }
        let mut payload = Vec::with_capacity(len as usize);
        file.take(MAX_BYTES + 1).read_to_end(&mut payload).ok()?;
        if payload.len() as u64 > MAX_BYTES || header[40..72] != Sha256::digest(&payload)[..] {
            return None;
        }
        Some(payload)
    }

    pub fn write(&self, payload: &[u8]) -> io::Result<()> {
        if payload.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("derived table exceeds cache byte limit"));
        }
        fs::create_dir_all(self.path.parent().unwrap())?;
        let temp = self.path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(MAGIC)?;
            file.write_all(&self.key)?;
            file.write_all(&Sha256::digest(payload))?;
            file.write_all(payload)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)
        })();
        let _ = fs::remove_file(temp);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_round_trip_and_key_inputs() {
        let dir = std::env::temp_dir().join(format!(
            "horizon-table-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.json");
        fs::write(&source, b"source").unwrap();
        assert!(Cache::new(&dir, "table-v1", &[&source], b"registry-a").is_none());
        fs::write(dir.join("horizon-mod-content.stamp"), b"content-a").unwrap();
        let cache = Cache::new(&dir, "table-v1", &[&source], b"registry-a").unwrap();
        let value = vec![("first".to_owned(), vec![1u32, 2, 3])];
        cache.write_value(&value).unwrap();
        assert_eq!(cache.read_value::<Vec<(String, Vec<u32>)>>(), Some(value));
        for (name, identity) in [("table-v2", b"registry-a"), ("table-v1", b"registry-b")] {
            assert!(
                Cache::new(&dir, name, &[&source], identity)
                    .unwrap()
                    .read()
                    .is_none()
            );
        }
        // A replacement is published at the same path, without retaining a
        // partial file. Readers of an old key must miss after replacement.
        let replacement = Cache::new(&dir, "table-v1", &[&source], b"registry-b").unwrap();
        replacement.write(b"replacement").unwrap();
        assert_eq!(
            replacement.read().as_deref(),
            Some(b"replacement".as_slice())
        );
        assert!(cache.read().is_none());
        fs::write(&source, b"a different source length").unwrap();
        assert!(
            Cache::new(&dir, "table-v1", &[&source], b"registry-b")
                .unwrap()
                .read()
                .is_none()
        );
        // Same size, changed timestamp also invalidates.
        let before = Cache::new(&dir, "table-v1", &[&source], b"registry-b").unwrap();
        before.write(b"current source").unwrap();
        let file = fs::File::options().write(true).open(&source).unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            Cache::new(&dir, "table-v1", &[&source], b"registry-b")
                .unwrap()
                .read()
                .is_none()
        );
        drop(file);
        let current = Cache::new(&dir, "table-v1", &[&source], b"registry-b").unwrap();
        current.write(b"current source").unwrap();
        assert!(current.read().is_some());
        fs::write(dir.join("horizon-mod-content.stamp"), b"content-b").unwrap();
        assert!(
            Cache::new(&dir, "table-v1", &[&source], b"registry-b")
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trip_stale_key_and_corruption() {
        let dir = std::env::temp_dir().join(format!("horizon-table-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cache = Cache {
            path: dir.join("table-v1"),
            key: [1; 32],
        };
        cache.write(b"derived table").unwrap();
        assert_eq!(cache.read().as_deref(), Some(b"derived table".as_slice()));
        let stale = Cache {
            path: cache.path.clone(),
            key: [2; 32],
        };
        assert!(stale.read().is_none());
        let mut bytes = fs::read(&cache.path).unwrap();
        bytes[72] ^= 1;
        fs::write(&cache.path, &bytes).unwrap();
        assert!(cache.read().is_none());
        fs::write(&cache.path, &bytes[..50]).unwrap();
        assert!(cache.read().is_none());
        // A complete file with a valid checksum can still contain an
        // unsupported typed payload; callers must fall back to the loader.
        cache.write(&[0xff]).unwrap();
        assert!(cache.read_value::<Vec<u32>>().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
