//! The server's blocks no content mod declares. A modded server numbers
//! about 1.2 million states, and a block whose mod ships its blockstate but
//! declares no template for it was air here. Each such block joins the root
//! with the server's own states (its state table, the one
//! `game::server_states` maps from), after every other block, on a neutral
//! template: stone for a block whose models are full cubes, stone without
//! collision or occlusion otherwise (better a chair to walk through than an
//! invisible wall), unless its name looks like a vanilla block (stairs, a
//! door, a fence...): then each state takes the shapes of the vanilla state
//! sharing the most of its properties. Every Compat's names stay mapped onto their module's
//! blocks and are never added.
use super::{ContentMod, J, MAX_STATES};
use crate::Result;
use horizon_mods::content::FileReader;
use horizon_world::{
    level::data::state_flags,
    nbt::{Compound, JavaString, Tag},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::BufRead,
    ops::Range,
    path::{Path, PathBuf},
};

/// The block whose rows a server block copies.
pub(super) const TEMPLATE: &str = "minecraft:stone";
/// Whose shapes (collision, occlusion, support) a block that is no cube takes.
pub(super) const OPEN: &str = "minecraft:air";
/// Every Compat's generated blocks, which `game::server_states` maps onto
/// their module's own blocks.
const EVERY_COMPAT: &str = "everycomp:";
/// Model parents followed before a model counts as no cube.
const MAX_PARENTS: u8 = 16;

/// A server block being added.
pub(super) struct ServerBlock {
    pub name: String,
    /// Each state's `key=value,...` as the table has it, in its order.
    pub states: Vec<String>,
    /// Its first state id here.
    pub first: u32,
    /// Every model of its blockstate is a full cube in shape: its collision.
    pub cube: bool,
    /// And renders opaque: only then does it occlude its neighbours.
    pub opaque: bool,
    /// For a block that is no cube whose name looks like a vanilla block: each
    /// state's vanilla state, whose shapes it takes. Empty: air's.
    pub donors: Vec<u32>,
}
impl ServerBlock {
    pub fn ids(&self) -> Range<u32> {
        self.first..self.first + self.states.len() as u32
    }

    /// The vanilla state that state `index`'s shapes come from.
    pub fn donor(&self, index: usize) -> Option<u32> {
        self.donors.get(index).copied()
    }
}

/// A vanilla block's states, as (id, properties), and its default's id.
pub(super) struct VanillaBlock {
    pub default: u32,
    pub states: Vec<(u32, Vec<(String, String)>)>,
}
impl VanillaBlock {
    /// The state sharing the most of `properties`, then of the default's: the
    /// default with every property the block also has overwritten (as
    /// `game::server_states` maps), or the nearest state when the block lacks
    /// one of the values.
    fn closest(&self, properties: &[(String, String)]) -> u32 {
        let default = self
            .states
            .iter()
            .find(|(id, _)| *id == self.default)
            .map(|(_, p)| p.as_slice())
            .unwrap_or_default();
        let shared = |state: &[(String, String)], wanted: &[(String, String)]| {
            wanted.iter().filter(|kv| state.contains(kv)).count()
        };
        self.states
            .iter()
            .max_by_key(|(id, state)| {
                (
                    shared(state, properties),
                    shared(state, default),
                    // Ties go to the lowest id.
                    std::cmp::Reverse(*id),
                )
            })
            .map_or(self.default, |(id, _)| *id)
    }
}

/// Gives each block that is no cube, whose name `lookalike` maps to a vanilla
/// block `vanilla` knows, its donor states. Vanilla blocks are read once.
pub(super) fn pick_donors(
    blocks: &mut [ServerBlock],
    mut vanilla: impl FnMut(&str) -> Option<VanillaBlock>,
) {
    let mut known: HashMap<String, Option<VanillaBlock>> = HashMap::new();
    for b in blocks.iter_mut().filter(|b| !b.cube) {
        let Some(name) = crate::lookalike::lookalike(&b.name) else {
            continue;
        };
        let Some(block) = known
            .entry(name)
            .or_insert_with_key(|name| vanilla(name))
            .as_ref()
        else {
            continue;
        };
        b.donors = b
            .states
            .iter()
            .map(|text| block.closest(&parse_properties(text)))
            .collect();
    }
}

/// A `facts.json` state row's properties, in order.
pub(super) fn state_properties(row: &J) -> Vec<(String, String)> {
    let J::Arr(columns) = row else {
        return Vec::new();
    };
    let Some(J::Obj(state)) = columns.get(4) else {
        return Vec::new();
    };
    let Some((_, J::Obj(properties))) = state.iter().find(|(key, _)| key == "properties") else {
        return Vec::new();
    };
    properties
        .iter()
        .filter_map(|(key, value)| match value {
            J::Str(value) => Some((key.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

/// The server's block state table: `HORIZON_SERVER_STATES`, else
/// `server-states/server-states.tsv` in the game's folder, as
/// `game::server_states` reads it; none when it is not there.
pub(super) fn table() -> Option<PathBuf> {
    let path = std::env::var_os("HORIZON_SERVER_STATES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("server-states").join("server-states.tsv"));
    path.is_file().then_some(path)
}

/// The blocks of the table at `path` that are not `known` (vanilla's, the
/// declared ones) and whose blockstate a mod ships, by name, with their ids
/// from `next`; and the warnings for the ones left out.
pub(super) fn blocks(
    base: &Path,
    mods: &[ContentMod],
    path: &Path,
    known: impl Fn(&str) -> bool,
    next: u32,
) -> Result<(Vec<ServerBlock>, Vec<String>)> {
    let mut assets = ModAssets::new(base, mods);
    blocks_with_assets(&mut assets, path, known, next)
}

pub(super) fn blocks_with_assets(
    assets: &mut ModAssets<'_>,
    path: &Path,
    known: impl Fn(&str) -> bool,
    next: u32,
) -> Result<(Vec<ServerBlock>, Vec<String>)> {
    let found = read_table(path, |name| !known(name) && assets.has_blockstate(name))?;
    let blocks = found
        .into_iter()
        .map(|(name, states)| {
            let cube = assets.cube(&name);
            let opaque = cube && assets.opaque_cube(&name);
            ServerBlock {
                name,
                states,
                first: 0,
                cube,
                opaque,
                donors: Vec::new(),
            }
        })
        .collect();
    let mut warnings = Vec::new();
    let placed = place(blocks, next, MAX_STATES, &mut warnings);
    log_added(path, &placed);
    Ok((placed, warnings))
}

/// The table's lines are `id \t block \t key=value,...` after a `#`
/// header. Streamed (1.2 million lines): only the blocks `wanted` keeps are
/// held, each state's properties in table order. Every Compat's never.
pub(super) fn read_table(
    path: &Path,
    mut wanted: impl FnMut(&str) -> bool,
) -> Result<BTreeMap<String, Vec<String>>> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut blocks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut line, mut last, mut keep) = (String::new(), String::new(), false);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let text = line.trim_end_matches(['\r', '\n']);
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let mut cols = text.splitn(3, '\t');
        let (Some(_), Some(name)) = (cols.next(), cols.next()) else {
            continue;
        };
        // A block's states are one run of lines: decide once per run.
        if name != last {
            last.clear();
            last.push_str(name);
            keep = !name.starts_with(EVERY_COMPAT) && wanted(name);
        }
        if keep {
            let properties = cols.next().unwrap_or_default().to_owned();
            match blocks.get_mut(name) {
                Some(states) => states.push(properties),
                None => {
                    blocks.insert(name.to_owned(), vec![properties]);
                }
            }
        }
    }
    Ok(blocks)
}

fn namespace(name: &str) -> &str {
    name.split_once(':').map_or("minecraft", |(ns, _)| ns)
}

/// Gives the blocks their ids from `start`, a namespace at a time: one
/// whose states would take the registry past `cap` is left out with a
/// warning (its states stay as `game::server_states` maps them), instead of
/// refusing the whole root as too many declared states are.
pub(super) fn place(
    blocks: Vec<ServerBlock>,
    start: u32,
    cap: u32,
    warnings: &mut Vec<String>,
) -> Vec<ServerBlock> {
    let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
    for b in &blocks {
        *totals.entry(namespace(&b.name)).or_default() += b.states.len() as u64;
    }
    let mut next = u64::from(start);
    let mut kept = BTreeSet::new();
    for (ns, n) in totals {
        if next + n > u64::from(cap) {
            warnings.push(format!(
                "the server's {ns} blocks are left out: their {n} states would take the \
                 registry past the {cap} a world can hold"
            ));
        } else {
            next += n;
            kept.insert(ns.to_owned());
        }
    }
    let mut next = start;
    blocks
        .into_iter()
        .filter(|b| kept.contains(namespace(&b.name)))
        .map(|mut b| {
            b.first = next;
            next += b.states.len() as u32;
            b
        })
        .collect()
}

fn log_added(path: &Path, blocks: &[ServerBlock]) {
    let mut by_ns: BTreeMap<&str, usize> = BTreeMap::new();
    for b in blocks {
        *by_ns.entry(namespace(&b.name)).or_default() += b.states.len();
    }
    let states: usize = by_ns.values().sum();
    let cubes = blocks.iter().filter(|b| b.cube).count();
    let list: Vec<String> = by_ns.iter().map(|(ns, n)| format!("{ns} {n}")).collect();
    log::info!(
        "Horizon: {states} states of {} server blocks no mod declares, from {}: {}; \
         {cubes} full cubes on {TEMPLATE}, {} others without collision",
        blocks.len(),
        path.display(),
        list.join(", "),
        blocks.len() - cubes
    );
    for b in blocks {
        log::debug!(
            "Horizon: server block {} ({} states): {}",
            b.name,
            b.states.len(),
            if b.cube {
                "full cube, template collision"
            } else {
                "no cube, no collision"
            }
        );
    }
}

/// The mods' asset files and, behind them, the base's: for the server
/// blocks' blockstates and models.
pub(super) struct ModAssets<'a> {
    base: &'a Path,
    mods: &'a [ContentMod],
    readers: Vec<Option<FileReader<'a>>>,
    /// `ns:path` of every blockstate a mod ships.
    blockstates: BTreeSet<String>,
    /// Whether each model read is a full cube in shape.
    cubes: HashMap<String, bool>,
    /// Whether each model read renders opaque (no cutout/translucent, no
    /// see-through vanilla parent).
    opaque: HashMap<String, bool>,
}
impl<'a> ModAssets<'a> {
    pub(super) fn new(base: &'a Path, mods: &'a [ContentMod]) -> Self {
        let blockstates = mods
            .iter()
            .flat_map(|m| &m.content.files)
            .filter_map(|file| {
                let (ns, path) = file.strip_prefix("assets/")?.split_once("/blockstates/")?;
                Some(format!("{ns}:{}", path.strip_suffix(".json")?))
            })
            .collect();
        ModAssets {
            base,
            mods,
            readers: mods.iter().map(|_| None).collect(),
            blockstates,
            cubes: HashMap::new(),
            opaque: HashMap::new(),
        }
    }

    pub(super) fn has_blockstate(&self, name: &str) -> bool {
        self.blockstates.contains(name)
    }

    /// A file as the root has it: the last mod's that has it, else the base's.
    fn read(&mut self, file: &str) -> Option<Vec<u8>> {
        let mods = self.mods;
        for (i, m) in mods.iter().enumerate().rev() {
            if m.content
                .files
                .binary_search_by(|f| f.as_str().cmp(file))
                .is_err()
            {
                continue;
            }
            if self.readers[i].is_none() {
                self.readers[i] = m.content.reader().ok();
            }
            return self.readers[i].as_mut()?.read(file).ok();
        }
        fs::read(self.base.join(file)).ok()
    }

    fn json(&mut self, file: &str) -> Option<Value> {
        serde_json::from_slice(&self.read(file)?).ok()
    }

    /// The models of the block's blockstate variants. None for a multipart
    /// blockstate (fences, walls, connected furniture) or one that does not
    /// read; never empty.
    fn variant_models(&mut self, name: &str) -> Option<Vec<String>> {
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        let doc = self.json(&format!("assets/{ns}/blockstates/{path}.json"))?;
        if doc.get("multipart").is_some() {
            return None;
        }
        let variants = doc.get("variants").and_then(Value::as_object)?;
        let mut models = Vec::new();
        for variant in variants.values() {
            match variant {
                Value::Array(choices) => {
                    models.extend(choices.iter().filter_map(|c| c.get("model")?.as_str()))
                }
                choice => models.extend(choice.get("model").and_then(Value::as_str)),
            }
        }
        let models: Vec<String> = models.into_iter().map(str::to_owned).collect();
        (!models.is_empty()).then_some(models)
    }

    /// Whether every model of the block's blockstate variants is a full cube
    /// in shape, whatever its render type: what its collision follows (leaves,
    /// glass and ice are solid cubes).
    pub(super) fn cube(&mut self, name: &str) -> bool {
        self.variant_models(name)
            .is_some_and(|models| models.iter().all(|m| self.cube_model(m, 0)))
    }

    /// Whether every model is a full cube that hides what is behind it: no
    /// cutout/translucent render type or see-through vanilla parent (leaves).
    /// Only this may occlude neighbours; a see-through cube culling the faces
    /// next to it shows holes into the void.
    pub(super) fn opaque_cube(&mut self, name: &str) -> bool {
        self.variant_models(name).is_some_and(|models| {
            models
                .iter()
                .all(|m| self.cube_model(m, 0) && self.render_opaque(m, 0))
        })
    }

    /// Whether no variant model is see-through. A multipart or unreadable
    /// blockstate cannot tell and counts as opaque.
    pub(super) fn render_opaque_block(&mut self, name: &str) -> bool {
        self.variant_models(name)
            .is_none_or(|models| models.iter().all(|m| self.render_opaque(m, 0)))
    }

    /// Whether every model is a plant, rail or carpet through its vanilla
    /// parents, or is made of flat planes only (patches, petals, crossed
    /// sprites): a block the player walks through (or, for a carpet, nearly).
    /// Other shapes keep their template's collision, a closer guess than none.
    pub(super) fn walk_through(&mut self, name: &str) -> bool {
        self.variant_models(name)
            .is_some_and(|models| models.iter().all(|m| self.walk_through_model(m, 0)))
    }

    fn walk_through_model(&mut self, model: &str, depth: u8) -> bool {
        let (ns, path) = model.split_once(':').unwrap_or(("minecraft", model));
        if ns == "minecraft" && walk_through_vanilla_parent(path) {
            return true;
        }
        let Some(doc) = self.json(&format!("assets/{ns}/models/{path}.json")) else {
            return false;
        };
        if doc.get("loader").is_some() {
            return false;
        }
        match (
            doc.get("elements"),
            doc.get("parent").and_then(Value::as_str),
        ) {
            (Some(elements), _) => only_planes(elements),
            (None, Some(parent)) if depth < MAX_PARENTS => {
                let parent = parent.to_owned();
                self.walk_through_model(&parent, depth + 1)
            }
            _ => false,
        }
    }

    /// No cutout/translucent render type on the model or its parents, and no
    /// see-through vanilla parent.
    fn render_opaque(&mut self, model: &str, depth: u8) -> bool {
        let (ns, path) = model.split_once(':').unwrap_or(("minecraft", model));
        let key = format!("{ns}:{path}");
        if let Some(&opaque) = self.opaque.get(&key) {
            return opaque;
        }
        let opaque = !non_opaque_vanilla_parent(&key)
            && match self.json(&format!("assets/{ns}/models/{path}.json")) {
                Some(doc) if non_opaque_render_type(&doc) => false,
                Some(doc) => match doc.get("parent").and_then(Value::as_str) {
                    Some(parent) if depth < MAX_PARENTS => {
                        let parent = parent.to_owned();
                        self.render_opaque(&parent, depth + 1)
                    }
                    _ => true,
                },
                None => true,
            };
        self.opaque.insert(key, opaque);
        opaque
    }

    /// A model is a full cube when the first of it and its parents with
    /// elements has one element from 0,0,0 to 16,16,16 (vanilla's
    /// `block/cube`, under `cube_all`, `cube_column` ...). A loader model
    /// (OBJ, composite, connected textures) or one that does not read is not.
    fn cube_model(&mut self, model: &str, depth: u8) -> bool {
        let (ns, path) = model.split_once(':').unwrap_or(("minecraft", model));
        let key = format!("{ns}:{path}");
        if let Some(&cube) = self.cubes.get(&key) {
            return cube;
        }
        let cube = match self.json(&format!("assets/{ns}/models/{path}.json")) {
            Some(doc) if doc.get("loader").is_none() => {
                match (
                    doc.get("elements"),
                    doc.get("parent").and_then(Value::as_str),
                ) {
                    (Some(elements), _) => full_cube(elements),
                    (None, Some(parent)) if depth < MAX_PARENTS => {
                        let parent = parent.to_owned();
                        self.cube_model(&parent, depth + 1)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        self.cubes.insert(key, cube);
        cube
    }
}

/// Elements that are all flat planes (no thickness on some axis): nothing to
/// stand on or bump into.
fn only_planes(elements: &Value) -> bool {
    let corner = |element: &Value, key: &str| -> Option<[f64; 3]> {
        let at = element.get(key)?.as_array()?;
        Some([at.first()?.as_f64()?, at.get(1)?.as_f64()?, at.get(2)?.as_f64()?])
    };
    elements.as_array().is_some_and(|list| {
        !list.is_empty()
            && list.iter().all(|element| {
                match (corner(element, "from"), corner(element, "to")) {
                    (Some(from), Some(to)) => {
                        from.iter().zip(&to).any(|(a, b)| (a - b).abs() < 1e-6)
                    }
                    _ => false,
                }
            })
    })
}

/// Vanilla parents of blocks with no collision (plants, crops, rails) or
/// almost none (carpets, flower beds).
fn walk_through_vanilla_parent(path: &str) -> bool {
    matches!(
        path,
        "block/cross"
            | "block/tinted_cross"
            | "block/crop"
            | "block/carpet"
            | "block/rail_flat"
            | "block/rail_raised_ne"
            | "block/rail_raised_sw"
            | "block/flowerbed_1"
            | "block/flowerbed_2"
            | "block/flowerbed_3"
            | "block/flowerbed_4"
    )
}

fn non_opaque_render_type(doc: &Value) -> bool {
    matches!(
        doc.get("render_type").and_then(Value::as_str),
        Some(
            "cutout"
                | "minecraft:cutout"
                | "cutout_mipped"
                | "minecraft:cutout_mipped"
                | "translucent"
                | "minecraft:translucent"
                | "tripwire"
                | "minecraft:tripwire"
        )
    )
}

fn non_opaque_vanilla_parent(key: &str) -> bool {
    matches!(
        key.strip_prefix("minecraft:").unwrap_or(key),
        "block/cross"
            | "block/tinted_cross"
            | "block/leaves"
            | "block/crop"
            | "block/flower_pot_cross"
            | "block/carpet"
            | "block/rail_flat"
            | "block/rail_raised_ne"
            | "block/rail_raised_sw"
    )
}

fn full_cube(elements: &Value) -> bool {
    let corner = |element: &Value, key: &str, at: f64| {
        element
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|c| c.len() == 3 && c.iter().all(|v| v.as_f64() == Some(at)))
    };
    matches!(
        elements.as_array().map(Vec::as_slice),
        Some([element]) if corner(element, "from", 0.0) && corner(element, "to", 16.0)
    )
}

/// `key=value,...` as pairs, in order.
pub(super) fn parse_properties(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Sets a `facts.json` row's state to `name` with exactly the properties
/// `text` lists, and its description to match. Upstream #5306's
/// `set_state_properties` adds declared properties to a template's; a server
/// block's are the server's whole, so they replace the template's.
pub(super) fn set_state_properties(row: &mut J, name: &str, text: &str) {
    let J::Arr(columns) = row else {
        return;
    };
    let properties = parse_properties(text);
    if let Some(description) = columns.get_mut(3) {
        *description = J::Str(match properties.is_empty() {
            true => format!("Block{{{name}}}"),
            false => format!("Block{{{name}}}[{text}]"),
        });
    }
    let mut state = Vec::new();
    if !properties.is_empty() {
        let values = properties.into_iter().map(|(k, v)| (k, J::Str(v)));
        state.push(("properties".to_owned(), J::Obj(values.collect())));
    }
    state.push(("id".to_owned(), J::Str(name.to_owned())));
    if let Some(slot) = columns.get_mut(4) {
        *slot = J::Obj(state);
    }
}

/// An NBT table's state identity: `name` with exactly the properties `text` lists.
pub(super) fn set_nbt_state(state: &mut Tag, name: &str, text: &str) {
    if !matches!(state, Tag::Compound(_)) {
        *state = Tag::Compound(Compound::new());
    }
    let Tag::Compound(c) = state else {
        return;
    };
    c.insert("id".into(), Tag::String(name.into()));
    let properties: Compound = parse_properties(text)
        .iter()
        .map(|(k, v)| (k.as_str().into(), Tag::String(v.as_str().into())))
        .collect();
    if properties.is_empty() {
        c.remove(&JavaString::from("properties"));
    } else {
        c.insert("properties".into(), Tag::Compound(properties));
    }
}

/// A `block-behaviours.json` state row for a block that is no cube: no
/// sturdy face and nothing to collide with.
pub(super) fn open_behaviour(row: &mut J) {
    if let J::Arr(columns) = row
        && columns.len() > 1
    {
        columns[0] = J::Num(0.into());
        columns[1] = J::Num(state_flags::COLLISION_EMPTY.into());
    }
}

/// A `block-behaviours.json` state row of a block that takes the shapes of
/// the vanilla state whose row is `donor`: its sturdy faces and flags.
pub(super) fn donor_behaviour(row: &mut J, donor: &J) {
    if let (J::Arr(columns), J::Arr(donor)) = (row, donor)
        && columns.len() > 1
        && donor.len() > 1
    {
        columns[0] = donor[0].clone();
        columns[1] = donor[1].clone();
    }
}

/// Removes occlusion flags (and optionally full collision) from a declared mod
/// block gameplay row that copied an opaque template like `minecraft:stone`.
pub(super) fn clear_occlusion_gameplay(row: &mut J, also_clear_collision: bool) {
    if also_clear_collision {
        row.set("full_collision", J::Bool(false));
    }
    for (key, value) in [
        ("can_occlude", false),
        ("solid_render", false),
        ("shape_occlusion", false),
        ("skylight_down", true),
    ] {
        row.set(key, J::Bool(value));
    }
    row.set("dampening", J::Num(0.into()));
}

/// A server block's gameplay row without its `id`, compact: 176k full
/// copies of the template's row would add about 200 MB to
/// gameplay-facts.json. Every reader defaults what is left out (false
/// flags, a full-cube outline and its edges, unit sound volume and pitch).
/// The block has no item here. One that is no cube lets light, sight and
/// walkers through; a see-through cube (leaves, glass) only light and sight.
/// Its outline stays a full cube, so it can be aimed at.
pub(super) fn gameplay_row(template: &J, cube: bool, opaque: bool) -> Result<String> {
    let mut row = template.clone();
    row.remove("id");
    if !opaque {
        clear_occlusion_gameplay(&mut row, !cube);
    }
    compact_row(row)
}

/// The gameplay row facts that follow a state's shapes.
const SHAPE_KEYS: [&str; 8] = [
    "full_collision",
    "can_occlude",
    "solid_render",
    "shape_occlusion",
    "skylight_down",
    "dampening",
    "outline",
    "outline_edges",
];

/// [`gameplay_row`] for a block that takes the shapes of the vanilla state
/// whose row is `donor`: what the donor's row leaves out stays out.
pub(super) fn gameplay_row_like(template: &J, donor: &J) -> Result<String> {
    let mut row = template.clone();
    row.remove("id");
    for key in SHAPE_KEYS {
        match donor.get(key) {
            Some(value) => row.set(key, value.clone()),
            None => row.remove(key),
        }
    }
    compact_row(row)
}

fn compact_row(mut row: J) -> Result<String> {
    row.set("item", J::Str("minecraft:air".into()));
    let cube_outline = full_box(row.get("outline"));
    if let J::Obj(fields) = &mut row {
        fields.retain(|(key, value)| {
            *value != J::Bool(false)
                && !(cube_outline && (key == "outline" || key == "outline_edges"))
        });
    }
    if let Some(J::Obj(sound)) = row.get_mut("sound") {
        sound.retain(|(key, value)| !((key == "volume" || key == "pitch") && number(value, 1.0)));
    }
    Ok(serde_json::to_string(&row)?)
}

/// `body` (a [`gameplay_row`]) with `id` first, as the readers check it.
pub(super) fn with_id(body: &str, id: u32) -> J {
    J::Raw(match body.strip_prefix('{') {
        Some(rest) if rest != "}" => format!("{{\"id\":{id},{rest}"),
        _ => format!("{{\"id\":{id}}}"),
    })
}

fn number(value: &J, expected: f64) -> bool {
    matches!(value, J::Num(n) if n.as_f64() == Some(expected))
}

/// One box from 0,0,0 to 1,1,1.
fn full_box(outline: Option<&J>) -> bool {
    let Some(J::Arr(boxes)) = outline else {
        return false;
    };
    matches!(boxes.as_slice(), [J::Arr(b)] if b.len() == 6
        && b.iter().zip([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).all(|(v, at)| number(v, at)))
}

/// The block's `model-states.json` entry: each property's values in the
/// order its states first show them, and the states themselves in id
/// order, so its own blockstate binds over exactly the server's states.
pub(super) fn model_states(b: &ServerBlock) -> J {
    let mut schema: Vec<(String, Vec<String>)> = Vec::new();
    let mut states = Vec::with_capacity(b.states.len());
    for text in &b.states {
        let properties = parse_properties(text);
        for (key, value) in &properties {
            let at = match schema.iter().position(|(k, _)| k == key) {
                Some(at) => at,
                None => {
                    schema.push((key.clone(), Vec::new()));
                    schema.len() - 1
                }
            };
            if !schema[at].1.contains(value) {
                schema[at].1.push(value.clone());
            }
        }
        states.push(J::Obj(
            properties
                .into_iter()
                .map(|(k, v)| (k, J::Str(v)))
                .collect(),
        ));
    }
    let schema = schema
        .into_iter()
        .map(|(key, values)| {
            let named = J::Arr(values.into_iter().map(J::Str).collect());
            (key, J::Obj(vec![("named".to_owned(), named)]))
        })
        .collect();
    J::Obj(vec![
        ("schema".to_owned(), J::Obj(schema)),
        ("states".to_owned(), J::Arr(states)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(text: &str) -> Vec<(String, String)> {
        parse_properties(text)
    }

    /// Vanilla stairs: ids 100.. in the order facing, half, shape.
    fn stairs() -> VanillaBlock {
        let mut states = Vec::new();
        for facing in ["north", "south"] {
            for half in ["bottom", "top"] {
                for shape in ["straight", "inner_left"] {
                    let id = 100 + states.len() as u32;
                    let text = format!("facing={facing},half={half},shape={shape}");
                    states.push((id, properties(&text)));
                }
            }
        }
        VanillaBlock {
            default: 100,
            states,
        }
    }

    fn mod_block(name: &str, states: &[&str]) -> ServerBlock {
        ServerBlock {
            name: name.to_owned(),
            states: states.iter().map(|s| (*s).to_owned()).collect(),
            first: 5000,
            cube: false,
            opaque: false,
            donors: Vec::new(),
        }
    }

    #[test]
    fn a_mod_stairs_state_takes_the_matching_vanilla_stairs_state() {
        let mut blocks = [mod_block(
            "bop:fir_stairs",
            &[
                "facing=south,half=top,shape=straight,waterlogged=false",
                "facing=north,half=bottom,shape=inner_left,waterlogged=true",
            ],
        )];
        pick_donors(&mut blocks, |name| {
            (name == "minecraft:oak_stairs").then(stairs)
        });
        // south/top/straight is the 7th state, north/bottom/inner_left the 2nd.
        assert_eq!(blocks[0].donors, [106, 101]);
        assert_eq!(blocks[0].donor(0), Some(106));
    }

    #[test]
    fn a_value_the_vanilla_block_lacks_keeps_the_rest_of_the_match() {
        let block = stairs();
        assert_eq!(
            block.closest(&properties("facing=south,half=top,shape=outer_left")),
            106
        );
        assert_eq!(block.closest(&properties("age=3")), 100);
    }

    #[test]
    fn cubes_and_blocks_without_a_lookalike_keep_no_donor() {
        let mut cube = mod_block("bop:fir_stairs", &["facing=north"]);
        cube.cube = true;
        let mut blocks = [cube, mod_block("handcrafted:oak_table", &[""])];
        pick_donors(&mut blocks, |_| Some(stairs()));
        assert!(blocks.iter().all(|b| b.donors.is_empty()));
    }
}
