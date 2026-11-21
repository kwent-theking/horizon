//! Disposable derived tables for an immutable, stamped content root.
//! Missing/stale/corrupt caches and failed writes use the source loader.
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};

const MAGIC: &[u8; 8] = b"HZTABLE1";
const MAX_BYTES: u64 = 1 << 30;
// A stage can contain many fields, but no individual string/blob needs the
// entire stage budget. Oversized fields take the authoritative loader path.
const MAX_FIELD_BYTES: usize = 768 * 1024 * 1024;
const SCRATCH_BYTES: usize = 65536;
const IMMUTABLE_SMALL_SOURCE_BYTES: u64 = 256 * 1024;
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

fn hash_file_bytes(path: &Path) -> Option<(u64, [u8; 32])> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; SCRATCH_BYTES];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        total = total.saturating_add(read as u64);
    }
    Some((total, hasher.finalize().into()))
}

fn current_exe_digest() -> Option<&'static (u64, [u8; 32])> {
    static DIGEST: OnceLock<Option<(u64, [u8; 32])>> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            hash_file_bytes(&exe)
        })
        .as_ref()
}

// Serialize directly into a bounded writer. In particular, cold snapshot writes
// must not allocate another atlas-sized postcard buffer beside the live data.
struct Hashed<W> {
    writer: W,
    hash: Sha256,
}
impl<W: Write> Write for Hashed<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.writer.write(bytes)?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
struct Output<W: Write> {
    writer: BufWriter<Hashed<W>>,
    bytes: u64,
}
impl<W: Write> Output<W> {
    fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(Hashed {
                writer,
                hash: Sha256::new(),
            }),
            bytes: 0,
        }
    }
}
impl<W: Write> postcard::ser_flavors::Flavor for Output<W> {
    type Output = (W, [u8; 32]);

    fn try_push(&mut self, byte: u8) -> postcard::Result<()> {
        self.try_extend(&[byte])
    }

    fn try_extend(&mut self, bytes: &[u8]) -> postcard::Result<()> {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if self.bytes > MAX_BYTES || self.writer.write_all(bytes).is_err() {
            return Err(postcard::Error::SerializeBufferFull);
        }
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        let hashed = self
            .writer
            .into_inner()
            .map_err(|_| postcard::Error::SerializeBufferFull)?;
        Ok((hashed.writer, hashed.hash.finalize().into()))
    }
}
impl<W: Write> Write for Output<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        use postcard::ser_flavors::Flavor;
        self.try_extend(bytes)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

// Buffered streaming input avoids holding a serialized snapshot beside its
// restored resource graph. Only owned fields may be deserialized from a file.
struct HashedReader {
    file: fs::File,
    hash: Sha256,
}
impl Read for HashedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(bytes)?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
}
struct Input {
    reader: BufReader<HashedReader>,
    remaining: usize,
    scratch: Vec<u8>,
    digest: [u8; 32],
}
impl<'de> postcard::de_flavors::Flavor<'de> for Input {
    type Source = Self;
    type Remainder = ();

    fn pop(&mut self) -> postcard::Result<u8> {
        // A visitor has copied the previous temporary field by this point.
        // Do not keep an atlas-sized scratch allocation beside the live graph.
        if self.scratch.capacity() > SCRATCH_BYTES {
            self.scratch = Vec::new();
        }
        if self.remaining == 0 {
            return Err(postcard::Error::DeserializeUnexpectedEnd);
        }
        let mut byte = [0];
        self.reader
            .read_exact(&mut byte)
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)?;
        self.remaining -= 1;
        Ok(byte[0])
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }

    fn try_take_n(&mut self, _: usize) -> postcard::Result<&'de [u8]> {
        Err(postcard::Error::DeserializeBadEncoding)
    }

    fn try_take_n_temp<'a>(&'a mut self, count: usize) -> postcard::Result<&'a [u8]>
    where
        'de: 'a,
    {
        if count > self.remaining || count > MAX_FIELD_BYTES {
            return Err(postcard::Error::DeserializeUnexpectedEnd);
        }
        if count <= SCRATCH_BYTES && self.scratch.capacity() > SCRATCH_BYTES {
            self.scratch = Vec::new();
        }
        self.scratch.resize(count, 0);
        self.reader
            .read_exact(&mut self.scratch)
            .map_err(|_| postcard::Error::DeserializeUnexpectedEnd)?;
        self.remaining -= count;
        Ok(&self.scratch)
    }

    fn finalize(mut self) -> postcard::Result<()> {
        let mut extra = [0];
        if self.remaining != 0
            || self
                .reader
                .read(&mut extra)
                .map_err(|_| postcard::Error::DeserializeBadEncoding)?
                != 0
            || self.reader.into_inner().hash.finalize()[..] != self.digest
        {
            return Err(postcard::Error::DeserializeBadEncoding);
        }
        Ok(())
    }
}

impl Cache {
    /// A source-stage key, for callers composing ordered multi-root identities.
    pub fn identity(&self) -> &[u8; 32] {
        &self.key
    }

    pub fn read_value<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        let (file, len, digest) = self.open()?;
        let input = Input {
            reader: BufReader::with_capacity(
                SCRATCH_BYTES,
                HashedReader {
                    file,
                    hash: Sha256::new(),
                },
            ),
            remaining: usize::try_from(len).ok()?,
            scratch: Vec::new(),
            digest,
        };
        let mut decoder = postcard::Deserializer::from_flavor(input);
        let value = T::deserialize(&mut decoder).ok()?;
        // No partially decoded value escapes before checksum/EOF validation.
        decoder.finalize().ok()?;
        Some(value)
    }

    pub fn write_value<T: serde::Serialize>(&self, value: &T) -> io::Result<()> {
        self.publish(|file| {
            file.write_all(&[0; 32])?;
            let (_, digest) = postcard::serialize_with_flavor(value, Output::new(&mut *file))
                .map_err(|e| io::Error::other(e.to_string()))?;
            file.seek(SeekFrom::Start(40))?;
            file.write_all(&digest)
        })
    }

    /// Include caller-supplied values as well as files in a stage's identity.
    /// This hashes without retaining a second serialized copy of the value.
    pub fn value_identity<T: serde::Serialize>(value: &T) -> Option<[u8; 32]> {
        postcard::serialize_with_flavor(value, Output::new(io::sink()))
            .ok()
            .map(|(_, digest)| digest)
    }

    /// JSON Values need type delimiters in their identity: postcard alone is
    /// not self-describing (for example, true and 1 have the same encoding).
    pub fn json_identity(value: &serde_json::Value) -> Option<[u8; 32]> {
        use postcard::ser_flavors::Flavor;
        let mut output = Output::new(io::sink());
        serde_json::to_writer(&mut output, value).ok()?;
        output.finalize().ok().map(|(_, digest)| digest)
    }

    /// Whether `root` is a packaged-mod content root verified by `mod_content`.
    /// Callers avoid traversing ~167k files across reloads when this holds.
    pub fn is_immutable_mod_root(root: &Path) -> bool {
        root.join("horizon-mod-content.stamp").is_file()
            && root.join("horizon-mod-content.json").is_file()
            && root.join(".horizon-derived/installed-mods-v1").is_file()
    }

    /// All inputs of a render tree, including linked base assets. Editable,
    /// unstamped roots still use the authoritative loader. Do not memoize this
    /// scan across reloads: a pack may have changed without rebuilding its root.
    pub fn for_tree(root: &Path, name: &str, identity: &[u8]) -> Option<Self> {
        if Self::is_immutable_mod_root(root) {
            let mut hash = Sha256::new();
            field(
                &mut hash,
                &fs::read(root.join("horizon-mod-content.stamp")).ok()?,
            );
            field(
                &mut hash,
                &fs::read(root.join("horizon-mod-content.json")).ok()?,
            );
            field(&mut hash, identity);
            let installed = root.join(".horizon-derived/installed-mods-v1");
            return Self::new(root, name, &[&installed], &hash.finalize());
        }
        fn visit(root: &Path, path: &Path, hash: &mut Sha256, depth: usize) -> Option<()> {
            if depth > 64 {
                return None;
            }
            let mut entries = fs::read_dir(path)
                .ok()?
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if path == root
                    && (entry.file_name() == ".horizon-derived"
                        || entry.file_name() == "horizon-sprites.cache")
                {
                    continue;
                }
                let path = entry.path();
                let kind = entry.file_type().ok()?;
                let meta = if kind.is_symlink() {
                    fs::metadata(&path).ok()?
                } else {
                    entry.metadata().ok()?
                };
                field(hash, path.strip_prefix(root).ok()?.to_str()?.as_bytes());
                if meta.is_dir() {
                    hash.update([0]);
                    visit(root, &path, hash, depth + 1)?;
                } else if meta.is_file() {
                    hash.update([1]);
                    hash.update(meta.len().to_le_bytes());
                    hash.update(
                        meta.modified()
                            .ok()?
                            .duration_since(UNIX_EPOCH)
                            .ok()?
                            .as_nanos()
                            .to_le_bytes(),
                    );
                } else {
                    return None;
                }
            }
            Some(())
        }
        // Check the stamp before traversing an uncached development tree.
        let mut hash = Sha256::new();
        field(
            &mut hash,
            &fs::read(root.join("horizon-mod-content.stamp")).ok()?,
        );
        field(&mut hash, identity);
        visit(root, root, &mut hash, 0)?;
        Self::new(root, name, &[], &hash.finalize())
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
        field(&mut hash, horizon_core::version::VERSION_ID.as_bytes());
        field(&mut hash, horizon_core::release::VERSION.as_bytes());
        field(
            &mut hash,
            horizon_core::release::NIGHTLY.unwrap_or("").as_bytes(),
        );
        field(
            &mut hash,
            option_env!("HORIZON_COMMIT").unwrap_or("").as_bytes(),
        );
        field(&mut hash, env!("CARGO_PKG_VERSION").as_bytes());
        field(&mut hash, identity);
        let &(exe_len, exe_digest) = current_exe_digest()?;
        hash.update(exe_len.to_le_bytes());
        hash.update(exe_digest);
        let immutable = Self::is_immutable_mod_root(root);
        for path in sources {
            let meta = fs::metadata(path).ok()?;
            if immutable && let Ok(rel) = path.strip_prefix(root) {
                // Packaged mod roots move across directories and archive extracts,
                // so root-relative sources must not key on host prefixes or mtimes.
                let normalized = rel.to_string_lossy().replace('\\', "/");
                field(&mut hash, b"rel");
                field(&mut hash, normalized.as_bytes());
                hash.update(meta.len().to_le_bytes());
                if meta.len() <= IMMUTABLE_SMALL_SOURCE_BYTES {
                    let (len, digest) = hash_file_bytes(path)?;
                    hash.update(len.to_le_bytes());
                    hash.update(digest);
                }
                continue;
            }
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

    fn open(&self) -> Option<(fs::File, u64, [u8; 32])> {
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
        Some((file, len, header[40..72].try_into().ok()?))
    }

    pub fn read(&self) -> Option<Vec<u8>> {
        let (file, len, digest) = self.open()?;
        let mut payload = Vec::with_capacity(len as usize);
        file.take(MAX_BYTES + 1).read_to_end(&mut payload).ok()?;
        if payload.len() as u64 != len || digest != Sha256::digest(&payload)[..] {
            return None;
        }
        Some(payload)
    }

    pub fn write(&self, payload: &[u8]) -> io::Result<()> {
        if payload.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("derived table exceeds cache byte limit"));
        }
        self.publish(|file| {
            file.write_all(&Sha256::digest(payload))?;
            file.write_all(payload)
        })
    }

    fn publish(&self, write: impl FnOnce(&mut fs::File) -> io::Result<()>) -> io::Result<()> {
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
            write(&mut file)?;
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

    fn fixture(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "horizon-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn streamed_blob_scratch_is_released_and_oversized_fields_are_rejected() {
        use postcard::de_flavors::Flavor;
        let dir = fixture("streamed-blob-scratch");
        let cache = Cache {
            path: dir.join("snapshot"),
            key: [21; 32],
        };
        let mut payload = vec![7; SCRATCH_BYTES * 2];
        payload.push(42);
        cache.write(&payload).unwrap();
        let (file, len, digest) = cache.open().unwrap();
        let mut input = Input {
            reader: BufReader::new(HashedReader {
                file,
                hash: Sha256::new(),
            }),
            remaining: len as usize,
            scratch: Vec::new(),
            digest,
        };
        assert_eq!(
            input.try_take_n_temp(payload.len() - 1).unwrap(),
            &payload[..payload.len() - 1]
        );
        assert!(input.scratch.capacity() > SCRATCH_BYTES);
        assert_eq!(input.pop().unwrap(), 42);
        assert_eq!(input.scratch.capacity(), 0);
        input.finalize().unwrap();

        let (file, _, digest) = cache.open().unwrap();
        let mut input = Input {
            reader: BufReader::new(HashedReader {
                file,
                hash: Sha256::new(),
            }),
            remaining: MAX_BYTES as usize,
            scratch: Vec::new(),
            digest,
        };
        assert!(input.try_take_n_temp(MAX_FIELD_BYTES + 1).is_err());
        assert_eq!(input.scratch.capacity(), 0);
        assert_eq!(input.remaining, MAX_BYTES as usize);
        // Release the open file before removing the fixture on Windows.
        drop(input);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn streamed_value_matches_slice_encoding_and_rejects_trailing_bytes() {
        let dir = fixture("streamed-cache");
        let cache = Cache {
            path: dir.join("snapshot"),
            key: [42; 32],
        };
        let value = (
            "streamed string".repeat(8192),
            (0..131072).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        );
        cache.write_value(&value).unwrap();
        assert_eq!(
            cache.read().unwrap(),
            postcard::to_allocvec(&value).unwrap()
        );
        assert_eq!(cache.read_value::<(String, Vec<u8>)>(), Some(value.clone()));
        let stale = Cache {
            path: cache.path.clone(),
            key: [43; 32],
        };
        assert!(stale.read_value::<(String, Vec<u8>)>().is_none());
        let original = fs::read(&cache.path).unwrap();
        let mut corrupt = original.clone();
        // The payload still decodes: only its checksum reveals the changed byte.
        *corrupt.last_mut().unwrap() ^= 1;
        for bytes in [
            &original[..40],
            &original[..original.len() - 1],
            &corrupt[..],
        ] {
            fs::write(&cache.path, bytes).unwrap();
            assert!(cache.read_value::<(String, Vec<u8>)>().is_none());
        }
        let mut trailing = postcard::to_allocvec(&value).unwrap();
        trailing.push(0);
        cache.write(&trailing).unwrap();
        assert!(cache.read_value::<(String, Vec<u8>)>().is_none());
        let mut corrupt = fs::read(&cache.path).unwrap();
        *corrupt.last_mut().unwrap() ^= 1;
        fs::write(&cache.path, corrupt).unwrap();
        assert!(cache.read_value::<(String, Vec<u8>)>().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_serialization_preserves_published_file_and_removes_temporary() {
        struct Broken;
        impl serde::Serialize for Broken {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture serialization failure"))
            }
        }
        let dir = fixture("failed-cache-write");
        let cache = Cache {
            path: dir.join("snapshot"),
            key: [13; 32],
        };
        cache.write_value(&vec![1u32, 2, 3]).unwrap();
        assert!(cache.write_value(&Broken).is_err());
        assert_eq!(cache.read_value::<Vec<u32>>(), Some(vec![1, 2, 3]));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tree_identity_includes_nested_assets_but_excludes_derived_caches() {
        let dir = fixture("tree-cache");
        fs::write(dir.join("horizon-mod-content.stamp"), b"stamp").unwrap();
        let assets = dir.join("assets").join("test");
        fs::create_dir_all(&assets).unwrap();
        let source = assets.join("model.json");
        fs::write(&source, b"source").unwrap();
        let cache = Cache::for_tree(&dir, "tree-v1", b"settings").unwrap();
        cache.write(b"snapshot").unwrap();
        fs::write(
            dir.join("horizon-sprites.cache"),
            b"disposable sprite cache",
        )
        .unwrap();
        let current = Cache::for_tree(&dir, "tree-v1", b"settings").unwrap();
        assert_eq!(current.read().as_deref(), Some(b"snapshot".as_slice()));
        fs::write(&source, b"changed nested asset").unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", b"settings")
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_file(source).unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", b"settings")
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_dir_all(dir).unwrap();
    }

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
    #[test]
    fn typed_trailing_bytes_and_json_type_changes_are_rejected() {
        let dir = std::env::temp_dir().join(format!("horizon-table-typed-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cache = Cache {
            path: dir.join("typed-v1"),
            key: [3; 32],
        };
        cache.write(&[1, 42, 0]).unwrap();
        assert!(cache.read_value::<Vec<u8>>().is_none());
        assert_ne!(
            Cache::json_identity(&serde_json::json!(true)),
            Cache::json_identity(&serde_json::json!(1))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tree_keys_cover_edits_additions_deletions_and_ignore_derived_files() {
        let dir = std::env::temp_dir().join(format!("horizon-tree-key-{}", std::process::id()));
        fs::create_dir_all(dir.join("assets/test")).unwrap();
        assert!(Cache::for_tree(&dir, "tree-v1", &[]).is_none());
        fs::write(dir.join("horizon-mod-content.stamp"), "tree-a").unwrap();
        let source = dir.join("assets/test/model.json");
        fs::write(&source, "{}").unwrap();
        let cache = Cache::for_tree(&dir, "tree-v1", &[]).unwrap();
        cache.write_value(&vec![1u32, 2, 3]).unwrap();
        assert_eq!(
            Cache::for_tree(&dir, "tree-v1", &[])
                .unwrap()
                .read_value::<Vec<u32>>(),
            Some(vec![1, 2, 3])
        );
        assert!(
            Cache::for_tree(&dir, "tree-v2", &[])
                .unwrap()
                .read()
                .is_none()
        );
        fs::write(dir.join("assets/test/new.json"), "{}").unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", &[])
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_file(dir.join("assets/test/new.json")).unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", &[])
                .unwrap()
                .read()
                .is_some()
        );
        fs::write(&source, "changed").unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", &[])
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_file(&source).unwrap();
        assert!(
            Cache::for_tree(&dir, "tree-v1", &[])
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn immutable_mod_root_invalidates_on_stamp_namespaces_or_marker_changes() {
        let dir = fixture("immutable-mod-root");
        assert!(!Cache::is_immutable_mod_root(&dir));
        fs::write(dir.join("horizon-mod-content.stamp"), "stamp-1").unwrap();
        fs::write(dir.join("horizon-mod-content.json"), "[\"mod_a\"]").unwrap();
        fs::create_dir_all(dir.join(".horizon-derived")).unwrap();
        fs::write(dir.join(".horizon-derived/installed-mods-v1"), b"v1").unwrap();
        assert!(Cache::is_immutable_mod_root(&dir));

        let cache = Cache::for_tree(&dir, "fast-tree-v1", b"cfg").unwrap();
        cache.write(b"cached-payload").unwrap();
        assert_eq!(
            Cache::for_tree(&dir, "fast-tree-v1", b"cfg")
                .unwrap()
                .read()
                .as_deref(),
            Some(b"cached-payload".as_slice())
        );
        fs::write(
            dir.join("horizon-mod-content.json"),
            "[\"mod_a\",\"mod_b\"]",
        )
        .unwrap();
        assert!(
            Cache::for_tree(&dir, "fast-tree-v1", b"cfg")
                .unwrap()
                .read()
                .is_none()
        );
        let cache = Cache::for_tree(&dir, "fast-tree-v1", b"cfg").unwrap();
        cache.write(b"cached-payload-2").unwrap();
        fs::write(dir.join(".horizon-derived/installed-mods-v1"), b"v2-longer").unwrap();
        assert!(
            Cache::for_tree(&dir, "fast-tree-v1", b"cfg")
                .unwrap()
                .read()
                .is_none()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn immutable_mod_root_caches_survive_relocation_and_mtime_changes() {
        let root1 = fixture("immutable-reloc-src");
        let root2 = fixture("immutable-reloc-dst");
        let init_root = |root: &Path| {
            fs::write(root.join("horizon-mod-content.stamp"), b"stamp-portable").unwrap();
            fs::write(root.join("horizon-mod-content.json"), br#"["create"]"#).unwrap();
            fs::write(root.join("facts.json"), br#"{"blocks":1}"#).unwrap();
            fs::create_dir_all(root.join(".horizon-derived")).unwrap();
            fs::write(
                root.join(".horizon-derived/installed-mods-v1"),
                b"validated",
            )
            .unwrap();
        };
        init_root(&root1);

        let src1 = root1.join("facts.json");
        Cache::new(&root1, "table-reloc-v1", &[&src1], b"cfg")
            .unwrap()
            .write(b"table-bytes")
            .unwrap();
        Cache::for_tree(&root1, "tree-reloc-v1", b"cfg")
            .unwrap()
            .write(b"tree-bytes")
            .unwrap();

        init_root(&root2);
        let src2 = root2.join("facts.json");
        let touched = fs::File::options().write(true).open(&src2).unwrap();
        touched
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(touched);
        let installed2 = fs::File::options()
            .write(true)
            .open(root2.join(".horizon-derived/installed-mods-v1"))
            .unwrap();
        installed2
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(installed2);

        fs::copy(
            root1.join(".horizon-derived/table-reloc-v1"),
            root2.join(".horizon-derived/table-reloc-v1"),
        )
        .unwrap();
        fs::copy(
            root1.join(".horizon-derived/tree-reloc-v1"),
            root2.join(".horizon-derived/tree-reloc-v1"),
        )
        .unwrap();

        assert_eq!(
            Cache::new(&root2, "table-reloc-v1", &[&src2], b"cfg")
                .unwrap()
                .read()
                .as_deref(),
            Some(b"table-bytes".as_slice())
        );
        assert_eq!(
            Cache::for_tree(&root2, "tree-reloc-v1", b"cfg")
                .unwrap()
                .read()
                .as_deref(),
            Some(b"tree-bytes".as_slice())
        );

        // Editing a tracked source file still invalidates even with the same length.
        fs::write(&src2, br#"{"blocks":2}"#).unwrap();
        assert!(
            Cache::new(&root2, "table-reloc-v1", &[&src2], b"cfg")
                .unwrap()
                .read()
                .is_none()
        );

        fs::remove_dir_all(root1).unwrap();
        fs::remove_dir_all(root2).unwrap();
    }
}
