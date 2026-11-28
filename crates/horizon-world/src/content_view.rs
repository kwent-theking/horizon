//! The files of a content root that stay in the archives they came in.
//!
//! A cold build of the mods' content root (`mod_content` in horizon-client)
//! wrote every packaged mod file into the root: about 200,000 small files for
//! a large modpack, minutes of file creation, then an antivirus scan of each
//! one on its first read. The parts of a mod that only Horizon's own loaders
//! read stay in its `.hmod` instead, behind an index the build saves in the
//! root ([`FILES`]) and registers here ([`Index::mount`]).
//!
//! Those loaders use this module for file reads, existence checks and directory
//! listings. Indexed paths read from their archives and all other paths read
//! from disk. The build never writes a file it
//! serves, and a root without an index (vanilla's, a resource-pack overlay)
//! reads exactly as it did.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Seek, SeekFrom},
    ops::Bound,
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};

/// The index's file in a root.
pub const FILES: &str = "horizon-mod-content.files";
/// Its format. Bump when [`Index`] changes: a root saved with another is rebuilt.
const MAGIC: &[u8; 8] = b"HZFILES1";
/// More than a content file may take, packed or not: an index naming more is damaged.
const MAX_ENTRY: u64 = 64 << 20;

/// Where one file's bytes are in its archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    /// The archive's number ([`Index::add_archive`]).
    pub archive: u32,
    /// Where its bytes start in the archive, after the entry's local header.
    pub offset: u64,
    pub compressed: u64,
    pub size: u64,
    pub crc32: u32,
    /// Raw deflate; else stored as is.
    pub deflated: bool,
}

/// An archive by what tells the installed copy apart, never by where it is:
/// a root moves between installs (its caches are portable) and the mods
/// folder moves with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ArchiveName {
    name: String,
    len: u64,
}

/// The files a root serves from archives, by root-relative `/` path.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    archives: Vec<ArchiveName>,
    files: BTreeMap<String, Stored>,
}

impl Index {
    /// Adds the archive at `path`; its number goes in [`Stored::archive`].
    pub fn add_archive(&mut self, path: &Path) -> io::Result<u32> {
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| io::Error::other(format!("{}: no file name", path.display())))?;
        let len = fs::metadata(path)?.len();
        self.archives.push(ArchiveName {
            name: name.to_owned(),
            len,
        });
        Ok((self.archives.len() - 1) as u32)
    }

    /// Serves `rel` from an archive. A later call for the same file wins, as
    /// a later mod's file overwrote an earlier one's on disk.
    pub fn insert(&mut self, rel: &str, stored: Stored) {
        self.files.insert(rel.to_owned(), stored);
    }

    /// Stops serving `rel`: the build writes it on disk instead.
    pub fn remove(&mut self, rel: &str) {
        self.files.remove(rel);
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Saves the index in `root` ([`FILES`]).
    pub fn save(&self, root: &Path) -> io::Result<()> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(postcard::to_allocvec(self).map_err(|e| io::Error::other(e.to_string()))?);
        let pending = root.join(format!("{FILES}.{}.tmp", std::process::id()));
        fs::write(&pending, bytes)?;
        fs::rename(&pending, root.join(FILES))
    }

    /// The index [`Self::save`] left in `root`.
    pub fn load(root: &Path) -> io::Result<Self> {
        let bytes = fs::read(root.join(FILES))?;
        let damaged = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{FILES} is not an index of this build"),
            )
        };
        let index: Index = postcard::from_bytes(bytes.strip_prefix(MAGIC).ok_or_else(damaged)?)
            .map_err(|_| damaged())?;
        if index
            .files
            .values()
            .any(|stored| stored.archive as usize >= index.archives.len())
        {
            return Err(damaged());
        }
        Ok(index)
    }

    /// Serves these files under `root` from now on, in place of any index
    /// `root` had. Each archive is the one of `candidates` with its name and
    /// length; none fails the whole index, so a root is never served in part.
    pub fn mount(self, root: &Path, candidates: &[PathBuf]) -> Result<(), String> {
        let archives = self
            .archives
            .iter()
            .map(|archive| {
                candidates
                    .iter()
                    .find(|path| {
                        path.file_name() == Some(OsStr::new(&archive.name))
                            && fs::metadata(path).is_ok_and(|m| m.len() == archive.len)
                    })
                    .map(|path| Archive {
                        path: path.clone(),
                        file: OnceLock::new(),
                    })
                    .ok_or_else(|| {
                        format!(
                            "{} ({} bytes) is not among the installed mods",
                            archive.name, archive.len
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let root = std::path::absolute(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let view = Arc::new(View {
            root,
            files: self.files,
            archives,
        });
        let mut views = VIEWS.write().unwrap_or_else(|e| e.into_inner());
        views.retain(|v| v.root != view.root);
        views.push(view);
        Ok(())
    }
}

/// Serves nothing under `root` any more: it is about to be rebuilt.
pub fn unmount(root: &Path) {
    let Ok(root) = std::path::absolute(root) else {
        return;
    };
    VIEWS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|v| v.root != root);
}

/// The bytes of the file at `path`.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if let Some((view, rel, stored)) = stored_file(path) {
        return view.read(&rel, &stored);
    }
    fs::read(path)
}

trait BufferedSeek: io::BufRead + io::Seek {}
impl<T: io::BufRead + io::Seek> BufferedSeek for T {}

/// Opens a buffered, seekable reader for decoders. Disk files keep streaming
/// from disk, while archive entries are checked before their bytes are exposed.
pub fn open(path: &Path) -> io::Result<impl io::BufRead + io::Seek> {
    let reader: Box<dyn BufferedSeek> = if let Some((view, rel, stored)) = stored_file(path) {
        Box::new(io::Cursor::new(view.read(&rel, &stored)?))
    } else {
        Box::new(io::BufReader::new(fs::File::open(path)?))
    };
    Ok(reader)
}

trait SeekableRead: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> SeekableRead for T {}

/// One archive entry with its own seek position.
pub struct ArchiveStream {
    reader: Box<dyn SeekableRead>,
    size: u64,
}

impl ArchiveStream {
    pub fn byte_len(&self) -> u64 {
        self.size
    }
}

impl Read for ArchiveStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Seek for ArchiveStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader.seek(pos)
    }
}

/// Opens an indexed entry for a streaming decoder. Stored entries are read within their
/// ZIP range, without buffering the whole entry. Deflate entries use the checked reader.
/// A decoder can verify the stored media as it reads, since ZIP CRC validation needs a full pass.
pub fn open_archive_stream(path: &Path) -> Option<io::Result<ArchiveStream>> {
    let (view, rel, stored) = stored_file(path)?;
    Some((|| {
        let reader: Box<dyn SeekableRead> = if stored.deflated {
            Box::new(io::Cursor::new(view.read(&rel, &stored)?))
        } else {
            let archive = view.archives.get(stored.archive as usize).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{rel}: no archive"))
            })?;
            let bytes = archive.file()?.metadata()?.len();
            if stored.size > MAX_ENTRY
                || stored.compressed != stored.size
                || stored
                    .offset
                    .checked_add(stored.size)
                    .is_none_or(|end| end > bytes)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid archive byte range",
                ));
            }
            Box::new(StoredStream {
                view,
                stored,
                position: 0,
            })
        };
        Ok(ArchiveStream {
            reader,
            size: stored.size,
        })
    })())
}

struct StoredStream {
    view: Arc<View>,
    stored: Stored,
    position: u64,
}

impl Read for StoredStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.stored.size.saturating_sub(self.position) as usize;
        let len = remaining.min(buf.len());
        if len != 0 {
            let file = self.view.archives[self.stored.archive as usize].file()?;
            read_exact_at(file, &mut buf[..len], self.stored.offset + self.position)?;
            self.position += len as u64;
        }
        Ok(len)
    }
}

impl Seek for StoredStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.position = match pos {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(delta) => self.stored.size.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive seek"))?;
        Ok(self.position)
    }
}

/// Uncompressed length and CRC32 of a served file. Derived caches use these
/// in place of filesystem metadata, independently of where the root is mounted.
pub fn identity(path: &Path) -> Option<(u64, u32)> {
    let (_, _, stored) = stored_file(path)?;
    Some((stored.size, stored.crc32))
}

/// Whether a file is served from an archive or exists on disk.
pub fn is_file(path: &Path) -> bool {
    identity(path).is_some() || path.is_file()
}

/// The text of the file at `path`.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Whether `path` is a folder: on disk, or one served files are in.
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    find(path).is_some_and(|(view, rel)| view.has_folder(&rel)) || path.is_dir()
}

/// One name in a folder ([`read_dir`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    path: PathBuf,
    is_dir: bool,
    served: Option<(u64, u32)>,
    disk: Option<(u64, Option<std::time::SystemTime>)>,
}
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> OsString {
        self.path.file_name().unwrap_or_default().to_owned()
    }
    /// A folder, a link to one included.
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
    /// A file served from an archive: its length and CRC32, as [`identity`]
    /// gives them. Walks read it from here instead of looking every file up
    /// again: a game folder holds hundreds of thousands, and none of the
    /// disk's are in any index.
    pub fn served(&self) -> Option<(u64, u32)> {
        self.served
    }
    /// A file on disk: its length and modification time, as the folder
    /// listing gave them (Windows hands them over with each name, so a walk
    /// needs no second look at every file, which an antivirus would also check).
    pub fn disk(&self) -> Option<(u64, Option<std::time::SystemTime>)> {
        self.disk
    }
}

/// The names in the folder at `path`, the disk's and the served ones, each
/// once and in no particular order.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<Vec<DirEntry>> {
    let path = path.as_ref();
    let served: Vec<(String, bool, Option<(u64, u32)>)> = find(path)
        .map(|(view, rel)| {
            let prefix = folder_prefix(&rel);
            view.children(&rel)
                .into_iter()
                .map(|(name, is_dir)| {
                    let stored = (!is_dir)
                        .then(|| view.files.get(&format!("{prefix}{name}")))
                        .flatten()
                        .map(|s| (s.size, s.crc32));
                    (name, is_dir, stored)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut entries = Vec::new();
    match fs::read_dir(path) {
        Ok(listing) => {
            for entry in listing.flatten() {
                let path = entry.path();
                let metadata = match entry.file_type() {
                    Ok(t) if !t.is_symlink() => entry.metadata().ok(),
                    _ => fs::metadata(&path).ok(),
                };
                let is_dir = metadata.as_ref().is_some_and(fs::Metadata::is_dir);
                let disk = metadata
                    .filter(fs::Metadata::is_file)
                    .map(|m| (m.len(), m.modified().ok()));
                entries.push(DirEntry {
                    path,
                    is_dir,
                    served: None,
                    disk,
                });
            }
        }
        // A folder only served files are in has none on disk.
        Err(e) if served.is_empty() => return Err(e),
        Err(_) => {}
    }
    if !served.is_empty() {
        let on_disk: HashSet<OsString> = entries.iter().map(DirEntry::file_name).collect();
        for (name, is_dir, stored) in served {
            if !on_disk.contains(OsStr::new(&name)) {
                entries.push(DirEntry {
                    path: path.join(name),
                    is_dir,
                    served: stored,
                    disk: None,
                });
            }
        }
    }
    Ok(entries)
}

static VIEWS: RwLock<Vec<Arc<View>>> = RwLock::new(Vec::new());

/// A mounted index: its root, as an absolute path, and its archives found.
struct View {
    root: PathBuf,
    files: BTreeMap<String, Stored>,
    archives: Vec<Archive>,
}
impl View {
    fn read(&self, rel: &str, stored: &Stored) -> io::Result<Vec<u8>> {
        let archive = self.archives.get(stored.archive as usize).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{rel}: no archive"))
        })?;
        archive.read(stored).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("{rel} in {}: {e}", archive.path.display()),
            )
        })
    }

    fn first_from(&self, from: &str) -> Option<&str> {
        self.files
            .range::<str, _>((Bound::Included(from), Bound::Unbounded))
            .next()
            .map(|(name, _)| name.as_str())
    }

    fn has_folder(&self, rel: &str) -> bool {
        let prefix = folder_prefix(rel);
        self.first_from(&prefix)
            .is_some_and(|name| name.starts_with(&prefix))
    }

    /// The names right in the folder `rel`, each once, and whether each is a
    /// folder. A folder's files sort together, so the walk jumps past them.
    fn children(&self, rel: &str) -> Vec<(String, bool)> {
        let prefix = folder_prefix(rel);
        let mut out = Vec::new();
        let mut from = prefix.clone();
        while let Some(name) = self.first_from(&from) {
            let Some(rest) = name.strip_prefix(prefix.as_str()) else {
                break;
            };
            match rest.split_once('/') {
                Some((folder, _)) => {
                    out.push((folder.to_owned(), true));
                    // '0' follows '/': the first name past the folder's.
                    from = format!("{prefix}{folder}0");
                }
                None => {
                    out.push((rest.to_owned(), false));
                    from = format!("{name}\0");
                }
            }
        }
        out
    }
}

fn folder_prefix(rel: &str) -> String {
    if rel.is_empty() {
        String::new()
    } else {
        format!("{rel}/")
    }
}

/// An archive, opened once on its first read and then shared by every
/// thread: each read says where, so no cursor is shared.
struct Archive {
    path: PathBuf,
    file: OnceLock<Result<fs::File, String>>,
}
impl Archive {
    fn file(&self) -> io::Result<&fs::File> {
        self.file
            .get_or_init(|| fs::File::open(&self.path).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| io::Error::other(e.clone()))
    }

    fn read(&self, stored: &Stored) -> io::Result<Vec<u8>> {
        if stored.compressed > MAX_ENTRY || stored.size > MAX_ENTRY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "larger than a content file may be",
            ));
        }
        let file = self.file()?;
        let mut packed = vec![0; stored.compressed as usize];
        read_exact_at(file, &mut packed, stored.offset)?;
        let bytes = if stored.deflated {
            let mut out = Vec::with_capacity(stored.size as usize);
            flate2::read::DeflateDecoder::new(packed.as_slice())
                .take(stored.size + 1)
                .read_to_end(&mut out)?;
            out
        } else {
            packed
        };
        let mut crc = flate2::Crc::new();
        crc.update(&bytes);
        if bytes.len() as u64 != stored.size || crc.sum() != stored.crc32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "changed since the root was built",
            ));
        }
        Ok(bytes)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &fs::File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &fs::File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// No positional reads here: such a platform builds its roots on disk.
#[cfg(not(any(unix, windows)))]
fn read_exact_at(_: &fs::File, _: &mut [u8], _: u64) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

fn stored_file(path: &Path) -> Option<(Arc<View>, String, Stored)> {
    let (view, rel) = find(path)?;
    let stored = *view.files.get(&rel)?;
    Some((view, rel, stored))
}

/// The mounted index `path` is under, and `path` relative to its root.
fn find(path: &Path) -> Option<(Arc<View>, String)> {
    let views = VIEWS.read().unwrap_or_else(|e| e.into_inner());
    if views.is_empty() {
        return None;
    }
    let absolute;
    let path = if path.is_relative() {
        absolute = std::path::absolute(path).ok()?;
        absolute.as_path()
    } else {
        path
    };
    let view = views.iter().find(|v| path.starts_with(&v.root))?;
    let rel = relative_name(path.strip_prefix(&view.root).ok()?)?;
    Some((view.clone(), rel))
}

/// `rel` as an index names it: plain parts joined by `/`.
fn relative_name(rel: &Path) -> Option<String> {
    let mut name = String::new();
    for part in rel.components() {
        let Component::Normal(part) = part else {
            return None;
        };
        if !name.is_empty() {
            name.push('/');
        }
        name.push_str(part.to_str()?);
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "horizon-content-view-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            unmount(&self.0.join("root"));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = flate2::Crc::new();
        crc.update(bytes);
        crc.sum()
    }

    /// An archive of `files` (each stored or deflated, after some header
    /// bytes), and where each one is in it.
    fn archive(path: &Path, files: &[(&[u8], bool)]) -> Vec<Stored> {
        let mut out = b"header".to_vec();
        let mut stored = Vec::new();
        for (body, deflated) in files {
            let packed = if *deflated {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(body).unwrap();
                encoder.finish().unwrap()
            } else {
                body.to_vec()
            };
            stored.push(Stored {
                archive: 0,
                offset: out.len() as u64,
                compressed: packed.len() as u64,
                size: body.len() as u64,
                crc32: crc(body),
                deflated: *deflated,
            });
            out.extend(packed);
            out.extend(b"between");
        }
        fs::write(path, out).unwrap();
        stored
    }

    const MODEL: &[u8] = br#"{"parent": "minecraft:block/cube_all"}"#;
    const ITEM: &[u8] = br#"{"model": {"type": "minecraft:model"}}"#;
    const LONELY: &[u8] = b"only in the archive";

    #[test]
    fn archive_streams_seek_without_reading_neighboring_entries() {
        let scratch = Scratch::new("stream");
        let root = scratch.0.join("root");
        let hmod = scratch.0.join("sound.hmod");
        let entries = archive(&hmod, &[(b"hello", false), (b"world", true)]);
        let mut index = Index::default();
        assert_eq!(index.add_archive(&hmod).unwrap(), 0);
        index.insert("hello.ogg", entries[0]);
        index.insert("world.ogg", entries[1]);
        index.insert(
            "invalid.ogg",
            Stored {
                offset: u64::MAX,
                ..entries[0]
            },
        );
        index.mount(&root, &[hmod]).unwrap();

        let mut first = open_archive_stream(&root.join("hello.ogg"))
            .unwrap()
            .unwrap();
        let mut second = open_archive_stream(&root.join("hello.ogg"))
            .unwrap()
            .unwrap();
        assert_eq!(first.byte_len(), 5);
        assert_eq!(first.seek(SeekFrom::End(-2)).unwrap(), 3);
        let mut tail = Vec::new();
        first.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"lo");
        assert!(first.seek(SeekFrom::End(-6)).is_err());
        assert_eq!(first.read(&mut [0; 8]).unwrap(), 0);
        let mut body = Vec::new();
        second.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"hello");

        let mut deflated = open_archive_stream(&root.join("world.ogg"))
            .unwrap()
            .unwrap();
        assert_eq!(deflated.seek(SeekFrom::Start(1)).unwrap(), 1);
        let mut body = Vec::new();
        deflated.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"orld");
        assert!(
            open_archive_stream(&root.join("invalid.ogg"))
                .unwrap()
                .is_err()
        );
        assert!(open_archive_stream(&root.join("absent.ogg")).is_none());
    }

    /// A root with files on disk and an index over `mods/ruby.hmod`.
    fn ruby(scratch: &Scratch) -> (PathBuf, Index) {
        let root = scratch.0.join("root");
        scratch.write("root/assets/ruby/lang/en_us.json", b"{}");
        scratch.write("root/assets/minecraft/models/block/stone.json", b"{}");
        fs::create_dir_all(scratch.0.join("mods")).unwrap();
        let hmod = scratch.0.join("mods/ruby.hmod");
        let at = archive(&hmod, &[(MODEL, true), (ITEM, false), (LONELY, true)]);
        let mut index = Index::default();
        assert_eq!(index.add_archive(&hmod).unwrap(), 0);
        index.insert("assets/ruby/models/block/ruby.json", at[0]);
        index.insert("assets/ruby/items/ruby.json", at[1]);
        index.insert("assets/lonely/models/item/a.json", at[2]);
        (root, index)
    }

    fn names(entries: &[DirEntry]) -> Vec<(String, bool)> {
        let mut names: Vec<_> = entries
            .iter()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.is_dir()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn archive_texture_cache_identity_survives_root_move() {
        use crate::resource_cache::Cache;

        let scratch = Scratch::new("texture-cache-move");
        scratch.write("root/horizon-mod-content.stamp", b"same inputs");
        let root = scratch.0.join("root");
        let hmod = scratch.0.join("textures.hmod");
        let rel = "assets/ruby/textures/block/ruby.png";
        let body = b"first";
        let saved = archive(&hmod, &[(body, true)]);
        let mut index = Index::default();
        index.add_archive(&hmod).unwrap();
        index.insert(rel, saved[0]);
        index.save(&root).unwrap();
        index.mount(&root, std::slice::from_ref(&hmod)).unwrap();

        let source = root.join(rel);
        assert!(!source.exists());
        assert_eq!(identity(&source), Some((body.len() as u64, crc(body))));
        let direct =
            |root: &Path| Cache::new(root, "test-source", &[&root.join(rel)], &[]).unwrap();
        let tree = |root: &Path| Cache::for_tree(root, "test-tree", &[]).unwrap();
        direct(&root).write(b"source cache").unwrap();
        tree(&root).write(b"tree cache").unwrap();

        let moved = scratch.0.join("moved");
        unmount(&root);
        fs::rename(&root, &moved).unwrap();
        let relocated = scratch.0.join("relocated");
        fs::create_dir(&relocated).unwrap();
        let moved_hmod = relocated.join("textures.hmod");
        fs::rename(&hmod, &moved_hmod).unwrap();
        Index::load(&moved)
            .unwrap()
            .mount(&moved, std::slice::from_ref(&moved_hmod))
            .unwrap();

        assert_eq!(
            identity(&moved.join(rel)),
            Some((body.len() as u64, crc(body)))
        );
        assert_eq!(
            direct(&moved).read().as_deref(),
            Some(b"source cache".as_slice())
        );
        assert_eq!(
            tree(&moved).read().as_deref(),
            Some(b"tree cache".as_slice())
        );

        unmount(&moved);
        let changed = archive(&moved_hmod, &[(b"other", false)]);
        let mut index = Index::default();
        index.add_archive(&moved_hmod).unwrap();
        index.insert(rel, changed[0]);
        index.mount(&moved, &[moved_hmod]).unwrap();
        assert!(direct(&moved).read().is_none());
        assert!(tree(&moved).read().is_none());
        unmount(&moved);
    }

    #[test]
    fn served_files_read_beside_the_disk_and_listings_merge() {
        let scratch = Scratch::new("merge");
        let (root, index) = ruby(&scratch);
        assert_eq!(index.len(), 3);
        index
            .mount(&root, &[scratch.0.join("mods/ruby.hmod")])
            .unwrap();
        let model = root.join("assets/ruby/models/block/ruby.json");
        assert!(!model.exists(), "a served file is not written");
        assert_eq!(read(&model).unwrap(), MODEL);
        assert_eq!(
            read(&root.join("assets/ruby/items/ruby.json")).unwrap(),
            ITEM
        );
        assert_eq!(
            read_to_string(&root.join("assets/lonely/models/item/a.json")).unwrap(),
            "only in the archive"
        );
        assert_eq!(
            read(&root.join("assets/ruby/lang/en_us.json")).unwrap(),
            b"{}"
        );
        assert_eq!(
            read(&root.join("assets/ruby/models/block/gone.json"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        // A namespace only the archive has is a folder too.
        assert_eq!(
            names(&read_dir(&root.join("assets")).unwrap()),
            [
                ("lonely".to_owned(), true),
                ("minecraft".to_owned(), true),
                ("ruby".to_owned(), true)
            ]
        );
        assert_eq!(
            names(&read_dir(&root.join("assets/ruby")).unwrap()),
            [
                ("items".to_owned(), true),
                ("lang".to_owned(), true),
                ("models".to_owned(), true)
            ]
        );
        assert_eq!(
            names(&read_dir(&root.join("assets/ruby/models/block")).unwrap()),
            [("ruby.json".to_owned(), false)]
        );
        // A listed served file carries the identity a lookup gives; a disk one none.
        let block = read_dir(&root.join("assets/ruby/models/block")).unwrap();
        assert_eq!(block[0].served(), identity(&model));
        assert!(block[0].served().is_some());
        let lang = read_dir(&root.join("assets/ruby/lang")).unwrap();
        assert!(lang.iter().all(|e| e.served().is_none()));
        // A disk file brings its length from the listing; a served one has none.
        assert!(
            lang.iter()
                .all(|e| e.disk().is_some_and(|(len, _)| len == 2))
        );
        assert!(block[0].disk().is_none());
        let lonely = read_dir(&root.join("assets/lonely/models")).unwrap();
        assert_eq!(
            lonely[0].path(),
            root.join("assets/lonely/models").join("item")
        );
        assert!(is_dir(&root.join("assets/lonely")));
        assert!(is_dir(&root.join("assets/ruby/lang")));
        assert!(!is_dir(&model));
        assert!(!is_dir(&root.join("assets/ruby/models/block/ruby")));
        assert_eq!(
            read_dir(&root.join("assets/nobody")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // Unmounted, the root is only its disk again.
        unmount(&root);
        assert!(read(&model).is_err());
        assert!(!is_dir(&root.join("assets/lonely")));
    }

    #[test]
    fn a_later_file_wins_and_a_removed_one_reads_from_disk() {
        let scratch = Scratch::new("order");
        let (root, mut index) = ruby(&scratch);
        let item = index.files["assets/ruby/items/ruby.json"];
        index.insert("assets/ruby/models/block/ruby.json", item);
        index.remove("assets/ruby/lang/en_us.json");
        index.remove("assets/ruby/items/ruby.json");
        scratch.write("root/assets/ruby/items/ruby.json", b"written");
        index
            .mount(&root, &[scratch.0.join("mods/ruby.hmod")])
            .unwrap();
        assert_eq!(
            read(&root.join("assets/ruby/models/block/ruby.json")).unwrap(),
            ITEM
        );
        assert_eq!(
            read(&root.join("assets/ruby/items/ruby.json")).unwrap(),
            b"written"
        );
        assert_eq!(
            read(&root.join("assets/ruby/lang/en_us.json")).unwrap(),
            b"{}"
        );
    }

    #[test]
    fn a_saved_index_finds_moved_archives_by_name_and_refuses_missing_ones() {
        let scratch = Scratch::new("saved");
        let (root, index) = ruby(&scratch);
        index.save(&root).unwrap();
        let loaded = Index::load(&root).unwrap();
        assert_eq!(loaded, index);
        // The install moved: the same archive in another folder serves the root.
        fs::create_dir_all(scratch.0.join("elsewhere")).unwrap();
        let moved = scratch.0.join("elsewhere/ruby.hmod");
        fs::rename(scratch.0.join("mods/ruby.hmod"), &moved).unwrap();
        let other = scratch.write("elsewhere/other.hmod", b"x");
        Index::load(&root)
            .unwrap()
            .mount(&root, &[other.clone(), moved.clone()])
            .unwrap();
        assert_eq!(
            read(&root.join("assets/ruby/models/block/ruby.json")).unwrap(),
            MODEL
        );
        // Missing, or another length: nothing is served.
        unmount(&root);
        assert!(Index::load(&root).unwrap().mount(&root, &[other]).is_err());
        fs::write(&moved, b"another build").unwrap();
        assert!(Index::load(&root).unwrap().mount(&root, &[moved]).is_err());
        assert!(read(&root.join("assets/ruby/models/block/ruby.json")).is_err());
        // Another format, or damage, is not an index.
        fs::write(root.join(FILES), b"HZFILES0").unwrap();
        assert_eq!(
            Index::load(&root).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_changed_archive_fails_the_read() {
        let scratch = Scratch::new("changed");
        let (root, index) = ruby(&scratch);
        let hmod = scratch.0.join("mods/ruby.hmod");
        index.mount(&root, std::slice::from_ref(&hmod)).unwrap();
        let mut bytes = fs::read(&hmod).unwrap();
        let item = bytes.windows(ITEM.len()).position(|w| w == ITEM).unwrap();
        bytes[item] ^= 1;
        fs::write(&hmod, bytes).unwrap();
        let err = read(&root.join("assets/ruby/items/ruby.json")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("assets/ruby/items/ruby.json"),
            "{err}"
        );
    }

    #[test]
    fn relative_names_take_plain_parts_only() {
        assert_eq!(
            relative_name(Path::new("assets/a/models")).as_deref(),
            Some("assets/a/models")
        );
        assert_eq!(relative_name(Path::new("")).as_deref(), Some(""));
        assert_eq!(relative_name(Path::new("assets/../x")), None);
    }
}
