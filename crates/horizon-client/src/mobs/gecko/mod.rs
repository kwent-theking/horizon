//! GeckoLib mobs from the ported mods content (`assets/<ns>/geo/**/*.geo.json`,
//! `animations/**/*.json`, `textures/entity{,ies}/**/*.png`): read into
//! vanilla-style model parts, drawn by the mob renderer with their texture
//! from the shared atlas, and posed by their own clips (`Anim::Gecko`).
//! `load` only indexes the files; an entity id is matched to its model,
//! texture and animation files the first time it is asked for (see `resolve`).
pub mod anim;
pub mod geo;
pub mod molang;
pub mod resolve;

use super::{Mob, MobType, Part, Polygon, anim::Anim};
use resolve::{AnimationEntry, GeoEntry, NamespaceIndex, TEXTURE_DIRS, TextureEntry};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

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

fn scan(root: &Path) -> HashMap<String, NamespaceIndex> {
    let mut out = HashMap::new();
    let Ok(namespaces) = fs::read_dir(root.join("assets")) else {
        return out;
    };
    for ns in namespaces.flatten() {
        let Some(namespace) = ns.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let dir = ns.path();
        if namespace == "minecraft" || !dir.join("geo").is_dir() {
            continue;
        }
        out.insert(namespace, index_namespace(&dir));
    }
    out
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

struct Queries {
    life_time: f32,
    speed: f32,
}
impl molang::MolangContext for Queries {
    fn query(&self, name: &str) -> f32 {
        match name {
            "query.life_time" => self.life_time,
            "query.ground_speed" | "query.modified_move_speed" => self.speed,
            "query.is_on_ground" => 1.0,
            _ => 0.0,
        }
    }
}

/// Poses a GeckoLib mob for this frame: the idle or walking clip, in the
/// renderer's axes (geo.rs: Bedrock's y up is our y down, so x and z rotations
/// flip), then the head's look.
pub fn animate(m: &Mob, s: &super::anim::State, p: &mut super::anim::Poses) {
    let Some(mob) = mob(&m.kind.id) else {
        return;
    };
    if let Some(clips) = &mob.clips {
        let moving = s.speed > 0.05;
        if let Some(clip) = anim::pick_clip(clips, moving).and_then(|name| clips.clips.get(name)) {
            let seconds = s.age / 20.0;
            let queries = Queries {
                life_time: seconds,
                speed: s.speed,
            };
            for (bone, sample) in clip.sample(seconds, &queries) {
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
    }
    if let Some(head) = &mob.head {
        let pose = p.get(head);
        pose[3] += s.x_rot.to_radians();
        pose[4] += s.y_rot.to_radians();
    }
}
