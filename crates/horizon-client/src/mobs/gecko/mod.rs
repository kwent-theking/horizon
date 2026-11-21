//! GeckoLib mobs from the ported mods content (`assets/<ns>/geo/**/*.geo.json`,
//! `animations/**/*.json`, `textures/entity{,ies}/**/*.png`): read into
//! vanilla-style model parts, drawn by the mob renderer with their texture
//! from the shared atlas, and posed by their own clips (`Anim::Gecko`).
//! `load` only indexes the files; an entity id is matched to its model,
//! texture and animation files the first time it is asked for (see `resolve`).
pub mod anim;
pub mod controller;
pub mod geo;
pub mod molang;
pub mod resolve;

use super::{Mob, MobType, Part, Polygon, anim::Anim};
use resolve::{AnimationEntry, GeoEntry, NamespaceIndex, TEXTURE_DIRS, TextureEntry};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedGeoEntry {
    stem: String,
    folders: Vec<String>,
    rel_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedTextureEntry {
    stem: String,
    folders: Vec<String>,
    rel_path: String,
    dir: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedAnimationEntry {
    name: String,
    rel_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedNamespaceIndex {
    geo: Vec<CachedGeoEntry>,
    textures: Vec<CachedTextureEntry>,
    animations: Vec<CachedAnimationEntry>,
}

fn to_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

impl CachedNamespaceIndex {
    fn from_index(index: &NamespaceIndex, root: &Path) -> Self {
        Self {
            geo: index
                .geo
                .iter()
                .map(|g| CachedGeoEntry {
                    stem: g.stem.clone(),
                    folders: g.folders.clone(),
                    rel_path: to_relative(&g.path, root),
                })
                .collect(),
            textures: index
                .textures
                .iter()
                .map(|t| CachedTextureEntry {
                    stem: t.stem.clone(),
                    folders: t.folders.clone(),
                    rel_path: to_relative(&t.path, root),
                    dir: t.dir,
                })
                .collect(),
            animations: index
                .animations
                .iter()
                .map(|a| CachedAnimationEntry {
                    name: a.name.clone(),
                    rel_path: to_relative(&a.path, root),
                })
                .collect(),
        }
    }

    fn into_index(self, root: &Path) -> NamespaceIndex {
        NamespaceIndex {
            geo: self
                .geo
                .into_iter()
                .map(|g| GeoEntry {
                    stem: g.stem,
                    folders: g.folders,
                    path: root.join(g.rel_path),
                })
                .collect(),
            textures: self
                .textures
                .into_iter()
                .map(|t| TextureEntry {
                    stem: t.stem,
                    folders: t.folders,
                    path: root.join(t.rel_path),
                    dir: t.dir,
                })
                .collect(),
            animations: self
                .animations
                .into_iter()
                .map(|a| AnimationEntry {
                    name: a.name,
                    path: root.join(a.rel_path),
                })
                .collect(),
        }
    }
}

pub struct GeckoMob {
    pub model: Arc<Part>,
    /// The atlas sprite (`ns:entity/...`, see `HudAtlas::pack`).
    pub texture: String,
    pub clips: Option<anim::AnimationFile>,
    /// The head bone, turned by the mob's look like vanilla heads.
    pub head: Option<String>,
}

struct Library {
    namespaces: HashMap<String, NamespaceIndex>,
    /// Entity id -> its mob, `None` when nothing matched.
    mobs: Mutex<HashMap<String, Option<Arc<GeckoMob>>>>,
}

static LIBRARY: OnceLock<Library> = OnceLock::new();

/// The GeckoLib mob of entity id `id` (`ns:path`), built on first use;
/// `None` before `load` or when no model matches.
pub fn mob(id: &str) -> Option<Arc<GeckoMob>> {
    let library = LIBRARY.get()?;
    let mut mobs = library.mobs.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(known) = mobs.get(id) {
        return known.clone();
    }
    let built = build(library, id);
    log::debug!(
        "Horizon: GeckoLib {id}: {}",
        built.as_ref().map_or("no model", |m| m.texture.as_str())
    );
    mobs.insert(id.to_owned(), built.clone());
    built
}

/// Logs, per namespace, how many of the entity ids asked for so far got a
/// model and how many did not.
pub fn log_coverage() {
    let Some(library) = LIBRARY.get() else {
        return;
    };
    let mobs = library.mobs.lock().unwrap_or_else(PoisonError::into_inner);
    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (id, mob) in mobs.iter() {
        let namespace = id.split_once(':').map_or("", |(ns, _)| ns);
        let entry = counts.entry(namespace).or_default();
        if mob.is_some() {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }
    for (namespace, (resolved, unresolved)) in counts {
        log::info!(
            "Horizon: GeckoLib {namespace}: {resolved} entity types modelled, {unresolved} not"
        );
    }
}

/// Indexes every mod namespace's geo models, textures and animations under
/// `root` (a content root); returns how many geo models it found. Runs once;
/// later calls change nothing.
pub fn load(root: &Path) -> usize {
    let library = LIBRARY.get_or_init(|| Library {
        namespaces: scan(root),
        mobs: Mutex::new(HashMap::new()),
    });
    for (namespace, index) in &library.namespaces {
        log::info!(
            "Horizon: GeckoLib {namespace}: {} models, {} textures, {} animation files",
            index.geo.len(),
            index.textures.len(),
            index.animations.len()
        );
    }
    library.namespaces.values().map(|n| n.geo.len()).sum()
}

pub fn scan(root: &Path) -> HashMap<String, NamespaceIndex> {
    let cache = horizon_world::resource_cache::Cache::for_tree(root, "gecko-index-v1", &[]);
    if let Some(snapshot) = cache
        .as_ref()
        .and_then(|c| c.read_value::<Vec<(String, CachedNamespaceIndex)>>())
    {
        return snapshot
            .into_iter()
            .map(|(ns, idx)| (ns, idx.into_index(root)))
            .collect();
    }

    let Ok(entries) = fs::read_dir(root.join("assets")) else {
        return HashMap::new();
    };

    let mut candidates = Vec::new();
    for ns in entries.flatten() {
        let Some(namespace) = ns.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let dir = ns.path();
        if namespace == "minecraft" || !dir.join("geo").is_dir() {
            continue;
        }
        candidates.push((namespace, dir));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));

    let indexed: Vec<(String, NamespaceIndex)> = horizon_core::par::par_map_with(
        horizon_core::par::threads(),
        &candidates,
        |(namespace, dir)| (namespace.clone(), index_namespace(dir)),
    );

    let snapshot: Vec<(String, CachedNamespaceIndex)> = indexed
        .iter()
        .map(|(ns, idx)| (ns.clone(), CachedNamespaceIndex::from_index(idx, root)))
        .collect();
    if let Some(cache) = cache {
        let _ = cache.write_value(&snapshot);
    }

    indexed.into_iter().collect()
}

fn index_namespace(dir: &Path) -> NamespaceIndex {
    let geo_dir = dir.join("geo");
    let geo = files(&geo_dir, ".geo.json")
        .into_iter()
        .filter_map(|path| {
            let (stem, folders) = split_name(&path, &geo_dir, ".geo.json")?;
            Some(GeoEntry {
                stem,
                folders,
                path,
            })
        })
        .collect();
    let mut textures = Vec::new();
    for (i, folder) in TEXTURE_DIRS.iter().enumerate() {
        let texture_dir = dir.join("textures").join(folder);
        textures.extend(files(&texture_dir, ".png").into_iter().filter_map(|path| {
            let (stem, folders) = split_name(&path, &texture_dir, ".png")?;
            Some(TextureEntry {
                stem,
                folders,
                path,
                dir: i,
            })
        }));
    }
    let animations = files(&dir.join("animations"), ".json")
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some(AnimationEntry { name, path })
        })
        .collect();
    NamespaceIndex {
        geo,
        textures,
        animations,
    }
}

/// A file's name without `suffix`, and the folders between `base` and it.
fn split_name(path: &Path, base: &Path, suffix: &str) -> Option<(String, Vec<String>)> {
    let stem = path.file_name()?.to_str()?.strip_suffix(suffix)?.to_owned();
    let folders = path
        .parent()?
        .strip_prefix(base)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some((stem, folders))
}

fn build(library: &Library, id: &str) -> Option<Arc<GeckoMob>> {
    let (namespace, path) = id.split_once(':')?;
    let index = library.namespaces.get(namespace)?;
    let resolved = resolve::resolve(index, path)?;
    let geo_file = &resolved.geo.path;
    let model = match fs::read_to_string(geo_file)
        .map_err(|e| e.to_string())
        .and_then(|text| geo::parse_geo(&text))
    {
        Ok(model) => model,
        Err(e) => {
            log::warn!("Horizon: GeckoLib model {}: {e}", geo_file.display());
            return None;
        }
    };
    let mut clips: Option<anim::AnimationFile> = None;
    for entry in &resolved.animations {
        let Some(file) = fs::read_to_string(&entry.path)
            .ok()
            .and_then(|text| anim::parse_animations(&text).ok())
        else {
            continue;
        };
        let merged = clips.get_or_insert_with(|| anim::AnimationFile {
            clips: BTreeMap::new(),
        });
        for (name, clip) in file.clips {
            merged.clips.entry(name).or_insert(clip);
        }
    }
    let root_part = part(geo::GeoPart {
        name: "root".into(),
        pose: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        polygons: Vec::new(),
        children: model.roots,
    });
    let head = find_bone(&root_part, "head");
    let texture_root = resolved
        .texture
        .path
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == "textures"))?;
    let sprite = format!(
        "{namespace}:{}",
        resolved
            .texture
            .path
            .strip_prefix(texture_root)
            .ok()?
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/")
    );
    Some(Arc::new(GeckoMob {
        model: Arc::new(root_part),
        texture: sprite,
        clips,
        head,
    }))
}

/// Files under `dir` (recursively) whose name ends with `suffix`.
fn files(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn part(geo: geo::GeoPart) -> Part {
    Part {
        name: geo.name,
        pose: geo.pose,
        polygons: geo
            .polygons
            .into_iter()
            .map(|p| Polygon {
                normal: p.normal,
                vertices: p.vertices,
            })
            .collect(),
        children: geo.children.into_iter().map(part).collect(),
    }
}

fn find_bone(part: &Part, name: &str) -> Option<String> {
    if part.name.eq_ignore_ascii_case(name) {
        return Some(part.name.clone());
    }
    part.children.iter().find_map(|c| find_bone(c, name))
}

/// A mob type for GeckoLib mob `id`, modelled on `template` (a vanilla type:
/// its size and physics stand in until the server's entity data says more).
pub fn mob_type(template: &MobType, id: &str) -> Option<MobType> {
    let mob = mob(id)?;
    let mut kind = template.clone();
    kind.id = id.into();
    kind.model = mob.model.clone();
    kind.layers = HashMap::new();
    kind.texture = mob.texture.clone();
    kind.anim = Anim::Gecko;
    kind.sounds = [""; 3];
    kind.step_sound = None;
    Some(kind)
}

/// Poses a GeckoLib mob for this frame: the clip its controller picks (attack,
/// death, fly, run, walk or idle, blended on changes), in the renderer's axes
/// (geo.rs: Bedrock's y up is our y down, so x and z rotations flip), then the
/// head's look.
pub fn animate(m: &Mob, s: &super::anim::State, p: &mut super::anim::Poses) {
    animate_with_lod(m, s, p, false);
}

/// Poses a GeckoLib mob, optionally skipping clip Molang evaluation at distance.
pub fn animate_with_lod(
    m: &Mob,
    s: &super::anim::State,
    p: &mut super::anim::Poses,
    skip_molang: bool,
) {
    let Some(mob) = mob(&m.kind.id) else {
        return;
    };
    if !skip_molang && let Some(clips) = &mob.clips {
        let snapshot = controller::Snapshot::from_mob(m, s);
        for (bone, sample) in controller::pose(m.id, clips, &snapshot) {
            let pose = p.get(&bone);
            pose[0] += sample.position[0];
            pose[1] -= sample.position[1];
            pose[2] += sample.position[2];
            pose[3] -= sample.rotation_deg[0].to_radians();
            pose[4] += sample.rotation_deg[1].to_radians();
            pose[5] -= sample.rotation_deg[2].to_radians();
            pose[6] *= sample.scale[0];
            pose[7] *= sample.scale[1];
            pose[8] *= sample.scale[2];
        }
    }
    if let Some(head) = &mob.head {
        let pose = p.get(head);
        pose[3] += s.x_rot.to_radians();
        pose[4] += s.y_rot.to_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gecko_index_cache_roundtrip_portable_across_roots() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root1 = std::env::temp_dir().join(format!("horizon-gecko-cache-test-1-{nonce}"));
        let root2 = std::env::temp_dir().join(format!("horizon-gecko-cache-test-2-{nonce}"));
        let _ = fs::remove_dir_all(&root1);
        let _ = fs::remove_dir_all(&root2);

        let geo_dir1 = root1.join("assets/testmod/geo");
        let tex_dir1 = root1.join("assets/testmod/textures/entity");
        let anim_dir1 = root1.join("assets/testmod/animations");
        let derived_dir1 = root1.join(".horizon-derived");
        fs::create_dir_all(&geo_dir1).unwrap();
        fs::create_dir_all(&tex_dir1).unwrap();
        fs::create_dir_all(&anim_dir1).unwrap();
        fs::create_dir_all(&derived_dir1).unwrap();

        fs::write(root1.join("horizon-mod-content.stamp"), "stamp-v1").unwrap();
        fs::write(root1.join("horizon-mod-content.json"), "{}").unwrap();
        fs::write(root1.join(".horizon-derived/installed-mods-v1"), "testmod").unwrap();

        fs::write(geo_dir1.join("test.geo.json"), b"{}").unwrap();
        fs::write(tex_dir1.join("test.png"), b"").unwrap();
        fs::write(anim_dir1.join("test.animation.json"), b"{}").unwrap();

        let first = scan(&root1);
        assert_eq!(first.len(), 1);
        let first_index = first.get("testmod").expect("testmod index");
        assert_eq!(first_index.geo.len(), 1);
        assert_eq!(first_index.geo[0].stem, "test");
        assert_eq!(first_index.geo[0].path, geo_dir1.join("test.geo.json"));
        assert_eq!(first_index.textures.len(), 1);
        assert_eq!(first_index.textures[0].path, tex_dir1.join("test.png"));
        assert_eq!(first_index.animations.len(), 1);
        assert_eq!(
            first_index.animations[0].path,
            anim_dir1.join("test.animation.json")
        );

        let cache_entries: Vec<_> = fs::read_dir(&derived_dir1)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("gecko-index-v1"))
            })
            .collect();
        assert_eq!(cache_entries.len(), 1, "expected gecko-index-v1 cache file");
        let cache_file = cache_entries[0].path();

        // While the content stamp and installed-mods sentinel match, scan() reads
        // the cached index rather than traversing geo/ again.
        fs::write(geo_dir1.join("extra.geo.json"), b"{}").unwrap();
        let cached = scan(&root1);
        assert_eq!(cached.len(), 1);
        let cached_index = cached.get("testmod").expect("cached testmod index");
        assert_eq!(cached_index.geo.len(), 1);
        assert_eq!(cached_index.geo[0].stem, "test");

        // Relative paths inside the cached payload re-root cleanly to any target root.
        let portable_index =
            CachedNamespaceIndex::from_index(first_index, &root1).into_index(&root2);
        assert_eq!(portable_index.geo.len(), 1);
        assert_eq!(
            portable_index.geo[0].path,
            root2.join("assets/testmod/geo/test.geo.json")
        );
        assert!(portable_index.geo[0].path.starts_with(&root2));
        assert!(!portable_index.geo[0].path.starts_with(&root1));

        assert_eq!(portable_index.textures.len(), 1);
        assert_eq!(
            portable_index.textures[0].path,
            root2.join("assets/testmod/textures/entity/test.png")
        );
        assert!(portable_index.textures[0].path.starts_with(&root2));
        assert!(!portable_index.textures[0].path.starts_with(&root1));

        assert_eq!(portable_index.animations.len(), 1);
        assert_eq!(
            portable_index.animations[0].path,
            root2.join("assets/testmod/animations/test.animation.json")
        );
        assert!(portable_index.animations[0].path.starts_with(&root2));
        assert!(!portable_index.animations[0].path.starts_with(&root1));

        // Corrupt cache bytes fall back cleanly to rescanning root1.
        fs::write(&cache_file, b"corrupted-cache-data").unwrap();
        let recovered = scan(&root1);
        assert_eq!(recovered.len(), 1);
        let rec_index = recovered.get("testmod").expect("recovered index");
        assert_eq!(rec_index.geo.len(), 2);
        assert!(rec_index.geo.iter().all(|g| g.path.starts_with(&root1)));

        let _ = fs::remove_dir_all(&root1);
        let _ = fs::remove_dir_all(&root2);
    }
}
