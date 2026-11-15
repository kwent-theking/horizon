//! Java-modelled mobs of a modded NeoForge 1.21.1 server, from the model dump of a client with
//! the same mods (mods/horizon/src/neoforge-client-dump, tools/neoforge_client_dump.py).
//! `modded-entity-models.json`, beside the server's tables (`net::server_states_path`), names each
//! entity type's model layer, texture, size and keyframe clips, and holds the layers' part trees in
//! the schema of the vanilla `entity-models.json`. The textures are copied to
//! `modded-entity-textures/<ns>/textures/` beside it and packed into the atlas
//! (`hud::set_mod_entity_textures`). Drawn like a GeckoLib mob (`Anim::Modded`) on a vanilla type's
//! physics: its walk and idle clips when it has them, else a generic swing of its leg and arm
//! parts, and the head turned to its look. Models are built from the file the first time an id
//! is asked for.
use super::{
    Mob, MobType, Part,
    anim::{Anim, Poses, State},
    keyframes::Animation,
};
use horizon_core::geometry::trig::Mth;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, value::RawValue};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    f32::consts::PI,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

/// The dump's file name, beside the server's tables.
pub const FILE: &str = "modded-entity-models.json";
/// The folder of the textures the dump copied, beside it.
pub const TEXTURES: &str = "modded-entity-textures";
/// Limb speed above which a mob walks (its walk clip plays, its idle clip stops).
const MOVING_SPEED: f32 = 0.01;
/// `animateWalk`'s speed and scale factors most models pass (CamelModel, BreezeModel...).
const WALK_SPEED_FACTOR: f32 = 2.0;
const WALK_SCALE_FACTOR: f32 = 2.5;
/// QuadrupedModel / HumanoidModel's limb swing: `cos(pos * 0.6662 + phase) * amplitude * speed`.
const SWING_RATE: f32 = 0.6662;
const LEG_SWING: f32 = 1.4;
const ARM_SWING: f32 = 1.0;

/// `modded-entity-models.json` beside the server's tables.
pub fn path() -> PathBuf {
    crate::net::server_states_path().with_file_name(FILE)
}

/// The dump's texture folder (`<ns>/textures/...png`).
pub fn textures_dir() -> PathBuf {
    crate::net::server_states_path().with_file_name(TEXTURES)
}

pub struct ModdedMob {
    pub model: Arc<Part>,
    /// The atlas sprite (`ns:entity/...`, see `HudAtlas::pack`).
    pub texture: String,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub eye_height: Option<f64>,
    walk: Option<Animation>,
    idle: Option<Animation>,
    /// The head part, turned by the mob's look.
    head: Option<String>,
    /// The leg and arm parts swung as it walks, when it has no walk clip.
    limbs: Vec<Limb>,
}

#[derive(Clone, Debug, PartialEq)]
struct Limb {
    part: String,
    phase: f32,
    amplitude: f32,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct EntityEntry {
    living: bool,
    layer: Option<String>,
    texture: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
    eye_height: Option<f64>,
    animations: EntityAnimations,
}

#[derive(Default)]
struct EntityAnimations(BTreeMap<String, Animation>);

impl<'de> Deserialize<'de> for EntityAnimations {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Clips;
        impl<'de> Visitor<'de> for Clips {
            type Value = EntityAnimations;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of entity animation clips")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut clips = BTreeMap::new();
                while let Some(name) = map.next_key::<String>()? {
                    if pose_clip_name(&name) {
                        let value = map.next_value::<Value>()?;
                        clips.insert(name, Animation::parse(&value));
                    } else {
                        map.next_value::<ValidateJson>()?;
                    }
                }
                Ok(EntityAnimations(clips))
            }
        }
        deserializer.deserialize_map(Clips)
    }
}

/// Validates discarded clips without retaining a JSON tree. Deserializing the
/// whole value here preserves serde_json's number/depth checks, including for
/// duplicate clip names that a map would otherwise overwrite.
struct ValidateJson;
impl<'de> Deserialize<'de> for ValidateJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Any;
        impl<'de> Visitor<'de> for Any {
            type Value = ValidateJson;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    map.next_value::<ValidateJson>()?;
                }
                Ok(ValidateJson)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<ValidateJson>()?.is_some() {}
                Ok(ValidateJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(ValidateJson)
            }
        }
        deserializer.deserialize_any(Any)
    }
}

#[derive(Deserialize)]
struct DumpFile {
    #[serde(default)]
    entities: HashMap<String, EntityEntry>,
    #[serde(default)]
    models: HashMap<String, Box<RawValue>>,
}

/// The parsed dump: entries and raw model trees, built into mobs on first use.
pub struct Library {
    entities: HashMap<String, EntityEntry>,
    model_layers: HashSet<String>,
    /// Raw model JSON is kept only until its layer is first materialized.
    models: Mutex<HashMap<String, Box<RawValue>>>,
    /// Only clips this renderer can play, moved out when an entity is first materialized.
    animations: Mutex<HashMap<String, EntityAnimations>>,
    textures: PathBuf,
    texture_exists: fn(&Path) -> bool,
    parts: Mutex<HashMap<String, Option<Arc<Part>>>>,
    mobs: Mutex<HashMap<String, Option<Arc<ModdedMob>>>>,
}

impl Library {
    /// The dump's text, with its textures under `textures` (checked with `texture_exists`).
    pub fn parse(
        json: &str,
        textures: PathBuf,
        texture_exists: fn(&Path) -> bool,
    ) -> Result<Self, String> {
        let file: DumpFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut entities = file.entities;
        let animations = entities
            .iter_mut()
            .filter_map(|(id, entry)| {
                let selected = std::mem::take(&mut entry.animations);
                (!selected.0.is_empty()).then(|| (id.clone(), selected))
            })
            .collect();
        let model_layers = file.models.keys().cloned().collect();
        Ok(Self {
            entities,
            model_layers,
            models: Mutex::new(file.models),
            animations: Mutex::new(animations),
            textures,
            texture_exists,
            parts: Mutex::new(HashMap::new()),
            mobs: Mutex::new(HashMap::new()),
        })
    }

    /// How many entity types the dump lists, and how many of them can be drawn: living, with a
    /// model and a texture the dump copied.
    pub fn counts(&self) -> (usize, usize) {
        let drawable = self
            .entities
            .values()
            .filter(|e| self.drawable(e).is_some())
            .count();
        (self.entities.len(), drawable)
    }

    /// The layer and sprite of an entry that can be drawn.
    fn drawable<'a>(&self, entry: &'a EntityEntry) -> Option<(&'a str, String)> {
        if !entry.living {
            return None;
        }
        let layer = entry
            .layer
            .as_deref()
            .filter(|l| self.model_layers.contains(*l))?;
        let texture = entry.texture.as_deref()?;
        let sprite = sprite(texture)?;
        let vanilla = !texture.contains(':') || texture.starts_with("minecraft:");
        if !vanilla && !(self.texture_exists)(&texture_file(&self.textures, texture)?) {
            return None;
        }
        Some((layer, sprite))
    }

    /// The mob of entity id `id` (`ns:path`), built on first use; `None` when the dump can't
    /// draw it.
    pub fn mob(&self, id: &str) -> Option<Arc<ModdedMob>> {
        let mut mobs = self.mobs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = mobs.get(id) {
            return known.clone();
        }
        let built = self.build(id).map(Arc::new);
        log::debug!(
            "Horizon: modded entity {id}: {}",
            built.as_ref().map_or("no model", |m| m.texture.as_str())
        );
        mobs.insert(id.to_owned(), built.clone());
        built
    }

    fn build(&self, id: &str) -> Option<ModdedMob> {
        let entry = self.entities.get(id)?;
        let (layer, texture) = self.drawable(entry)?;
        let model = self.part(layer)?;
        let clips = self
            .animations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .unwrap_or_default()
            .0;
        let clips: Vec<(&str, &Animation)> = clips
            .iter()
            .map(|(name, clip)| (name.as_str(), clip))
            .collect();
        Some(ModdedMob {
            walk: pick_clip(&clips, &["walk", "run", "move"]),
            idle: pick_clip(&clips, &["idle"]),
            head: find_head(&model),
            limbs: limbs(&model),
            model,
            texture,
            width: entry.width.filter(|w| *w > 0.0),
            height: entry.height.filter(|h| *h > 0.0),
            eye_height: entry.eye_height.filter(|h| *h > 0.0),
        })
    }

    /// The part tree of `layer`, parsed once and shared by the entities drawn with it.
    fn part(&self, layer: &str) -> Option<Arc<Part>> {
        let mut parts = self.parts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = parts.get(layer) {
            return known.clone();
        }
        let raw = self
            .models
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(layer)?;
        let built = match serde_json::from_str::<Value>(raw.get()) {
            Ok(tree) => {
                let mut root = Part::parse("root", &tree);
                unique_names(&mut root, &mut HashSet::new());
                Some(Arc::new(root))
            }
            Err(e) => {
                log::warn!("Horizon: modded entity model {layer}: {e}");
                None
            }
        };
        parts.insert(layer.to_owned(), built.clone());
        built
    }

    /// A mob type for `id`, modelled on `template` (a vanilla type: its physics stand in) with
    /// the dump's size.
    pub fn mob_type(&self, template: &MobType, id: &str) -> Option<MobType> {
        let mob = self.mob(id)?;
        let mut kind = template.clone();
        kind.id = id.into();
        kind.model = mob.model.clone();
        kind.layers = HashMap::new();
        kind.texture = mob.texture.clone();
        kind.anim = Anim::Modded;
        kind.sounds = [""; 3];
        kind.step_sound = None;
        if let Some(width) = mob.width {
            kind.width = width;
        }
        if let Some(height) = mob.height {
            kind.height = height;
        }
        if let Some(eye_height) = mob.eye_height {
            kind.eye_height = eye_height;
        }
        Some(kind)
    }
}

fn pose_clip_name(name: &str) -> bool {
    ["walk", "run", "move", "idle"]
        .iter()
        .any(|word| name.contains(word))
}

static LIBRARY: OnceLock<Option<Library>> = OnceLock::new();

/// Reads the dump once (later calls change nothing); returns how many entity types it can draw.
pub fn load() -> usize {
    let library = LIBRARY.get_or_init(|| {
        let file = path();
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) => {
                log::info!(
                    "Horizon: no modded entity models at {}: {e}",
                    file.display()
                );
                return None;
            }
        };
        match Library::parse(&text, textures_dir(), Path::is_file) {
            Ok(library) => {
                let (listed, drawable) = library.counts();
                log::info!(
                    "Horizon: modded entity models from {}: {listed} entity types, {drawable} \
                     drawn with their own model",
                    file.display()
                );
                Some(library)
            }
            Err(e) => {
                log::warn!("Horizon: modded entity models {}: {e}", file.display());
                None
            }
        }
    });
    library.as_ref().map_or(0, |l| l.counts().1)
}

fn library() -> Option<&'static Library> {
    LIBRARY.get()?.as_ref()
}

/// The dumped mob of entity id `id`; `None` before `load` or when the dump can't draw it.
pub fn mob(id: &str) -> Option<Arc<ModdedMob>> {
    library()?.mob(id)
}

/// A mob type for dumped mob `id`, on `template`'s physics (see `Library::mob_type`).
pub fn mob_type(template: &MobType, id: &str) -> Option<MobType> {
    library()?.mob_type(template, id)
}

/// Poses a dumped mob for this frame.
pub fn animate(m: &Mob, s: &State, p: &mut Poses) {
    if let Some(mob) = mob(&m.kind.id) {
        pose(&mob, s, p);
    }
}

/// The walk clip while it walks (`animateWalk`), else its limbs' swing; the idle clip while it
/// stands; then the head's look, added to the model's own head pose.
fn pose(mob: &ModdedMob, s: &State, p: &mut Poses) {
    let moving = s.speed > MOVING_SPEED;
    match &mob.walk {
        Some(walk) if moving => {
            walk.apply_walk(s.pos, s.speed, WALK_SPEED_FACTOR, WALK_SCALE_FACTOR, p);
        }
        _ => {
            for limb in &mob.limbs {
                p.get(&limb.part)[3] +=
                    (s.pos * SWING_RATE + limb.phase).mcos() * limb.amplitude * s.speed;
            }
        }
    }
    if !moving && let Some(idle) = &mob.idle {
        idle.apply((s.age * 50.0) as i64, 1.0, p);
    }
    if let Some(head) = &mob.head {
        let pose = p.get(head);
        pose[3] += s.x_rot.to_radians();
        pose[4] += s.y_rot.to_radians();
    }
}

/// `ns:textures/<path>.png` as its atlas sprite: `ns:<path>`, bare `<path>` for vanilla's.
fn sprite(texture: &str) -> Option<String> {
    let (namespace, path) = texture.split_once(':').unwrap_or(("minecraft", texture));
    let rest = path.strip_prefix("textures/")?.strip_suffix(".png")?;
    Some(if namespace == "minecraft" {
        rest.to_owned()
    } else {
        format!("{namespace}:{rest}")
    })
}

/// Where the dump copied texture `ns:path`: `<dir>/<ns>/<path>`.
fn texture_file(dir: &Path, texture: &str) -> Option<PathBuf> {
    let (namespace, path) = texture.split_once(':')?;
    if path
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        return None;
    }
    Some(dir.join(namespace).join(path))
}

/// Renames every part whose name an earlier one (depth first) already has, `name#2`, `name#3`...:
/// poses are keyed by name, and exported Java models repeat names like `cube_r1` under many
/// parents. The first keeps its name, as `getAnyDescendantWithName` finds it.
fn unique_names(part: &mut Part, seen: &mut HashSet<String>) {
    if !seen.insert(part.name.clone()) {
        let renamed = (2..)
            .map(|n| format!("{}#{n}", part.name))
            .find(|candidate| !seen.contains(candidate))
            .unwrap_or_default();
        seen.insert(renamed.clone());
        part.name = renamed;
    }
    for child in &mut part.children {
        unique_names(child, seen);
    }
}

/// The clip whose name has the first of `words` that any has, the shortest such name
/// (`walk` before `walk_baby`).
fn pick_clip(clips: &[(&str, &Animation)], words: &[&str]) -> Option<Animation> {
    words.iter().find_map(|word| {
        clips
            .iter()
            .filter(|(name, _)| name.contains(word))
            .min_by_key(|(name, _)| name.len())
            .map(|(_, clip)| (*clip).clone())
    })
}

/// A part name's words in lower case: split at `_`, `.`, case changes and digits
/// (`LeftFrontLeg`, `left_front_leg` and `leftFrontLeg` are all `left front leg`).
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = previous.is_some_and(|p| {
            (c.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
                || (c.is_ascii_uppercase()
                    && p.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase()))
                || (c.is_ascii_digit() != p.is_ascii_digit() && p.is_ascii_alphanumeric())
        });
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A leg or arm part's swing (phase, amplitude), when its name says which side it is on: legs
/// like QuadrupedModel's (right hind and left front in step) or HumanoidModel's, arms opposite
/// their side's leg.
fn limb_swing(name: &str) -> Option<(f32, f32)> {
    let words = words(name);
    let joined = words.concat();
    let has = |w: &str| words.iter().any(|x| x == w);
    let any = |list: &[&str]| list.iter().any(|w| joined.contains(w));
    let leg = (has("leg") || has("legs") || any(&["leftleg", "rightleg", "frontleg", "hindleg"]))
        && !joined.contains("legging");
    let arm = !leg
        && (has("arm") || has("arms") || any(&["leftarm", "rightarm"]))
        && !any(&["armor", "armour"]);
    let left = joined.contains("left") || has("l");
    let right = joined.contains("right") || has("r");
    if (!leg && !arm) || left == right {
        return None;
    }
    if arm {
        return Some((if left { 0.0 } else { PI }, ARM_SWING));
    }
    let front = any(&["front", "fore"]);
    Some((if left != front { PI } else { 0.0 }, LEG_SWING))
}

/// `leg0`..`leg3` (or `leg1`..`leg4`): the old QuadrupedModel's right hind, left hind, right
/// front and left front legs.
fn numbered_leg(name: &str) -> Option<u32> {
    match words(name).as_slice() {
        [leg, n] if leg == "leg" => n.parse().ok(),
        _ => None,
    }
}

/// The limbs of `model`: the outermost parts named as legs or arms (their own sub-parts move
/// with them).
fn limbs(model: &Part) -> Vec<Limb> {
    fn walk(part: &Part, named: &mut Vec<Limb>, numbered: &mut Vec<(u32, String)>) {
        if let Some((phase, amplitude)) = limb_swing(&part.name) {
            named.push(Limb {
                part: part.name.clone(),
                phase,
                amplitude,
            });
            return;
        }
        if let Some(n) = numbered_leg(&part.name) {
            numbered.push((n, part.name.clone()));
            return;
        }
        for child in &part.children {
            walk(child, named, numbered);
        }
    }
    let mut named = Vec::new();
    let mut numbered = Vec::new();
    walk(model, &mut named, &mut numbered);
    numbered.sort();
    let first = numbered.first().map_or(0, |(n, _)| *n);
    let quadruped = numbered.len() == 4
        && first <= 1
        && numbered
            .iter()
            .enumerate()
            .all(|(i, (n, _))| *n == first + i as u32);
    if quadruped {
        for ((_, part), phase) in numbered.into_iter().zip([0.0, PI, PI, 0.0]) {
            named.push(Limb {
                part,
                phase,
                amplitude: LEG_SWING,
            });
        }
    }
    named
}

/// The shallowest part named `head` (in any case).
fn find_head(model: &Part) -> Option<String> {
    let mut queue = VecDeque::from([model]);
    while let Some(part) = queue.pop_front() {
        if words(&part.name) == ["head"] {
            return Some(part.name.clone());
        }
        queue.extend(part.children.iter());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small dump: `test:boar` (a quadruped with legs, a walk and an idle clip),
    /// `test:wisp` (texture never copied), `test:dart` (not living) and `test:ghost` (its
    /// layer is missing).
    const FIXTURE: &str = r#"{
      "format": 1,
      "entities": {
        "test:boar": {"renderer": "test.BoarRenderer", "system": "layer", "living": true,
          "layer": "test:boar#main", "texture": "test:textures/entity/boar.png",
          "width": 1.4, "height": 1.2, "eye_height": 1.0,
          "animations": {
            "walk": {"length": 1.0, "looping": true, "bones": {"body": [{"target": "rotation",
              "keyframes": [[0.0, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], "linear"],
                            [1.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], "linear"]]}]}},
            "walk_baby": {"length": 1.0, "looping": true, "bones": {}},
            "idle": {"length": 2.0, "looping": true, "bones": {"Head": [{"target": "position",
              "keyframes": [[0.0, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], "linear"],
                            [2.0, [0.0, -2.0, 0.0], [0.0, -2.0, 0.0], "linear"]]}]},
            "death": {"length": 10.0, "looping": false, "bones": {}}
          }},
        "test:wisp": {"living": true, "layer": "test:boar#main",
          "texture": "test:textures/entity/missing_wisp.png"},
        "test:dart": {"living": false, "layer": "test:boar#main",
          "texture": "test:textures/entity/boar.png"},
        "test:ghost": {"living": true, "layer": "test:ghost#main",
          "texture": "test:textures/entity/boar.png"}
      },
      "models": {
        "test:boar#main": {"pose": [0, 0, 0, 0, 0, 0, 1, 1, 1], "polygons": [], "normals": [],
          "children": {
            "body": {"pose": [0, 16, 0, 0, 0, 0, 1, 1, 1],
              "polygons": [[[-4, -4, -4, 0.0, 0.0], [4, -4, -4, 0.5, 0.0],
                            [4, 4, -4, 0.5, 0.5], [-4, 4, -4, 0.0, 0.5]]],
              "normals": [[0, 0, -1]],
              "children": {"cube_r1": {"pose": [0, 0, 0, 0.5, 0, 0, 1, 1, 1], "polygons": [],
                "normals": [], "children": {}}}},
            "Head": {"pose": [0, 12, -6, 0.1, 0, 0, 1, 1, 1], "polygons": [], "normals": [],
              "children": {"cube_r1": {"pose": [0, 0, 0, -0.5, 0, 0, 1, 1, 1], "polygons": [],
                "normals": [], "children": {}}}},
            "leftFrontLeg": {"pose": [3, 18, -5, 0, 0, 0, 1, 1, 1], "polygons": [],
              "normals": [], "children": {"left_front_leg_lower": {"pose": [0, 3, 0, 0, 0, 0,
                1, 1, 1], "polygons": [], "normals": [], "children": {}}}},
            "right_front_leg": {"pose": [-3, 18, -5, 0, 0, 0, 1, 1, 1], "polygons": [],
              "normals": [], "children": {}},
            "LegL": {"pose": [3, 18, 5, 0, 0, 0, 1, 1, 1], "polygons": [], "normals": [],
              "children": {}},
            "RightHindLeg": {"pose": [-3, 18, 5, 0, 0, 0, 1, 1, 1], "polygons": [],
              "normals": [], "children": {}},
            "armor": {"pose": [0, 0, 0, 0, 0, 0, 1, 1, 1], "polygons": [], "normals": [],
              "children": {}}
          }}
      }
    }"#;

    fn exists_unless_missing(path: &Path) -> bool {
        !path.to_string_lossy().contains("missing")
    }

    fn library() -> Library {
        Library::parse(FIXTURE, PathBuf::from("dump"), exists_unless_missing)
            .expect("the fixture parses")
    }

    fn state(pos: f32, speed: f32) -> State {
        State {
            pos,
            speed,
            age: 0.0,
            y_rot: 0.0,
            x_rot: 0.0,
            swing: 0.0,
            baby: false,
            partial: 0.0,
        }
    }

    fn find<'a>(part: &'a Part, name: &str) -> Option<&'a Part> {
        if part.name == name {
            return Some(part);
        }
        part.children.iter().find_map(|c| find(c, name))
    }

    #[test]
    fn only_living_entities_with_a_model_and_a_copied_texture_are_drawn() {
        let library = library();
        assert_eq!(library.counts(), (4, 1));
        let boar = library.mob("test:boar").expect("boar is drawn");
        assert_eq!(boar.texture, "test:entity/boar");
        assert_eq!(
            (boar.width, boar.height, boar.eye_height),
            (Some(1.4), Some(1.2), Some(1.0))
        );
        for id in ["test:wisp", "test:dart", "test:ghost", "test:unknown"] {
            assert!(library.mob(id).is_none(), "{id} has no model");
        }
        // The decoded part tree and selected clips replace their raw JSON after first use.
        assert!(library.models.lock().unwrap().is_empty());
        assert!(library.animations.lock().unwrap().is_empty());
        // Consuming raw model data does not change the drawable count.
        assert_eq!(library.counts(), (4, 1));
    }

    #[test]
    fn the_model_tree_parses_with_unique_part_names() {
        let boar = library().mob("test:boar").expect("boar is drawn");
        let body = find(&boar.model, "body").expect("body");
        assert_eq!(body.pose[1], 16.0);
        assert_eq!(body.polygons.len(), 1);
        assert_eq!(body.polygons[0].normal, [0.0, 0.0, -1.0]);
        // Children sort by name: `Head` before `body`, so the head's helper keeps the name.
        assert_eq!(find(&boar.model, "cube_r1").map(|p| p.pose[3]), Some(-0.5));
        assert_eq!(find(&boar.model, "cube_r1#2").map(|p| p.pose[3]), Some(0.5));
    }

    #[test]
    fn clips_head_and_limbs_come_from_the_names() {
        let boar = library().mob("test:boar").expect("boar is drawn");
        assert_eq!(boar.head.as_deref(), Some("Head"));
        assert!(boar.walk.as_ref().is_some_and(|w| w.bones.len() == 1));
        assert!(boar.idle.is_some());
        let mut limbs: Vec<(&str, f32)> = boar
            .limbs
            .iter()
            .map(|l| (l.part.as_str(), l.phase))
            .collect();
        limbs.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            limbs,
            [
                ("LegL", PI),
                ("RightHindLeg", 0.0),
                ("leftFrontLeg", 0.0),
                ("right_front_leg", PI)
            ]
        );
    }

    #[test]
    fn mob_types_take_the_dumped_model_texture_and_size() {
        let template = crate::mobs::sensor_scan_tests::kind();
        let kind = library()
            .mob_type(&template, "test:boar")
            .expect("boar has a type");
        assert_eq!(kind.id, "test:boar");
        assert_eq!(kind.anim, Anim::Modded);
        assert_eq!(kind.texture, "test:entity/boar");
        assert_eq!((kind.width, kind.height, kind.eye_height), (1.4, 1.2, 1.0));
        assert!(kind.layers.is_empty());
        assert!(library().mob_type(&template, "test:dart").is_none());
    }

    #[test]
    fn walking_plays_the_walk_clip_and_standing_the_idle_one() {
        let boar = library().mob("test:boar").expect("boar is drawn");
        let mut walking = Poses::new(&boar.model);
        // Limb position 5 at speed 0.5: 5 * 50 * 2 ms = 0.5 s into the clip, at scale 1.
        pose(&boar, &state(5.0, 0.5), &mut walking);
        assert!((walking.get("body")[3] - 0.5).abs() < 1e-5);
        assert_eq!(walking.get("leftFrontLeg")[3], 0.0);
        let mut standing = Poses::new(&boar.model);
        let mut s = state(0.0, 0.0);
        s.age = 20.0;
        pose(&boar, &s, &mut standing);
        assert_eq!(standing.get("body")[3], 0.0);
        // The idle clip one second in moves the head half way, from the model's 12.
        assert!((standing.get("Head")[1] - 11.0).abs() < 1e-5);
    }

    #[test]
    fn limbs_swing_opposite_without_a_walk_clip_and_the_head_looks() {
        let mut boar = library().build("test:boar").expect("boar is drawn");
        boar.walk = None;
        let mut poses = Poses::new(&boar.model);
        let mut s = state(0.0, 1.0);
        s.x_rot = 30.0;
        s.y_rot = -45.0;
        pose(&boar, &s, &mut poses);
        assert!((poses.get("RightHindLeg")[3] - LEG_SWING).abs() < 1e-5);
        assert!((poses.get("LegL")[3] + LEG_SWING).abs() < 1e-5);
        assert!((poses.get("right_front_leg")[3] + LEG_SWING).abs() < 1e-5);
        assert!((poses.get("leftFrontLeg")[3] - LEG_SWING).abs() < 1e-5);
        assert_eq!(poses.get("left_front_leg_lower")[3], 0.0);
        assert!((poses.get("Head")[3] - (0.1 + 30f32.to_radians())).abs() < 1e-5);
        assert!((poses.get("Head")[4] + 45f32.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn names_split_into_words() {
        assert_eq!(words("LeftFrontLeg"), ["left", "front", "leg"]);
        assert_eq!(words("left_front_leg"), ["left", "front", "leg"]);
        assert_eq!(words("LegL"), ["leg", "l"]);
        assert_eq!(words("RArm"), ["r", "arm"]);
        assert_eq!(words("leg0"), ["leg", "0"]);
        assert_eq!(words("cube_r1"), ["cube", "r", "1"]);
    }

    #[test]
    fn limb_names_say_their_swing() {
        assert_eq!(limb_swing("right_hind_leg"), Some((0.0, LEG_SWING)));
        assert_eq!(limb_swing("left_hind_leg"), Some((PI, LEG_SWING)));
        assert_eq!(limb_swing("RightFrontLeg"), Some((PI, LEG_SWING)));
        assert_eq!(limb_swing("left_leg"), Some((PI, LEG_SWING)));
        assert_eq!(limb_swing("rightArm"), Some((PI, ARM_SWING)));
        assert_eq!(limb_swing("ArmL"), Some((0.0, ARM_SWING)));
        for not_a_limb in ["armor", "legs", "leggings_left", "farm", "body", "cube_r1"] {
            assert_eq!(limb_swing(not_a_limb), None, "{not_a_limb}");
        }
    }

    #[test]
    fn four_numbered_legs_swing_like_the_old_quadruped() {
        let tree = serde_json::json!({"children": {
            "leg0": {}, "leg1": {}, "leg2": {}, "leg3": {}, "leg_spine": {}
        }});
        let model = Part::parse("root", &tree);
        let swings: Vec<(String, f32)> = limbs(&model)
            .into_iter()
            .map(|l| (l.part, l.phase))
            .collect();
        assert_eq!(
            swings,
            [
                ("leg0".to_owned(), 0.0),
                ("leg1".to_owned(), PI),
                ("leg2".to_owned(), PI),
                ("leg3".to_owned(), 0.0)
            ]
        );
        let spider = serde_json::json!({"children": {
            "leg1": {}, "leg2": {}, "leg3": {}, "leg4": {}, "leg5": {}, "leg6": {}
        }});
        assert!(limbs(&Part::parse("root", &spider)).is_empty());
    }

    #[test]
    fn textures_name_their_sprite_and_file() {
        assert_eq!(
            sprite("cataclysm:textures/entity/ignis/ignis.png").as_deref(),
            Some("cataclysm:entity/ignis/ignis")
        );
        assert_eq!(
            sprite("minecraft:textures/entity/pig/pig.png").as_deref(),
            Some("entity/pig/pig")
        );
        assert_eq!(sprite("ns:models/x.png"), None);
        assert_eq!(
            texture_file(Path::new("d"), "ns:textures/entity/a.png"),
            Some(Path::new("d").join("ns").join("textures/entity/a.png"))
        );
        assert_eq!(
            texture_file(Path::new("d"), "ns:textures/../../x.png"),
            None
        );
    }

    #[test]
    fn a_broken_dump_is_an_error() {
        assert!(Library::parse("{\"entities\": 3}", PathBuf::new(), Path::is_file).is_err());
        let empty = Library::parse("{}", PathBuf::new(), Path::is_file).expect("empty parses");
        assert_eq!(empty.counts(), (0, 0));
    }

    #[test]
    fn unused_animation_values_are_still_validated() {
        let invalid = r#"{"entities":{"test:boar":{"animations":{
            "death":{"value":1e999},"death":{}
        }}}}"#;
        assert!(Library::parse(invalid, PathBuf::new(), Path::is_file).is_err());
        assert!(pose_clip_name("walk_baby"));
        assert!(!pose_clip_name("death"));
    }
}
