//! Content mods in the resource root (docs/modpacks.md, registries 0.2).
//!
//! Every loader reads one resource root, and most of its block and item
//! tables are exports from vanilla indexed by block-state id or keyed by
//! name. A content mod's block copies a vanilla *template*. So the mods'
//! root is an overlay, like the resource-pack one:
//! - the base root linked, with the per-state and per-name tables
//!   rewritten: each mod block gets its template's rows appended (states
//!   after vanilla's, mods and blocks in id order) with names, translation
//!   keys and in-table state ids moved over, and the declared overrides
//!   applied;
//! - packaged models, textures and much of the mod's data served straight
//!   from its archive (`horizon_world::content_view`);
//! - additions to tags merged into the tag files and bundles.
//!
//! The loaders need no mod code, vanilla state ids never move, and with no
//! content mod the root is the base root itself. With a modded server's
//! state table, the server's blocks no mod declares follow (`mod_content_server.rs`).
use crate::{Result, blocks::MAP_COLORS};
use horizon_mods::content::{Block, Content, Rarity};
use horizon_world::content_view::{self, Index, Stored};
use horizon_world::nbt::{self, JavaString, Limits, Tag};
use serde::{Deserialize, Serialize, de};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt, fs,
    io::{BufWriter, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

mod decode;
mod reuse;
#[path = "mod_content_server.rs"]
mod server;
pub use reuse::installed_resource_root;

/// A content mod to put in the root: its id and content.
pub struct ContentMod {
    pub id: String,
    pub content: Content,
}

/// JSON with its object keys in file order, so a rewritten table differs
/// from the original only by what was added.
#[derive(Clone, Debug, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
    /// JSON text written as it is: rows added, never read back.
    Raw(String),
}
impl<'de> Deserialize<'de> for J {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = J;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> std::result::Result<J, E> {
                Ok(J::Null)
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<J, E> {
                Ok(J::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<J, E> {
                Ok(J::Num(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<J, E> {
                Ok(J::Num(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<J, E> {
                serde_json::Number::from_f64(v)
                    .map(J::Num)
                    .ok_or_else(|| E::custom("not a JSON number"))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<J, E> {
                Ok(J::Str(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> std::result::Result<J, E> {
                Ok(J::Str(v))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<J, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(J::Arr(out))
            }
            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<J, A::Error> {
                let mut out = Vec::new();
                while let Some(kv) = a.next_entry()? {
                    out.push(kv);
                }
                Ok(J::Obj(out))
            }
        }
        d.deserialize_any(V)
    }
}
impl Serialize for J {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            J::Null => s.serialize_unit(),
            J::Bool(b) => s.serialize_bool(*b),
            J::Num(n) => n.serialize(s),
            J::Str(t) => s.serialize_str(t),
            J::Arr(a) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            J::Obj(o) => {
                let mut map = s.serialize_map(Some(o.len()))?;
                for (k, v) in o {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
            J::Raw(text) => match serde_json::value::RawValue::from_string(text.clone()) {
                Ok(raw) => raw.serialize(s),
                Err(e) => Err(serde::ser::Error::custom(e)),
            },
        }
    }
}
impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn remove(&mut self, key: &str) {
        if let J::Obj(o) = self {
            o.retain(|(k, _)| k != key);
        }
    }
    fn get_mut(&mut self, key: &str) -> Option<&mut J> {
        match self {
            J::Obj(o) => o.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// Sets `key`, in place if present, else at the end.
    fn set(&mut self, key: &str, value: J) {
        if let Some(v) = self.get_mut(key) {
            *v = value;
        } else if let J::Obj(o) = self {
            o.push((key.to_owned(), value));
        }
    }
    /// The value at a `/`-separated path ("" is the document).
    fn at(&mut self, path: &str) -> Option<&mut J> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |j, key| j.get_mut(key))
    }
    fn f64(v: f64) -> J {
        serde_json::Number::from_f64(v).map_or(J::Null, J::Num)
    }
    fn as_u32(&self) -> Option<u32> {
        match self {
            J::Num(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
            _ => None,
        }
    }
    /// Renames strings and, with `ids`, moves state ids, everywhere inside.
    fn rename(&mut self, r: &Rename, ids: bool) {
        match self {
            J::Str(s) => {
                if let Some(new) = r.string(s) {
                    *s = new;
                }
            }
            J::Num(n) if ids => {
                if let Some(new) = n
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .and_then(|id| r.id(id))
                {
                    *n = new.into();
                }
            }
            J::Arr(a) => a.iter_mut().for_each(|v| v.rename(r, ids)),
            J::Obj(o) => o.iter_mut().for_each(|(_, v)| v.rename(r, ids)),
            _ => {}
        }
    }
}

/// One id moving to another: `minecraft:iron_block` → `ruby:ruby_block`,
/// with its state range for blocks.
#[derive(Clone)]
struct Rename {
    from: String,
    to: String,
    states: Range<u32>,
    first: u32,
}
impl Rename {
    fn new(from: &str, to: &str) -> Self {
        Rename {
            from: from.to_owned(),
            to: to.to_owned(),
            states: 0..0,
            first: 0,
        }
    }
    /// The id itself, `Block{id}`, and `block.<ns>.<path>` / `item.<ns>.<path>` keys.
    fn string(&self, s: &str) -> Option<String> {
        if s == self.from {
            return Some(self.to.clone());
        }
        if s.strip_prefix("Block{").and_then(|x| x.strip_suffix('}')) == Some(&self.from) {
            return Some(format!("Block{{{}}}", self.to));
        }
        let key = |id: &str| id.replacen(':', ".", 1).replace('/', ".");
        for kind in ["block", "item"] {
            if s.strip_prefix(kind).and_then(|x| x.strip_prefix('.')) == Some(&key(&self.from)) {
                return Some(format!("{kind}.{}", key(&self.to)));
            }
        }
        None
    }
    fn id(&self, id: u32) -> Option<u32> {
        self.states
            .contains(&id)
            .then(|| self.first + (id - self.states.start))
    }
}

/// A block being added: its declaration and where its states go.
struct NewBlock<'a> {
    decl: &'a Block,
    rename: Rename,
    /// The template's item, if it has one.
    item: Option<String>,
    /// False when its shipped model hides less than its template: no opaque
    /// cube on a full-cube template, or see-through on any (see
    /// [`keeps_template_occlusion`]). It must not occlude then.
    opaque_cube: bool,
    /// True when it sits on `minecraft:stone` but its model is a plant, rail
    /// or carpet: walked through. Any other shape keeps stone's collision
    /// (the server's leaves, ice, depots and paths are solid).
    open_collision: bool,
    /// The vanilla block its models are drawn from, whose shapes it takes
    /// (see [`server::ModAssets::shape_donor`]).
    shape_donor: Option<&'static str>,
    /// Per template state: the donor's state whose shapes it takes. Empty:
    /// the template's own.
    donors: Vec<u32>,
}
/// Whether a declared block may keep its template's occlusion. A template
/// that hides what is behind it as a full cube (stone, logs, furnaces,
/// pumpkins: often picked only for matching properties) passes that on only
/// to a model that does too, else wall torches and chains on it cull their
/// neighbours' faces into holes. Slabs and stairs keep their shapes unless
/// the model is see-through.
fn keeps_template_occlusion(
    assets: &mut server::ModAssets<'_>,
    id: &str,
    template: &str,
    on_stone: bool,
) -> bool {
    if on_stone || assets.opaque_cube(template) {
        assets.opaque_cube(id)
    } else {
        assets.render_opaque_block(id)
    }
}
/// An item being added (declared, or a block's).
struct NewItem {
    rename: Rename,
    max_stack_size: Option<u8>,
    rarity: Option<Rarity>,
    components: Vec<(String, J)>,
}

/// What a per-state table's rows are, for the server blocks' copies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rows {
    /// `facts.json`: the state's name and properties.
    Facts,
    /// `gameplay-facts.json`, compact.
    Gameplay,
    /// `block-behaviours.json`: sturdy faces and flags.
    Behaviours,
    /// `structure-blocks.json`'s `full_block`.
    FullBlock,
    Other,
}

struct Plan<'a> {
    blocks: Vec<NewBlock<'a>>,
    items: Vec<NewItem>,
    /// Vanilla's state count (the tables' length before).
    states: usize,
    /// Each vanilla block's default state.
    defaults: BTreeMap<String, u32>,
    /// The gameplay rows' sound objects, by block, for `sounds` overrides.
    sounds: BTreeMap<String, J>,
    /// The server's blocks no mod declares, after the declared ones.
    server: Vec<server::ServerBlock>,
    /// The state ids of [`server::TEMPLATE`] and [`server::OPEN`].
    template: u32,
    open: u32,
    /// Server blocks left out, for the log.
    warnings: Vec<String>,
}

/// An item's component patch on its entry; `known` holds every type a vanilla item has, so typos fail.
fn patch_components(
    entry: &mut J,
    patch: &[(String, J)],
    known: &BTreeSet<String>,
) -> std::result::Result<(), String> {
    for (key, value) in patch {
        let kind = key.strip_prefix('!').unwrap_or(key);
        if !known.contains(kind) {
            return Err(format!("{kind} is no component a vanilla item has"));
        }
        if kind == key {
            entry.set(key, value.clone());
        } else {
            entry.remove(kind);
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<J> {
    Ok(serde_json::from_slice(&fs::read(path)?).map_err(|e| format!("{}: {e}", path.display()))?)
}

impl<'a> Plan<'a> {
    /// The plan, with the blocks of the server's state table at
    /// `server_table` that no mod declares after the declared ones.
    fn new(base: &Path, mods: &'a [ContentMod], server_table: Option<&Path>) -> Result<Self> {
        let mut facts = read_json(&base.join("facts.json"))?;
        let names: Vec<String> = match facts.at("states") {
            Some(J::Arr(states)) => states
                .iter()
                .map(|s| match s {
                    J::Arr(row) => match row.first() {
                        Some(J::Str(name)) => Ok(name.clone()),
                        _ => Err("facts.json: a state without a name"),
                    },
                    _ => Err("facts.json: a state that is not a row"),
                })
                .collect::<std::result::Result<_, _>>()?,
            _ => return Err("facts.json: no states".into()),
        };
        let defaults: BTreeMap<String, u32> = match facts.at("defaults") {
            Some(J::Obj(o)) => o
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_u32()?)))
                .collect(),
            _ => return Err("facts.json: no defaults".into()),
        };
        let range = |name: &str| -> Option<Range<u32>> {
            let start = names.iter().position(|n| n == name)?;
            let len = names[start..].iter().take_while(|n| *n == name).count();
            // A block's states are one run in registry order.
            (names[start + len..].iter().all(|n| n != name))
                .then_some(start as u32..(start + len) as u32)
        };
        let mut blocks = Vec::new();
        let mut next = names.len() as u32;
        let mut sorted: Vec<&ContentMod> = mods.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let mut assets = server::ModAssets::new(base, mods);
        let mut wanted_sounds = Vec::new();
        for m in &sorted {
            let mut declared: Vec<&Block> = m.content.blocks.iter().collect();
            declared.sort_by(|a, b| a.id.cmp(&b.id));
            for decl in declared {
                if defaults.contains_key(&decl.id) {
                    return Err(format!("{}: {} is already a block", m.id, decl.id).into());
                }
                let states = range(&decl.template).ok_or_else(|| {
                    format!("{}: template {} is not a block", decl.id, decl.template)
                })?;
                let mut rename = Rename::new(&decl.template, &decl.id);
                rename.first = next;
                next += states.len() as u32;
                rename.states = states;
                if let Some(s) = &decl.properties.sounds {
                    if !defaults.contains_key(s) {
                        return Err(format!("{}: sounds {s} is not a block", decl.id).into());
                    }
                    wanted_sounds.push(s.clone());
                }
                // `minecraft:stone` is only the fallback template of a block with
                // no properties, whatever its shape: its own model tells.
                let on_stone = decl.template == server::TEMPLATE;
                let (opaque_cube, open_collision, shape_donor) = if assets.has_blockstate(&decl.id)
                {
                    (
                        keeps_template_occlusion(&mut assets, &decl.id, &decl.template, on_stone),
                        on_stone && assets.walk_through(&decl.id),
                        assets.shape_donor(&decl.id),
                    )
                } else {
                    (true, false, None)
                };
                blocks.push(NewBlock {
                    decl,
                    rename,
                    item: None,
                    opaque_cube,
                    open_collision,
                    shape_donor,
                    donors: Vec::new(),
                });
            }
        }
        if next > MAX_STATES {
            let mut by_mod: BTreeMap<&str, u32> = BTreeMap::new();
            for b in &blocks {
                *by_mod
                    .entry(&b.decl.id[..b.decl.id.find(':').unwrap_or(0)])
                    .or_default() += b.rename.states.len() as u32;
            }
            let list: Vec<String> = by_mod.iter().map(|(m, n)| format!("{m} {n}")).collect();
            return Err(format!(
                "{next} block states, more than the {MAX_STATES} a world can hold ({} in the game; \
                 the mods: {})",
                names.len(),
                list.join(", ")
            )
            .into());
        }
        // Vanilla's and the declared ids never move: the server's blocks come after.
        let (template, open) = (
            defaults.get(server::TEMPLATE).copied(),
            defaults.get(server::OPEN).copied(),
        );
        let (mut server, warnings) = match (server_table, template, open) {
            (Some(table), Some(_), Some(_)) => {
                let declared: BTreeSet<&str> = blocks.iter().map(|b| b.decl.id.as_str()).collect();
                let known = |name: &str| defaults.contains_key(name) || declared.contains(name);
                server::blocks_with_assets(&mut assets, table, known, next)?
            }
            _ => (Vec::new(), Vec::new()),
        };
        // A server block that is no cube takes the shapes of the vanilla block it looks like,
        // a declared block those of the vanilla block it is drawn like.
        if let Some(J::Arr(fact_rows)) = facts.at("states") {
            let vanilla = |name: &str| {
                let default = *defaults.get(name)?;
                let states = range(name)?
                    .map(|id| (id, server::state_properties(&fact_rows[id as usize])))
                    .collect();
                Some(server::VanillaBlock { default, states })
            };
            server::pick_donors(&mut server, &vanilla);
            let mut donor_blocks: BTreeMap<&str, Option<server::VanillaBlock>> = BTreeMap::new();
            for b in &mut blocks {
                let Some(name) = b.shape_donor else {
                    continue;
                };
                let Some(donor) = donor_blocks.entry(name).or_insert_with(|| vanilla(name)) else {
                    continue;
                };
                b.donors = b
                    .rename
                    .states
                    .clone()
                    .map(|id| donor.closest(&server::state_properties(&fact_rows[id as usize])))
                    .collect();
            }
        }
        // Block items copy the template block's item.
        let gameplay = read_json(&base.join("gameplay-facts.json"))?;
        let rows = match gameplay.get("states") {
            Some(J::Arr(rows)) if rows.len() == names.len() => rows,
            _ => return Err("gameplay-facts.json does not match facts.json".into()),
        };
        let mut sounds = BTreeMap::new();
        for s in wanted_sounds {
            let row = &rows[defaults[&s] as usize];
            sounds.insert(s, row.get("sound").cloned().unwrap_or(J::Null));
        }
        let mut items = Vec::new();
        for b in &mut blocks {
            let row = &rows[b.rename.states.start as usize];
            b.item = match row.get("item") {
                Some(J::Str(i)) if i != "minecraft:air" => Some(i.clone()),
                _ => None,
            };
            if b.decl.item {
                let item = b.item.as_deref().ok_or_else(|| {
                    format!("{}: template {} has no item", b.decl.id, b.decl.template)
                })?;
                items.push(NewItem {
                    rename: Rename::new(item, &b.decl.id),
                    max_stack_size: None,
                    rarity: None,
                    components: Vec::new(),
                });
            }
        }
        for m in &sorted {
            for i in &m.content.items {
                items.push(NewItem {
                    rename: Rename::new(&i.template, &i.id),
                    max_stack_size: i.max_stack_size,
                    rarity: i.rarity,
                    components: i
                        .components
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), serde_json::from_value(v.clone())?)))
                        .collect::<std::result::Result<_, serde_json::Error>>()
                        .map_err(|e| format!("{}: {e}", i.id))?,
                });
            }
        }
        // Block items and standalone items share the same registry. Sort
        // them together by mod ID and entry name before assigning IDs.
        items.sort_by(|a, b| a.rename.to.cmp(&b.rename.to));
        Ok(Plan {
            blocks,
            items,
            states: names.len(),
            defaults,
            sounds,
            server,
            template: template.unwrap_or(0),
            open: open.unwrap_or(0),
            warnings,
        })
    }

    /// What moves in a new block's copies of its template's rows: the
    /// block, and the template's item when it has another name. A block
    /// without an item gets `minecraft:air` where rows name one.
    fn renames(&self, b: &NewBlock) -> Vec<Rename> {
        let mut out = vec![b.rename.clone()];
        if let Some(item) = b.item.as_deref().filter(|i| *i != b.decl.template)
            && b.decl.item
        {
            out.push(Rename::new(item, &b.decl.id));
        }
        out
    }

    /// Appends each new block's copies of the template's states to `list`;
    /// gameplay rows get their index and the declared overrides. The server
    /// blocks' rows follow.
    fn states(&self, list: &mut Vec<J>, ids: bool, rows: Rows) -> Result<()> {
        let mut added = Vec::new();
        self.emit_states(list, ids, rows, |row| {
            added.push(row);
            Ok(())
        })?;
        list.append(&mut added);
        Ok(())
    }

    /// Templates and shape donors are vanilla state IDs, so the output
    /// never needs to look up an added row.
    fn emit_states(
        &self,
        vanilla: &[J],
        ids: bool,
        rows: Rows,
        mut emit: impl FnMut(J) -> Result<()>,
    ) -> Result<()> {
        if vanilla.len() != self.states {
            return Err(format!("{} states, not {}", vanilla.len(), self.states).into());
        }
        for b in &self.blocks {
            let renames = self.renames(b);
            for (i, id) in b.rename.states.clone().enumerate() {
                let mut row = vanilla[id as usize].clone();
                for r in &renames {
                    row.rename(r, ids);
                }
                let donor = b.donors.get(i).and_then(|&d| vanilla.get(d as usize));
                match (rows, donor) {
                    (Rows::Gameplay, _) => {
                        row.set("id", J::Num((b.rename.first + i as u32).into()));
                        if !b.decl.item {
                            row.set("item", J::Str("minecraft:air".into()));
                        }
                        if let Some(donor) = donor {
                            server::donor_gameplay(&mut row, donor);
                        } else if !b.opaque_cube {
                            server::clear_occlusion_gameplay(&mut row, b.open_collision);
                        }
                        self.override_gameplay(b, &mut row);
                    }
                    (Rows::Behaviours, Some(donor)) => server::donor_behaviour(&mut row, donor),
                    (Rows::Behaviours, None) if b.open_collision => {
                        server::open_behaviour(&mut row);
                    }
                    // `full_block` is the collision shape's (worldgen).
                    (Rows::FullBlock, Some(donor)) => row = donor.clone(),
                    (Rows::FullBlock, None) if b.open_collision => {
                        row = J::Bool(false);
                    }
                    _ => {}
                }
                emit(row)?;
            }
        }
        self.emit_server_states(vanilla, ids, rows, emit)
    }

    /// Appends each server block's rows: the template's per state, with its
    /// own id, name and properties as `rows` holds them.
    fn emit_server_states(
        &self,
        vanilla: &[J],
        ids: bool,
        rows: Rows,
        mut emit: impl FnMut(J) -> Result<()>,
    ) -> Result<()> {
        if self.server.is_empty() {
            return Ok(());
        }
        let template = vanilla
            .get(self.template as usize)
            .cloned()
            .ok_or("no server block template row")?;
        for b in &self.server {
            let gameplay = match rows {
                Rows::Gameplay => server::gameplay_row(&template, b.cube, b.opaque)?,
                _ => String::new(),
            };
            for (index, (id, text)) in b.ids().zip(&b.states).enumerate() {
                let donor = b.donor(index).and_then(|d| vanilla.get(d as usize));
                let row = match (rows, donor) {
                    (Rows::Gameplay, Some(donor)) => {
                        server::with_id(&server::gameplay_row_like(&template, donor)?, id)
                    }
                    (Rows::Gameplay, None) => server::with_id(&gameplay, id),
                    (Rows::FullBlock, Some(donor)) => donor.clone(),
                    (Rows::FullBlock, None) if !b.cube => J::Bool(false),
                    _ => {
                        let mut row = template.clone();
                        let mut rename = Rename::new(server::TEMPLATE, &b.name);
                        rename.states = self.template..self.template + 1;
                        rename.first = id;
                        row.rename(&rename, ids);
                        match rows {
                            Rows::Facts => server::set_state_properties(&mut row, &b.name, text),
                            Rows::Behaviours => match donor {
                                Some(donor) => server::donor_behaviour(&mut row, donor),
                                None if !b.cube => server::open_behaviour(&mut row),
                                None => {}
                            },
                            _ => {}
                        }
                        row
                    }
                };
                emit(row)?;
            }
        }
        Ok(())
    }

    /// The declared properties, on a gameplay row.
    fn override_gameplay(&self, b: &NewBlock, row: &mut J) {
        let p = &b.decl.properties;
        if let Some(v) = p.hardness {
            row.set("hardness", J::f64(v as f64));
        }
        if let Some(v) = p.explosion_resistance {
            row.set("explosion_resistance", J::f64(v as f64));
        }
        if let Some(v) = p.light {
            row.set("emission", J::Num(v.into()));
        }
        if let Some(v) = p.requires_tool {
            row.set("requires_tool", J::Bool(v));
        }
        if let Some(v) = p.friction {
            row.set("friction", J::f64(v as f64));
        }
        if let Some(v) = p.map_color {
            let col = if v == 0 { 0 } else { MAP_COLORS[v as usize] };
            row.set("map_color", J::Num(col.into()));
        }
        if let Some(s) = &p.sounds {
            row.set("sound", self.sounds[s].clone());
        }
    }

    /// Adds each new block's copy of the template's entry to a map by name.
    fn names(&self, map: &mut J, ids: bool) {
        for b in &self.blocks {
            if let Some(v) = map.get(&b.decl.template) {
                let mut v = v.clone();
                for r in &self.renames(b) {
                    v.rename(r, ids);
                }
                map.set(&b.decl.id, v);
            }
        }
    }

    /// Adds each server block's copy of the template's entry to a map by name.
    fn server_names(&self, map: &mut J) {
        let Some(entry) = map.get(server::TEMPLATE).cloned() else {
            return;
        };
        for b in &self.server {
            let mut v = entry.clone();
            v.rename(&Rename::new(server::TEMPLATE, &b.name), false);
            map.set(&b.name, v);
        }
    }

    /// Each server block's render definition: the template's over its own
    /// states. One that is no cube hides nothing behind it.
    fn server_render(&self, doc: &mut J) {
        let Some(entry) = doc.get(server::TEMPLATE).cloned() else {
            return;
        };
        for b in &self.server {
            let mut v = entry.clone();
            v.set("default", J::Num(b.first.into()));
            v.set(
                "states",
                J::Arr(b.ids().map(|id| J::Num(id.into())).collect()),
            );
            if !b.opaque {
                v.set("can_occlude", J::Bool(false));
            }
            doc.set(&b.name, v);
        }
    }

    /// Adds each new item's copy of its template's entry to a map by name;
    /// `components` applies the overrides (items.json).
    fn items(&self, map: &mut J, components: bool) -> Result<()> {
        let known: BTreeSet<String> = match map {
            J::Obj(entries) if components => entries
                .iter()
                .filter_map(|(_, v)| match v {
                    J::Obj(c) => Some(c.iter().map(|(k, _)| k.clone())),
                    _ => None,
                })
                .flatten()
                .collect(),
            _ => BTreeSet::new(),
        };
        for i in &self.items {
            let Some(v) = map.get(&i.rename.from) else {
                continue;
            };
            let mut v = v.clone();
            v.rename(&i.rename, false);
            if components {
                if let Some(n) = i.max_stack_size {
                    v.set("minecraft:max_stack_size", J::Num(n.into()));
                }
                if let Some(r) = i.rarity {
                    let r = match r {
                        Rarity::Common => "common",
                        Rarity::Uncommon => "uncommon",
                        Rarity::Rare => "rare",
                        Rarity::Epic => "epic",
                    };
                    v.set("minecraft:rarity", J::Str(r.into()));
                }
                patch_components(&mut v, &i.components, &known)
                    .map_err(|e| format!("{}: {e}", i.rename.to))?;
            }
            map.set(&i.rename.to, v);
        }
        Ok(())
    }

    /// One JSON table, rewritten, except the rows of a table [`streamed_rows`]
    /// names: [`write_states_table`] writes those.
    fn json(&self, name: &str, doc: &mut J) -> Result<()> {
        let states = |doc: &mut J, path: &str, ids: bool, rows: Rows| -> Result<()> {
            match doc.at(path) {
                Some(J::Arr(a)) => self.states(a, ids, rows),
                _ => Err(format!("no list at /{path}").into()),
            }
        };
        match name {
            // The rows of these three are written as they are made ([`write_states_table`]).
            "gameplay-facts.json" => {}
            "facts.json" => {
                let defaults = doc.at("defaults").ok_or("no defaults")?;
                for b in &self.blocks {
                    let default = self.defaults[&b.decl.template];
                    let id = b.rename.id(default).ok_or("a default outside its block")?;
                    defaults.set(&b.decl.id, J::Num(id.into()));
                }
                // The server does not say which state is a block's default: its first.
                for b in &self.server {
                    defaults.set(&b.name, J::Num(b.first.into()));
                }
            }
            "block-behaviours.json" => {
                let Some(J::Arr(blocks)) = doc.at("blocks") else {
                    return Err("no blocks".into());
                };
                for b in &self.blocks {
                    let template = blocks.iter().find(
                        |x| matches!(x.get("name"), Some(J::Str(n)) if *n == b.decl.template),
                    );
                    if let Some(mut v) = template.cloned() {
                        for r in &self.renames(b) {
                            v.rename(r, false);
                        }
                        if !b.decl.item {
                            v.set("item", J::Str("minecraft:air".into()));
                        }
                        blocks.push(v);
                    }
                }
                // One entry per block, in registry order, as the loaders check.
                if !self.server.is_empty() {
                    let template = blocks
                        .iter()
                        .find(|x| matches!(x.get("name"), Some(J::Str(n)) if n == server::TEMPLATE))
                        .cloned()
                        .ok_or("no server block template")?;
                    for b in &self.server {
                        let mut v = template.clone();
                        v.rename(&Rename::new(server::TEMPLATE, &b.name), false);
                        v.set("item", J::Str("minecraft:air".into()));
                        blocks.push(v);
                    }
                }
                self.items(doc.at("items").ok_or("no items")?, false)?;
                for odds in ["fire/burnOdds", "fire/igniteOdds"] {
                    if let Some(map) = doc.at(odds) {
                        self.names(map, false);
                        self.server_names(map);
                    }
                }
            }
            "model-states.json" => {
                self.names(doc, false);
                // The server's states, so each binds its own blockstate as there.
                for b in &self.server {
                    doc.set(&b.name, server::model_states(b));
                }
            }
            "landing-definitions.json" | "worldgen/blocks/survival-rules.json" => {
                self.names(doc, false);
                self.server_names(doc);
            }
            "render-definitions.json" => {
                self.names(doc, true);
                for b in &self.blocks {
                    if !b.opaque_cube
                        && let Some(v) = doc.get_mut(&b.decl.id)
                    {
                        v.set("can_occlude", J::Bool(false));
                    }
                }
                self.server_render(doc);
            }
            "worldgen/blocks/survival-extra-definitions.json" => {
                if let Some(map) = doc.at("ignition_odds") {
                    self.names(map, false);
                    self.server_names(map);
                }
            }
            "items.json" => self.items(doc, true)?,
            "registries.json" => {
                // Network ids: the game and its server map items and blocks through
                // this report, so new ones get the ids after vanilla's, in plan order.
                let new_items = self.items.iter().map(|i| i.rename.to.clone());
                let new_blocks = self
                    .blocks
                    .iter()
                    .map(|b| b.decl.id.clone())
                    .chain(self.server.iter().map(|b| b.name.clone()));
                for (registry, names) in [
                    ("minecraft:item", new_items.collect::<Vec<_>>()),
                    ("minecraft:block", new_blocks.collect()),
                ] {
                    let Some(J::Obj(entries)) =
                        doc.get_mut(registry).and_then(|r| r.get_mut("entries"))
                    else {
                        return Err(format!("no {registry} entries").into());
                    };
                    let next = entries
                        .iter()
                        .filter_map(|(_, e)| match e {
                            J::Obj(o) => o.iter().find(|(k, _)| k == "protocol_id")?.1.as_u32(),
                            _ => None,
                        })
                        .max()
                        .map_or(0, |m| m + 1);
                    for (i, name) in names.into_iter().enumerate() {
                        if entries.iter().any(|(k, _)| *k == name) {
                            continue;
                        }
                        let id = J::Num((next + i as u32).into());
                        entries.push((name, J::Obj(vec![("protocol_id".into(), id)])));
                    }
                }
            }
            "command-registries.json" => {
                let blocks = doc.at("blocks").ok_or("no blocks")?;
                self.names(blocks, false);
                self.server_names(blocks);
                let new_items = || self.items.iter().map(|i| J::Str(i.rename.to.clone()));
                let new_blocks = || {
                    self.blocks
                        .iter()
                        .map(|b| J::Str(b.decl.id.clone()))
                        .chain(self.server.iter().map(|b| J::Str(b.name.clone())))
                };
                let Some(J::Arr(items)) = doc.at("items") else {
                    return Err("no items".into());
                };
                items.extend(new_items());
                // The registries commands check ids against.
                if let Some(J::Arr(ids)) = doc.at("by_registry/minecraft:item/ids") {
                    ids.extend(new_items());
                }
                if let Some(J::Arr(ids)) = doc.at("by_registry/minecraft:block/ids") {
                    ids.extend(new_blocks());
                }
            }
            "mob-data.json" => {}
            "worldgen/block_flags.json" => states(doc, "", false, Rows::Other)?,
            "worldgen/blocks/structure-blocks.json" => {
                // A server block rotates and mirrors onto itself.
                for key in ["rotate", "mirror", "full_block"] {
                    let rows = match key {
                        "full_block" => Rows::FullBlock,
                        _ => Rows::Other,
                    };
                    states(doc, key, true, rows)?;
                }
                let added: usize = self.blocks.iter().map(|b| b.rename.states.len()).sum();
                let server: usize = self.server.iter().map(|b| b.states.len()).sum();
                doc.set(
                    "states",
                    J::Num(((self.states + added + server) as u64).into()),
                );
            }
            _ => return Err("no rule for this table".into()),
        }
        Ok(())
    }

    /// Each new block's state rows after the vanilla ones in `list`: the
    /// declared blocks' copies, then the server blocks' from the template.
    fn emit_nbt_states<E>(
        &self,
        list: &[Tag],
        mut emit: impl FnMut(Tag) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        for b in &self.blocks {
            for id in b.rename.states.clone() {
                let mut state = list[id as usize].clone();
                if let Some(slot) = state.get_mut("id") {
                    *slot = Tag::String(b.decl.id.as_str().into());
                }
                emit(state)?;
            }
        }
        if let Some(template) = list.get(self.template as usize) {
            for b in &self.server {
                for text in &b.states {
                    let mut state = template.clone();
                    server::set_nbt_state(&mut state, &b.name, text);
                    emit(state)?;
                }
            }
        }
        Ok(())
    }

    /// How many rows [`Self::emit_nbt_states`] gives for `list`.
    fn nbt_state_rows(&self, list: &[Tag]) -> usize {
        let declared: usize = self.blocks.iter().map(|b| b.rename.states.len()).sum();
        let server: usize = self.server.iter().map(|b| b.states.len()).sum();
        let has_template = list.get(self.template as usize).is_some();
        declared + if has_template { server } else { 0 }
    }

    /// Appends each new block's shapes to a `cached_shapes` array.
    fn extend_shapes(&self, table_name: &str, a: &mut Vec<i32>) {
        let is_occlusion = table_name.ends_with("occlusion.nbt");
        let is_collision = table_name.ends_with("cached-collision.nbt");
        let open_shape = a.get(self.open as usize).copied();
        for b in &self.blocks {
            let r = &b.rename.states;
            if !b.donors.is_empty() {
                let shapes: Vec<_> = b.donors.iter().map(|&d| a[d as usize]).collect();
                a.extend(shapes);
            } else if let Some(open_shape) = open_shape
                && ((is_occlusion && !b.opaque_cube) || (is_collision && b.open_collision))
            {
                a.extend(std::iter::repeat_n(open_shape, r.len()));
            } else {
                let copy = a[r.start as usize..r.end as usize].to_vec();
                a.extend(copy);
            }
        }
        // A block that is no cube takes the shapes of the vanilla
        // block it looks like, else air's: nothing to collide
        // with, stand on or hide behind.
        if !self.server.is_empty() {
            let (cube, open) = (a[self.template as usize], a[self.open as usize]);
            for b in &self.server {
                // A see-through cube collides but hides nothing.
                let solid = if is_occlusion { b.opaque } else { b.cube };
                let shapes: Vec<_> = (0..b.states.len())
                    .map(|i| match b.donor(i) {
                        Some(donor) => a[donor as usize],
                        None if solid => cube,
                        None => open,
                    })
                    .collect();
                a.extend(shapes);
            }
        }
    }

    /// Adds each declared block's copy of its template's entry to a `blocks` map.
    fn insert_declared_blocks(&self, map: &mut nbt::Compound) {
        for b in &self.blocks {
            let Some(mut v) = map
                .get(&JavaString::from(b.decl.template.as_str()))
                .cloned()
            else {
                continue;
            };
            if let (Some(f), Some(slot)) = (b.decl.properties.friction, v.get_mut("friction")) {
                *slot = Tag::Float(f.to_bits());
            }
            map.insert(b.decl.id.as_str().into(), v);
        }
    }

    /// The entry every server block has in a `blocks` map: the template's.
    fn server_block_entry(&self, map: &nbt::Compound) -> Option<Tag> {
        if self.server.is_empty() {
            return None;
        }
        map.get(&JavaString::from(server::TEMPLATE)).cloned()
    }

    /// One NBT table in memory: `states` lists, `cached_shapes` arrays and
    /// `blocks` compounds, wherever they are. The game writes tables with
    /// [`Self::write_nbt`] instead; this tree is what its bytes must equal.
    #[cfg(test)]
    fn nbt(&self, table_name: &str, tag: &mut Tag) {
        let Tag::Compound(c) = tag else {
            return;
        };
        for (key, value) in c.iter_mut() {
            match (key.to_string_lossy().as_str(), value) {
                ("states", Tag::List(list)) if list.len() == self.states => {
                    let mut added = Vec::new();
                    let _ = self.emit_nbt_states(list, |state| {
                        added.push(state);
                        Ok::<(), ()>(())
                    });
                    list.append(&mut added);
                }
                ("cached_shapes", Tag::IntArray(a)) if a.len() == self.states => {
                    self.extend_shapes(table_name, a);
                }
                ("blocks", Tag::Compound(map)) => {
                    self.insert_declared_blocks(map);
                    if let Some(v) = self.server_block_entry(map) {
                        for b in &self.server {
                            map.insert(b.name.as_str().into(), v.clone());
                        }
                    }
                }
                (_, inner @ Tag::Compound(_)) => self.nbt(table_name, inner),
                _ => {}
            }
        }
    }

    /// One NBT table, rewritten as it is written: the new blocks' state rows
    /// and server entries go straight to `out`, so they are never all in memory
    /// as a tree and again as the file's bytes. The bytes are those of
    /// [`Self::nbt`]'s tree.
    fn write_nbt(
        &self,
        table_name: &str,
        named: &mut nbt::NamedTag,
        out: &mut dyn Write,
    ) -> std::io::Result<()> {
        let mut writer = nbt::StreamWriter::new(out, Limits::tables());
        match &mut named.tag {
            Tag::Compound(root) => {
                writer.begin_root(&named.name)?;
                self.write_compound(table_name, root, 0, &mut writer)?;
            }
            other => {
                writer.entry(other.id(), &named.name)?;
                writer.payload(other, 0)?;
            }
        }
        writer.finish()
    }

    /// A compound's entries and its end, `depth` compounds below the root.
    fn write_compound(
        &self,
        table_name: &str,
        compound: &mut nbt::Compound,
        depth: usize,
        writer: &mut nbt::StreamWriter,
    ) -> std::io::Result<()> {
        for (key, value) in compound.iter_mut() {
            match (key.to_string_lossy().as_str(), value) {
                ("states", Tag::List(list)) if list.len() == self.states => {
                    self.write_states(key, list, depth, writer)?;
                }
                ("cached_shapes", Tag::IntArray(shapes)) if shapes.len() == self.states => {
                    self.extend_shapes(table_name, shapes);
                    let shapes = Tag::IntArray(std::mem::take(shapes));
                    writer.entry(shapes.id(), key)?;
                    writer.payload(&shapes, depth + 1)?;
                }
                ("blocks", Tag::Compound(map)) => {
                    self.write_blocks(key, map, depth, writer)?;
                }
                (_, Tag::Compound(inner)) => {
                    writer.entry(10, key)?;
                    self.write_compound(table_name, inner, depth + 1, writer)?;
                }
                (_, other) => {
                    writer.entry(other.id(), key)?;
                    writer.payload(other, depth + 1)?;
                }
            }
        }
        writer.end_compound()
    }

    /// A `states` list: the vanilla rows, then the new blocks' as they are made.
    fn write_states(
        &self,
        key: &JavaString,
        list: &[Tag],
        depth: usize,
        writer: &mut nbt::StreamWriter,
    ) -> std::io::Result<()> {
        let element_id = uniform_element_id(list).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a states list of mixed or wrapped elements",
            )
        })?;
        writer.entry(9, key)?;
        writer.list_header(Some(element_id), list.len() + self.nbt_state_rows(list))?;
        for state in list {
            writer.payload(state, depth + 2)?;
        }
        self.emit_nbt_states(list, |state| writer.payload(&state, depth + 2))
    }

    /// A `blocks` map with the declared blocks' entries added and the server
    /// blocks' merged in as they are written, in the map's key order.
    fn write_blocks(
        &self,
        key: &JavaString,
        map: &mut nbt::Compound,
        depth: usize,
        writer: &mut nbt::StreamWriter,
    ) -> std::io::Result<()> {
        self.insert_declared_blocks(map);
        let server_entry = self.server_block_entry(map);
        let mut server_names: Vec<&str> = match server_entry {
            Some(_) => self.server.iter().map(|b| b.name.as_str()).collect(),
            None => Vec::new(),
        };
        server_names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        server_names.dedup();
        writer.entry(10, key)?;
        let mut existing = map.iter().peekable();
        let mut added = server_names.into_iter().peekable();
        loop {
            // A server block replaces an entry of the same name, as an insert does.
            let take_added = match (existing.peek(), added.peek()) {
                (None, None) => break,
                (Some(_), None) => false,
                (None, Some(_)) => true,
                (Some((old, _)), Some(new)) => {
                    match old.0.iter().copied().cmp(new.encode_utf16()) {
                        std::cmp::Ordering::Less => false,
                        std::cmp::Ordering::Equal => {
                            existing.next();
                            true
                        }
                        std::cmp::Ordering::Greater => true,
                    }
                }
            };
            match (take_added, &server_entry, added.next_if(|_| take_added)) {
                (true, Some(entry), Some(name)) => {
                    writer.entry(entry.id(), &JavaString::from(name))?;
                    writer.payload(entry, depth + 2)?;
                }
                _ => {
                    if let Some((name, entry)) = existing.next() {
                        writer.entry(entry.id(), name)?;
                        writer.payload(entry, depth + 2)?;
                    }
                }
            }
        }
        writer.end_compound()
    }
}

/// The type of every element of `list`, if they share one and none is a
/// compound that the NBT writer would wrap, which is all a table's `states`
/// list can hold; a list of other shapes cannot be streamed.
fn uniform_element_id(list: &[Tag]) -> Option<u8> {
    let first = list.first().map_or(0, Tag::id);
    let wrapped = |tag: &Tag| matches!(tag, Tag::Compound(c) if c.len() == 1 && c.contains_key(&JavaString::default()));
    list.iter()
        .all(|tag| tag.id() == first && !wrapped(tag))
        .then_some(first)
}

/// The JSON and NBT tables rewritten for new blocks and items.
const JSON_TABLES: [&str; 14] = [
    "facts.json",
    "registries.json",
    "gameplay-facts.json",
    "block-behaviours.json",
    "model-states.json",
    "render-definitions.json",
    "landing-definitions.json",
    "items.json",
    "command-registries.json",
    "mob-data.json",
    "worldgen/block_flags.json",
    "worldgen/blocks/structure-blocks.json",
    "worldgen/blocks/survival-rules.json",
    "worldgen/blocks/survival-extra-definitions.json",
];
const NBT_TABLES: [&str; 6] = [
    "occlusion.nbt",
    "cached-collision.nbt",
    "movement-properties.nbt",
    "worldgen/blocks/block-properties.nbt",
    "worldgen/blocks/cached-support.nbt",
    "worldgen/blocks/cached-collision.nbt",
];
use horizon_items::content_namespaces::MARKER;
pub use horizon_items::content_namespaces::{namespaces, tag_namespaces};
/// Block states a world can hold. Chunk sections widen their global palette
/// past vanilla's 16 bits to fit the registry (crate::chunk::set_block_states);
/// this caps it at 20 bits, about 29 times vanilla's 35,723 states. Mods past
/// it are refused, not wrapped onto other blocks.
pub const MAX_STATES: u32 = 1 << 20;

/// Bundles of block tag files (tag id → the tag file's text).
const TAG_BUNDLES: [&str; 2] = ["block-tags.json", "worldgen/block_tags.json"];
/// The worldgen loader's bundle of biome tags.
const BIOME_TAGS: &str = "worldgen/biome_tags.json";
/// Worldgen registries a content mod may add to, and the bundle the
/// worldgen loader reads each from (id → the file's text): what structure
/// mods hold. Biomes, noise and density functions change the terrain and
/// are not read from mods.
const WORLDGEN_BUNDLES: [(&str, &str); 6] = [
    ("structure", "worldgen/structures.json"),
    ("structure_set", "worldgen/structure_sets.json"),
    ("template_pool", "worldgen/template_pools.json"),
    ("processor_list", "worldgen/processor_lists.json"),
    ("configured_feature", "worldgen/features.json"),
    ("placed_feature", "worldgen/placed_features.json"),
];

/// The registry of `command-registries.json` that each bundle's ids join,
/// for the commands that suggest them (`/locate`, `/place`).
const COMMAND_REGISTRIES: [(&str, &str); 6] = [
    ("worldgen/structures.json", "minecraft:worldgen/structure"),
    (
        "worldgen/structure_sets.json",
        "minecraft:worldgen/structure_set",
    ),
    (
        "worldgen/template_pools.json",
        "minecraft:worldgen/template_pool",
    ),
    (
        "worldgen/processor_lists.json",
        "minecraft:worldgen/processor_list",
    ),
    ("worldgen/features.json", "minecraft:worldgen/feature"),
    (
        "worldgen/placed_features.json",
        "minecraft:worldgen/placed_feature",
    ),
];

/// The bundle, namespace and id path for a worldgen registry file.
fn worldgen_entry(file: &str) -> Option<(&'static str, &str, &str)> {
    let (ns, rest) = file.strip_prefix("data/")?.split_once("/worldgen/")?;
    let (kind, path) = rest.split_once('/')?;
    let (_, bundle) = WORLDGEN_BUNDLES.iter().find(|(k, _)| *k == kind)?;
    Some((bundle, ns, path.strip_suffix(".json")?))
}

/// The namespace and path of a template also exposed under worldgen/structure.
fn structure_template(file: &str) -> Option<(&str, &str)> {
    file.strip_prefix("data/")?
        .split_once("/structure/")
        .filter(|(_, path)| path.ends_with(".nbt"))
}

/// The key of a mod's structure set or template pool file that adds its
/// entries to other sets or pools (vanilla's, another mod's) instead of
/// being one of its own. Vanilla data never has it.
const ADD_TO: &str = "horizon:add_to";
/// The bundles a mod may add to, and the list each entry goes into.
const ADDABLE: [(&str, &str); 2] = [
    ("worldgen/structure_sets.json", "structures"),
    ("worldgen/template_pools.json", "elements"),
];

/// A mod file's additions (`horizon:add_to`): its entries appended to the
/// list of each set or pool it names, as lithostitched's
/// `add_structure_set_entries` and `add_template_pool_elements` do for
/// NeoForge and Fabric packs.
struct Addition {
    targets: Vec<String>,
    list: &'static str,
    entries: Vec<J>,
}
impl Addition {
    /// What a mod file of `bundle` adds, if it is an addition file.
    fn read(bundle: &str, text: &str) -> Result<Option<Self>> {
        let Some((_, list)) = ADDABLE.iter().find(|(b, _)| *b == bundle) else {
            return Ok(None);
        };
        if !text.contains(ADD_TO) {
            return Ok(None);
        }
        let doc: J = serde_json::from_str(text)?;
        let Some(targets) = doc.get(ADD_TO) else {
            return Ok(None);
        };
        let targets = match targets {
            J::Str(s) => vec![s.clone()],
            J::Arr(a) => a
                .iter()
                .map(|v| match v {
                    J::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect::<Option<_>>()
                .ok_or(format!("{ADD_TO}: not a list of ids"))?,
            _ => return Err(format!("{ADD_TO}: not an id or a list of ids").into()),
        };
        let Some(J::Arr(entries)) = doc.get(list) else {
            return Err(format!("{ADD_TO} without a {list} list").into());
        };
        Ok(Some(Self {
            targets,
            list,
            entries: entries.clone(),
        }))
    }
    /// Appends the entries to each target in the bundle `doc` (id → the
    /// file's text). A target nobody defines is skipped, as a tag entry
    /// naming an unknown id is.
    fn apply(&self, doc: &mut J) -> Result<()> {
        for target in &self.targets {
            let Some(J::Str(text)) = doc.get(target) else {
                continue;
            };
            let mut v: J = serde_json::from_str(text).map_err(|e| format!("{target}: {e}"))?;
            let Some(J::Arr(list)) = v.get_mut(self.list) else {
                continue;
            };
            list.extend(self.entries.iter().cloned());
            doc.set(target, J::Str(serde_json::to_string(&v)?));
        }
        Ok(())
    }
}

/// The overlay being written: `out` mirrors `base` through links, and a
/// directory becomes real only when something inside it is written. The files
/// it serves from archives are never written: writing one stops serving it.
struct Overlay<'a> {
    base: &'a Path,
    out: &'a Path,
    dirs: Mutex<HashSet<PathBuf>>,
    served: Mutex<Index>,
}
impl Overlay<'_> {
    fn new<'a>(base: &'a Path, out: &'a Path) -> Result<Overlay<'a>> {
        // Its files are about to go: nothing reads them through the old index.
        content_view::unmount(out);
        if let Ok(meta) = fs::symlink_metadata(out) {
            let clean_started = std::time::Instant::now();
            if meta.is_dir() && !meta.file_type().is_symlink() {
                // Keep `.horizon-derived` intact across overlay rebuilds: every derived
                // table header validates its own stamp key and drops stale entries safely.
                let mut targets: Vec<(PathBuf, bool)> = Vec::new();
                let mut top_dirs: Vec<PathBuf> = Vec::new();
                for entry in fs::read_dir(out)?.flatten() {
                    if entry.file_name() == ".horizon-derived" {
                        continue;
                    }
                    let path = entry.path();
                    let kind = entry.file_type()?;
                    if kind.is_dir() && !kind.is_symlink() {
                        top_dirs.push(path.clone());
                        for sub in fs::read_dir(&path)?.flatten() {
                            let sub_kind = sub.file_type()?;
                            targets.push((sub.path(), sub_kind.is_dir() && !sub_kind.is_symlink()));
                        }
                    } else {
                        targets.push((path, false));
                    }
                }
                let threads = horizon_core::par::threads().max(1);
                let results =
                    horizon_core::par::par_map_with(threads, &targets, |(path, is_dir)| {
                        if *is_dir {
                            fs::remove_dir_all(path)
                        } else {
                            fs::remove_file(path)
                        }
                    });
                for res in results {
                    res?;
                }
                // `remove_dir_all` on the emptied folder: it copes with Windows'
                // delete-pending names that a plain `remove_dir` reports as not empty.
                for dir in top_dirs {
                    fs::remove_dir_all(dir)?;
                }
                // A rebuilt root is unvalidated until `installed_resource_root` blesses
                // it again: a stale blessing kept derived caches on the stamp-only path.
                for marker in ["installed-mods-v1", "installed-mods-reuse-v1"] {
                    if let Err(e) = fs::remove_file(out.join(".horizon-derived").join(marker))
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(e.into());
                    }
                }
            } else {
                fs::remove_file(out)?;
            }
            log::info!(
                "Horizon: stage=mod_content_overlay_clean elapsed_ms={}",
                clean_started.elapsed().as_millis()
            );
        }
        fs::create_dir_all(out)?;
        let overlay = Overlay {
            base,
            out,
            dirs: Mutex::new(HashSet::new()),
            served: Mutex::new(Index::default()),
        };
        let link_started = std::time::Instant::now();
        #[cfg(not(unix))]
        {
            let threads = horizon_core::par::threads().max(1);
            let (created_dirs, file_count) = link_tree(base, out, threads, link_file)?;
            let dir_count = created_dirs.len();
            *overlay.dirs.lock().unwrap_or_else(|e| e.into_inner()) = created_dirs;
            log::info!(
                "Horizon: stage=mod_content_overlay_link files={} dirs={} threads={} elapsed_ms={}",
                file_count,
                dir_count,
                threads,
                link_started.elapsed().as_millis()
            );
        }
        #[cfg(unix)]
        {
            overlay.link_children(Path::new(""))?;
            log::info!(
                "Horizon: stage=mod_content_overlay_link elapsed_ms={}",
                link_started.elapsed().as_millis()
            );
        }
        Ok(overlay)
    }
    fn link_children(&self, rel: &Path) -> Result<()> {
        let Ok(read) = fs::read_dir(self.base.join(rel)) else {
            return Ok(());
        };
        let is_root = rel.as_os_str().is_empty();
        let mut entries = Vec::new();
        for entry in read.flatten() {
            if is_root && entry.file_name() == ".horizon-derived" {
                continue;
            }
            entries.push(rel.join(entry.file_name()));
        }
        if is_root {
            let threads = horizon_core::par::threads().max(1);
            let results = horizon_core::par::par_map_with(threads, &entries, |child| {
                link(&self.base.join(child), &self.out.join(child))
            });
            for res in results {
                res?;
            }
        } else {
            for child in &entries {
                link(&self.base.join(child), &self.out.join(child))?;
            }
        }
        Ok(())
    }
    /// Makes `rel`'s directories real, each holding links to the base's.
    fn dir(&self, rel: &Path) -> Result<()> {
        if rel.as_os_str().is_empty() {
            return Ok(());
        }
        if self
            .dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(rel)
        {
            return Ok(());
        }
        let mut at = PathBuf::new();
        for part in rel.iter() {
            at.push(part);
            if self
                .dirs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&at)
            {
                continue;
            }
            let path = self.out.join(&at);
            let meta = fs::symlink_metadata(&path);
            if meta
                .as_ref()
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            {
                self.dirs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(at.clone());
                continue;
            }
            if meta.is_ok() {
                fs::remove_file(&path)?;
            }
            fs::create_dir(&path)?;
            self.link_children(&at)?;
            self.dirs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(at.clone());
        }
        Ok(())
    }
    fn served(&self) -> MutexGuard<'_, Index> {
        self.served.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Serves `rel` from its mod's archive instead of writing it.
    fn serve(&self, rel: &str, stored: Stored) {
        self.served().insert(rel, stored);
    }
    fn create(&self, rel: &str) -> Result<fs::File> {
        self.served().remove(rel);
        let rel = Path::new(rel);
        if let Some(parent) = rel.parent() {
            self.dir(parent)?;
        }
        let path = self.out.join(rel);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(fs::File::create(path)?)
    }
    fn write(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        self.create(rel)?.write_all(bytes)?;
        Ok(())
    }
    fn write_batch(
        &self,
        writes: &[(&str, impl AsRef<[u8]> + Sync)],
        threads: usize,
    ) -> Result<(u128, u128)> {
        if writes.is_empty() {
            return Ok((0, 0));
        }
        {
            let mut served = self.served();
            for (rel, _) in writes {
                served.remove(rel);
            }
        }
        let dirs_started = std::time::Instant::now();
        let mut parents = HashSet::new();
        for (rel, _) in writes {
            if let Some(parent) = Path::new(rel).parent()
                && !parent.as_os_str().is_empty()
                && parents.insert(parent)
            {
                self.dir(parent)?;
            }
        }
        let dirs_ms = dirs_started.elapsed().as_millis();
        let write_started = std::time::Instant::now();
        let write = |(rel, bytes): &(&str, _)| -> std::io::Result<()> {
            let path = self.out.join(rel);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            fs::write(path, bytes)
        };
        // Not the CPU pool's threads: creating a file waits on the file system
        // and the antivirus, so more at once finish sooner.
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..WRITE_THREADS.max(threads).min(writes.len()))
                .map(|_| {
                    scope.spawn(|| -> std::io::Result<()> {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(entry) = writes.get(i) else {
                                return Ok(());
                            };
                            write(entry)?;
                            crate::boot_progress::advance(1);
                        }
                    })
                })
                .collect();
            workers.into_iter().try_for_each(|w| {
                w.join()
                    .unwrap_or_else(|_| Err(std::io::Error::other("a writer thread panicked")))
            })
        })?;
        Ok((dirs_ms, write_started.elapsed().as_millis()))
    }
}
fn out_path(overlay: &Overlay<'_>, rel: &str) -> PathBuf {
    overlay.out.join(rel)
}
#[cfg(any(test, not(unix)))]
fn link_file(target: &Path, at: &Path) -> std::io::Result<()> {
    if fs::hard_link(target, at).is_ok() {
        return Ok(());
    }
    fs::copy(target, at)?;
    // Keep the source mtime: the sprite cache key is size + mtime.
    // Best effort: `fs::copy` already keeps the time, and a read-only
    // source's copy cannot be reopened for writing.
    let modified = fs::metadata(target)?.modified()?;
    if let Ok(file) = fs::File::options().write(true).open(at) {
        let _ = file.set_modified(modified);
    }
    Ok(())
}
/// Mirror `base` (except root `.horizon-derived`) into `out` by creating every
/// directory first and linking every file in parallel across `threads`.
#[cfg(any(test, not(unix)))]
fn link_tree(
    base: &Path,
    out: &Path,
    threads: usize,
    link_leaf: impl Fn(&Path, &Path) -> std::io::Result<()> + Sync,
) -> Result<(HashSet<PathBuf>, usize)> {
    let mut created_dirs = HashSet::new();
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let src_dir = base.join(&rel_dir);
        let entries = match fs::read_dir(&src_dir) {
            Ok(entries) => entries,
            Err(_) if rel_dir.as_os_str().is_empty() => return Ok((created_dirs, 0)),
            Err(e) => return Err(e.into()),
        };
        let is_root = rel_dir.as_os_str().is_empty();
        for entry in entries.flatten() {
            let name = entry.file_name();
            if is_root && name == ".horizon-derived" {
                continue;
            }
            let child_rel = rel_dir.join(&name);
            let src = entry.path();
            let is_dir = match entry.file_type() {
                Ok(t) if !t.is_symlink() => t.is_dir(),
                _ => src.is_dir(),
            };
            if is_dir {
                fs::create_dir_all(out.join(&child_rel))?;
                created_dirs.insert(child_rel.clone());
                stack.push(child_rel);
            } else {
                files.push(child_rel);
            }
        }
    }
    let file_count = files.len();
    let results = horizon_core::par::par_map_with(threads, &files, |rel| {
        link_leaf(&base.join(rel), &out.join(rel))
    });
    for res in results {
        res?;
    }
    Ok((created_dirs, file_count))
}
#[cfg(unix)]
fn link(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}
#[cfg(not(unix))]
fn link(target: &Path, at: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        fs::create_dir_all(at)?;
        for entry in fs::read_dir(target)?.flatten() {
            link(&entry.path(), &at.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        link_file(target, at)
    }
}

#[cfg(all(test, not(unix)))]
mod link_tests {
    #[test]
    fn copy_keeps_the_source_mtime() {
        let dir = std::env::temp_dir().join(format!("horizon-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("a.png"), dir.join("b.png"));
        std::fs::write(&src, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();
        super::link(&src, &dst).unwrap();
        assert_eq!(
            std::fs::metadata(&dst).unwrap().modified().unwrap(),
            std::fs::metadata(&src).unwrap().modified().unwrap()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// A tag file's JSON text with `more` appended to its values (no repeats).
fn merge_tag(text: Option<&str>, more: &[J], what: &str) -> Result<String> {
    let mut doc = match text {
        Some(t) => serde_json::from_str(t).map_err(|e| format!("{what}: {e}"))?,
        None => J::Obj(vec![("values".into(), J::Arr(Vec::new()))]),
    };
    let Some(J::Arr(values)) = doc.get_mut("values") else {
        return Err(format!("{what}: no values").into());
    };
    for v in more {
        if !values.contains(v) {
            values.push(v.clone());
        }
    }
    Ok(serde_json::to_string_pretty(&doc)?)
}

/// The block and item ids and tags a content root has, for tag entries.
struct Known {
    blocks: std::collections::BTreeSet<String>,
    items: std::collections::BTreeSet<String>,
    /// The worldgen loader's biomes and biome tags.
    biomes: std::collections::BTreeSet<String>,
    biome_tags: std::collections::BTreeSet<String>,
    /// `data/<ns>/tags/<kind>/<path>.json` files the mods add.
    tag_files: std::collections::BTreeSet<String>,
    /// Vanilla's block and item tags, by kind (their paths in the base's
    /// `data/minecraft/tags/<kind>/`); a kind whose folder the base lacks is
    /// absent.
    vanilla_tags: BTreeMap<&'static str, std::collections::BTreeSet<String>>,
}
impl Known {
    fn new(base: &Path, plan: &Plan, tags: &BTreeMap<String, TagMerge>) -> Result<Self> {
        let mut blocks: std::collections::BTreeSet<String> =
            plan.defaults.keys().cloned().collect();
        blocks.extend(plan.blocks.iter().map(|b| b.decl.id.clone()));
        // The mods' tags name their undeclared blocks too (mineable/axe ...).
        blocks.extend(plan.server.iter().map(|b| b.name.clone()));
        let mut items = std::collections::BTreeSet::new();
        if let J::Obj(o) = read_json(&base.join("items.json"))? {
            items.extend(o.into_iter().map(|(k, _)| k));
        }
        items.extend(plan.items.iter().map(|i| i.rename.to.clone()));
        let keys = |name: &str| -> Result<std::collections::BTreeSet<String>> {
            Ok(match base.join(name).exists() {
                true => match read_json(&base.join(name))? {
                    J::Obj(o) => o.into_iter().map(|(k, _)| k).collect(),
                    _ => Default::default(),
                },
                false => Default::default(),
            })
        };
        let mut vanilla_tags = BTreeMap::new();
        for kind in ["block", "item"] {
            let dir = base.join("data/minecraft/tags").join(kind);
            if dir.is_dir() {
                let mut paths = std::collections::BTreeSet::new();
                tag_paths(&dir, "", &mut paths)?;
                vanilla_tags.insert(kind, paths);
            }
        }
        Ok(Known {
            blocks,
            items,
            biomes: keys("worldgen/biomes.json")?,
            biome_tags: keys(BIOME_TAGS)?,
            tag_files: tags.keys().cloned().collect(),
            vanilla_tags,
        })
    }
    /// Whether tag `file` keeps entry `v`: a known id or an existing tag,
    /// for block, item and biome tags (other registries are kept as they
    /// are). The worldgen loader refuses a biome tag naming what it lacks
    /// (another mod's biomes, a loader's tags), so those entries go.
    fn keeps(&self, file: &str, v: &J) -> bool {
        let id = match v {
            J::Str(s) => s.as_str(),
            J::Obj(_) => match v.get("id") {
                Some(J::Str(s)) => s.as_str(),
                _ => return true,
            },
            _ => return true,
        };
        let Some(rest) = file
            .strip_prefix("data/")
            .and_then(|r| r.split_once("/tags/"))
            .map(|(_, rest)| rest)
        else {
            return true;
        };
        let kind = match rest.strip_prefix("worldgen/biome/") {
            Some(_) => "worldgen/biome",
            None => rest.split_once('/').map_or("", |(k, _)| k),
        };
        let ids = match kind {
            "block" => &self.blocks,
            "item" => &self.items,
            "worldgen/biome" => &self.biomes,
            _ => return true,
        };
        match id.strip_prefix('#') {
            Some(tag) => {
                let (tns, path) = tag.split_once(':').unwrap_or(("minecraft", tag));
                let rel = format!("data/{tns}/tags/{kind}/{path}.json");
                self.tag_files.contains(&rel)
                    || match kind {
                        "worldgen/biome" => self.biome_tags.contains(&format!("{tns}:{path}")),
                        // 1.21.1's tags 26.3 dropped or renamed (`concrete_powder`)
                        // are unknown too: one would leave the block tags unresolved.
                        _ => {
                            tns == "minecraft"
                                && self.vanilla_tags.get(kind).is_none_or(|t| t.contains(path))
                        }
                    }
            }
            None => ids.contains(id),
        }
    }
}

/// What the mods do to one tag file, in load order (a data pack's
/// TagLoader): each adds its values, and one that replaces the tag first
/// drops vanilla's entries and the mods' before it.
#[derive(Default)]
struct TagMerge {
    /// A mod replaced the tag: vanilla's entries are left out.
    replaced: bool,
    /// The mods whose values are in `values`.
    mods: Vec<String>,
    values: Vec<J>,
}

impl TagMerge {
    /// Adds one mod's tag file. A replace that drops entries (vanilla's in a
    /// `shared` tag, one not in the mod's namespaces, or other mods') warns.
    fn add(
        &mut self,
        id: &str,
        file: &str,
        replace: bool,
        shared: bool,
        values: &[J],
    ) -> Option<String> {
        let mut warning = None;
        if replace {
            let mut dropped: Vec<String> = Vec::new();
            if shared && !self.replaced {
                dropped.push("vanilla's entries".into());
            }
            if !self.mods.is_empty() {
                dropped.push(format!("those of {}", self.mods.join(", ")));
            }
            if !dropped.is_empty() {
                warning = Some(format!(
                    "mod {id} replaces {file}: {} are left out, as a data pack's replace \
                     does; the mods after it in load order still add to it",
                    dropped.join(" and ")
                ));
            }
            self.replaced = true;
            self.mods.clear();
            self.values.clear();
        }
        if !self.mods.iter().any(|m| m == id) {
            self.mods.push(id.to_owned());
        }
        self.values.extend(values.iter().cloned());
        warning
    }
}

/// The tag paths under `dir` (`mineable/pickaxe` for `mineable/pickaxe.json`).
fn tag_paths(dir: &Path, prefix: &str, out: &mut std::collections::BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            tag_paths(&entry.path(), &format!("{prefix}{name}/"), out)?;
        } else if let Some(stem) = name.strip_suffix(".json") {
            out.insert(format!("{prefix}{stem}"));
        }
    }
    Ok(())
}

/// Threads rewriting the tables at once. Each holds a whole table as a tree
/// (gameplay-facts.json's is over a gigabyte with ATM10's mods), so more
/// would mostly add memory.
const TABLE_THREADS: usize = 3;
/// Threads writing a batch of extracted mod files at once. Creating a file on
/// Windows waits on the file system and the antivirus, not the CPU: 17k real
/// mod files took 14.9 s on 6 threads, 10.4 s on 16 and 9.3 s on 24.
const WRITE_THREADS: usize = 16;
/// Flush merged tags in bounded batches, with room for one oversized tag.
const TAG_WRITE_BUDGET: usize = 8 << 20;
/// Free physical memory kept for the rest of the system (a browser, voice
/// chat) while the tables are rewritten.
const TABLE_MEMORY_RESERVE: u64 = 1 << 30;
/// Free physical memory one table thread may take at its peak. With less, fewer
/// threads run, down to one: three at once on a PC with a browser open went
/// from 3.2 GB free to under 600 MB within seconds.
const TABLE_THREAD_MEMORY: u64 = 3 << 29;

/// Table threads that fit `available` bytes of free memory, between one and
/// [`TABLE_THREADS`]; all of them when the system cannot tell.
fn table_threads(cores: usize, available: Option<u64>) -> usize {
    let fit = available.map_or(TABLE_THREADS, |free| {
        (free.saturating_sub(TABLE_MEMORY_RESERVE) / TABLE_THREAD_MEMORY) as usize
    });
    cores.min(TABLE_THREADS).min(fit).max(1)
}

/// `private_mb=<n> live_heap_mb=<n>` for a stage line: the process's committed
/// memory now, and the part of it that is live allocations.
fn memory_fields() -> String {
    let private = horizon_threads::process_memory()
        .and_then(|m| m.private_bytes)
        .map_or_else(|| "unknown".into(), |bytes| (bytes >> 20).to_string());
    format!(
        "private_mb={private} live_heap_mb={}",
        horizon_threads::live_heap() >> 20
    )
}

/// The per-state tables whose added rows are written as they are made, and
/// what their rows are.
fn streamed_rows(name: &str) -> Option<Rows> {
    match name {
        "facts.json" => Some(Rows::Facts),
        "gameplay-facts.json" => Some(Rows::Gameplay),
        "block-behaviours.json" => Some(Rows::Behaviours),
        "mob-data.json" => Some(Rows::Other),
        _ => None,
    }
}

/// Added rows can dwarf the base document after adding mod blocks. Keep
/// the vanilla rows for template lookups and write each added row immediately.
fn write_states_table(plan: &Plan<'_>, doc: &J, rows: Rows, mut out: impl Write) -> Result<()> {
    let J::Obj(fields) = doc else {
        return Err("no list at /states".into());
    };
    let Some(states_index) = fields.iter().position(|(key, _)| key == "states") else {
        return Err("no list at /states".into());
    };
    let J::Arr(vanilla) = &fields[states_index].1 else {
        return Err("no list at /states".into());
    };
    out.write_all(b"{")?;
    for (index, (key, value)) in fields.iter().enumerate() {
        if index != 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(&mut out, key)?;
        out.write_all(b":")?;
        if index != states_index {
            serde_json::to_writer(&mut out, value)?;
            continue;
        }

        out.write_all(b"[")?;
        let mut first = true;
        let mut write_row = |row: &J| -> Result<()> {
            if !first {
                out.write_all(b",")?;
            }
            first = false;
            serde_json::to_writer(&mut out, row)?;
            Ok(())
        };
        for row in vanilla {
            write_row(row)?;
        }
        plan.emit_states(vanilla, false, rows, |row| write_row(&row))?;
        out.write_all(b"]")?;
    }
    out.write_all(b"}")?;
    Ok(())
}

/// Rewrites the base's per-state tables for the mods' states into the
/// overlay. They are independent: [`TABLE_THREADS`] threads take them,
/// the largest first.
fn tables(base: &Path, plan: &Plan, overlay: &Overlay<'_>) -> Result<()> {
    let mut jobs: Vec<(&str, u64)> = JSON_TABLES
        .iter()
        .chain(&NBT_TABLES)
        .filter_map(|name| Some((*name, fs::metadata(base.join(name)).ok()?.len())))
        .collect();
    jobs.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    crate::boot_progress::begin(crate::boot_progress::Step::ModTables, jobs.len() as u64);
    // Their directories become real here, so the threads only write files.
    for (name, _) in &jobs {
        if let Some(parent) = Path::new(name).parent() {
            overlay.dir(parent)?;
        }
    }
    let table = |name: &str| -> Result<()> {
        let table_started = std::time::Instant::now();
        let path = base.join(name);
        if let Some(rows) = streamed_rows(name) {
            let mut doc = read_json(&path)?;
            plan.json(name, &mut doc)
                .map_err(|e| format!("{name}: {e}"))?;
            let mut out = BufWriter::new(overlay.create(name)?);
            write_states_table(plan, &doc, rows, &mut out).map_err(|e| format!("{name}: {e}"))?;
            out.flush()?;
        } else if JSON_TABLES.contains(&name) {
            let mut doc = read_json(&path)?;
            plan.json(name, &mut doc)
                .map_err(|e| format!("{name}: {e}"))?;
            overlay.write(name, &serde_json::to_vec(&doc)?)?;
        } else {
            let (mut named, _) = nbt::read_named(&fs::read(&path)?, Limits::tables())
                .map_err(|e| format!("{name}: {e}"))?;
            let mut out = overlay.create(name)?;
            plan.write_nbt(name, &mut named, &mut out)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        log::info!(
            "Horizon: stage=mod_content_table name={} elapsed_ms={} {}",
            name,
            table_started.elapsed().as_millis(),
            memory_fields()
        );
        crate::boot_progress::advance(1);
        Ok(())
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let available = horizon_threads::available_memory();
    let threads = table_threads(
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        available,
    );
    log::info!(
        "Horizon: stage=mod_content_tables_start threads={threads} available_mb={} {}",
        available.map_or_else(|| "unknown".into(), |bytes| (bytes >> 20).to_string()),
        memory_fields()
    );
    let finished = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| -> std::result::Result<(), String> {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((name, _)) = jobs.get(i) else {
                            return Ok(());
                        };
                        table(name).map_err(|e| e.to_string())?;
                    }
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .unwrap_or_else(|_| Err("a table thread panicked".into()))
        })
    });
    if let Some(peak) = horizon_threads::peak_memory() {
        log::info!(
            "Horizon: stage=mod_content_tables_peak working_set_mb={} private_mb={} heap_peak_mb={}",
            peak.working_set_bytes >> 20,
            peak.private_bytes >> 20,
            horizon_threads::heap_peak() >> 20
        );
    }
    finished?;
    Ok(())
}

/// Builds the content root for `mods` over `base` in `out`. Returns the
/// warnings for the log: the shared tags a mod replaced.
/// The folder a written mod file counts under: `data/<kind>` or
/// `assets/<kind>` without the namespace, else the path's first folder
/// (`worldgen` for the worldgen loader's structure templates).
fn write_folder(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [root @ ("data" | "assets"), _namespace, kind, _, ..] => format!("{root}/{kind}"),
        [top, _, ..] => (*top).to_owned(),
        _ => "(root)".to_owned(),
    }
}

/// `folder=files/MB` per folder, most files first.
fn describe_written_folders(folders: &BTreeMap<String, (usize, u64)>) -> String {
    let mut rows: Vec<_> = folders.iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(b.0)));
    rows.iter()
        .map(|(folder, (files, bytes))| {
            format!(
                "{folder}={files}/{:.1}MB",
                *bytes as f64 / (1024.0 * 1024.0)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn build(base: &Path, mods: &[ContentMod], out: &Path) -> Result<Vec<String>> {
    build_with_server(base, mods, None, out)
}

/// [`build`], adding the blocks of the server's state table at
/// `server_table` that no mod declares (`mod_content_server.rs`). The warnings also name
/// the server blocks left out.
pub fn build_with_server(
    base: &Path,
    mods: &[ContentMod],
    server_table: Option<&Path>,
    out: &Path,
) -> Result<Vec<String>> {
    // Two mods owning one namespace would mix their files.
    for (i, a) in mods.iter().enumerate() {
        for b in &mods[..i] {
            let (b, a) = (
                (b.id.as_str(), b.content.namespaces.as_slice()),
                (a.id.as_str(), a.content.namespaces.as_slice()),
            );
            if let Some(e) = horizon_mods::content::clash(b, a) {
                return Err(e.into());
            }
        }
    }
    // The overlay links to the base: a relative base would leave them dangling.
    let base = &std::path::absolute(base)?;
    use crate::boot_progress::{self, Step};
    boot_progress::begin(Step::ModList, 0);
    let plan_started = std::time::Instant::now();
    let plan = Plan::new(base, mods, server_table)?;
    log::info!(
        "Horizon: stage=mod_content_plan elapsed_ms={}",
        plan_started.elapsed().as_millis()
    );
    boot_progress::begin(Step::ModLinks, 0);
    let overlay = Overlay::new(base, out)?;
    let tables_started = std::time::Instant::now();
    tables(base, &plan, &overlay)?;
    log::info!(
        "Horizon: stage=mod_content_tables elapsed_ms={}",
        tables_started.elapsed().as_millis()
    );
    // Mod files, and every tag addition gathered per tag file.
    let mut tags: BTreeMap<String, TagMerge> = BTreeMap::new();
    let mut warnings = plan.warnings.clone();
    let mut worldgen: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    let mut additions: BTreeMap<&str, Vec<Addition>> = BTreeMap::new();
    let threads = horizon_core::par::threads().max(1);
    let batches_started = std::time::Instant::now();
    let listed = decode::list(mods, threads);
    let served = served_files(base, mods, &listed, &mut overlay.served())?;
    // A served file only takes up the decode budget if a bundle needs its bytes.
    let sizes: Vec<Vec<u64>> = listed
        .iter()
        .zip(&served)
        .zip(mods)
        .map(|((files, served), m)| {
            files
                .iter()
                .zip(served)
                .zip(&m.content.files)
                .map(|((file, served), name)| {
                    if decode::needs_bytes(name, *served) {
                        file.size
                    } else {
                        0
                    }
                })
                .collect()
        })
        .collect();
    drop(listed);
    let batches = decode::plan(&sizes, decode::BUDGET, threads);
    log::info!(
        "Horizon: stage=mod_content_decode_batches batches={} elapsed_ms={}",
        batches.len(),
        batches_started.elapsed().as_millis()
    );
    // Every file the archives don't serve gets written: the bar counts them.
    let to_write = served.iter().flatten().filter(|s| s.is_none()).count() as u64;
    boot_progress::begin(Step::ModFiles, to_write);
    let extract_started = std::time::Instant::now();
    let mut decode_ms = 0u128;
    let mut write_dirs_ms = 0u128;
    let mut write_files_ms = 0u128;
    let mut written_files = 0usize;
    let mut written_by_folder: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let batch_count = batches.len();
    // Bounded: a batch of decoded files at a time ([`decode::BUDGET`]), not
    // `threads` whole mods.
    for batch in batches {
        let decode_started = std::time::Instant::now();
        let decoded_ok: Vec<Vec<Vec<u8>>> =
            horizon_core::par::par_map_with(threads, &batch, |slice| {
                decode::read(mods, &served, slice)
            })
            .into_iter()
            .map(|res| res.map_err(|e| -> Box<dyn std::error::Error> { e.into() }))
            .collect::<Result<_>>()?;
        decode_ms += decode_started.elapsed().as_millis();
        let mut pending_order: Vec<String> = Vec::new();
        let mut pending_bytes: HashMap<String, &[u8]> = HashMap::new();
        for ((mod_index, indices), files) in batch.iter().zip(&decoded_ok) {
            let m = &mods[*mod_index];
            let declarations = format!("data/{}/horizon/", m.id);
            for (idx, bytes) in indices.clone().zip(files) {
                let file = &m.content.files[idx];
                let stored = served[*mod_index][idx];
                // data/<ns>/tags/<registry>/...: a mod's own data under another
                // folder named tags (Big Cannons' block_armor/tags) is not one.
                let is_tag = file.starts_with("data/") && file.split('/').nth(2) == Some("tags");
                if is_tag && file.ends_with(".json") {
                    let doc: J =
                        serde_json::from_slice(bytes).map_err(|e| format!("{file}: {e}"))?;
                    let Some(J::Arr(values)) = doc.get("values") else {
                        // One malformed tag skips that file, not every mod.
                        warnings.push(format!("{}: {file}: a tag without values, skipped", m.id));
                        continue;
                    };
                    let replace = doc.get("replace") == Some(&J::Bool(true));
                    let ns = file.split('/').nth(1).unwrap_or_default();
                    let shared = !m.content.namespaces.iter().any(|n| n == ns);
                    let merge = tags.entry(file.to_owned()).or_default();
                    warnings.extend(merge.add(&m.id, file, replace, shared, values));
                    continue;
                }
                if file.starts_with(&declarations) {
                    continue;
                }
                let mut add_file = |rel: &str| {
                    if let Some(stored) = stored {
                        // A later mod's file replaces an earlier one's, as on disk.
                        if pending_bytes.remove(rel).is_some() {
                            pending_order.retain(|pending| pending != rel);
                        }
                        overlay.serve(rel, stored);
                    } else if let Some(existing) = pending_bytes.get_mut(rel) {
                        *existing = bytes.as_slice();
                    } else {
                        pending_order.push(rel.to_owned());
                        pending_bytes.insert(rel.to_owned(), bytes.as_slice());
                    }
                };
                add_file(file);
                if let Some((bundle, ns, path)) = worldgen_entry(file) {
                    let text = std::str::from_utf8(bytes)
                        .map_err(|e| format!("{file}: {e}"))?
                        .to_owned();
                    let entries = worldgen.entry(bundle).or_default();
                    match Addition::read(bundle, &text).map_err(|e| format!("{file}: {e}"))? {
                        Some(addition) => additions.entry(bundle).or_default().push(addition),
                        None => entries.push((format!("{ns}:{path}"), text)),
                    }
                } else if let Some((ns, path)) = structure_template(file) {
                    add_file(&format!("worldgen/structure/{ns}/{path}"));
                }
            }
        }
        let writes: Vec<(&str, &[u8])> = pending_order
            .iter()
            .map(|rel| (rel.as_str(), pending_bytes[rel.as_str()]))
            .collect();
        written_files += writes.len();
        for (rel, bytes) in &writes {
            let group = written_by_folder.entry(write_folder(rel)).or_default();
            group.0 += 1;
            group.1 += bytes.len() as u64;
        }
        let (dirs_ms, files_ms) = overlay.write_batch(&writes, threads)?;
        write_dirs_ms += dirs_ms;
        write_files_ms += files_ms;
    }
    log::info!(
        "Horizon: stage=mod_content_extract batches={} files={} served={} decode_ms={} dirs_ms={} write_ms={} elapsed_ms={}",
        batch_count,
        written_files,
        overlay.served().len(),
        decode_ms,
        write_dirs_ms,
        write_files_ms,
        extract_started.elapsed().as_millis()
    );
    // The files written above, by folder: merged tags and the mods' block
    // declarations are not among them.
    log::info!(
        "Horizon: stage=mod_content_written_by_folder {}",
        describe_written_folders(&written_by_folder)
    );
    boot_progress::begin(Step::ModTags, tags.len() as u64);
    let tail_started = std::time::Instant::now();
    // A tag naming an id nobody registers fails as a whole in vanilla, and
    // carried-over mods name blocks left out or other mods' ids: such
    // entries are dropped instead, so the rest of the tag holds.
    let known = Known::new(base, &plan, &tags)?;
    for (file, merge) in &mut tags {
        merge.values.retain(|v| known.keeps(file, v));
    }
    let mut bundles: Vec<(&str, J)> = Vec::new();
    for name in TAG_BUNDLES {
        if base.join(name).exists() {
            bundles.push((name, read_json(&base.join(name))?));
        }
    }
    let mut tag_writes = Vec::new();
    let mut tag_bytes = 0;
    let mut tag_dirs_ms = 0;
    let mut tag_write_ms = 0;
    let mut flush_tags = |writes: &mut Vec<(&str, String)>| -> Result<()> {
        let (dirs_ms, write_ms) = overlay.write_batch(writes, threads)?;
        tag_dirs_ms += dirs_ms;
        tag_write_ms += write_ms;
        writes.clear();
        Ok(())
    };
    for (file, merge) in &tags {
        let vanilla = if merge.replaced {
            None
        } else {
            fs::read_to_string(base.join(file)).ok()
        };
        let merged = merge_tag(vanilla.as_deref(), &merge.values, file)?;
        tag_bytes += merged.len();
        tag_writes.push((file.as_str(), merged));
        if tag_bytes >= TAG_WRITE_BUDGET {
            flush_tags(&mut tag_writes)?;
            tag_bytes = 0;
        }
        // data/<ns>/tags/block/<path>.json is the block tag <ns>:<path>.
        let Some((ns, path)) = file
            .strip_prefix("data/")
            .and_then(|r| r.split_once("/tags/block/"))
            .and_then(|(ns, p)| Some((ns, p.strip_suffix(".json")?)))
        else {
            continue;
        };
        let id = format!("{ns}:{path}");
        for (name, doc) in &mut bundles {
            let text = match doc.get(&id) {
                Some(J::Str(t)) if !merge.replaced => Some(t.clone()),
                _ => None,
            };
            let merged = merge_tag(text.as_deref(), &merge.values, name)?;
            doc.set(&id, J::Str(merged));
        }
    }
    flush_tags(&mut tag_writes)?;
    for (name, doc) in &bundles {
        overlay.write(name, &serde_json::to_vec(doc)?)?;
    }
    // Biome tags, merged into the worldgen loader's bundle as block tags are.
    let biome_tags: Vec<(String, &TagMerge)> = tags
        .iter()
        .filter_map(|(file, merge)| {
            let (ns, path) = file
                .strip_prefix("data/")?
                .split_once("/tags/worldgen/biome/")?;
            Some((format!("{ns}:{}", path.strip_suffix(".json")?), merge))
        })
        .collect();
    if !biome_tags.is_empty() && base.join(BIOME_TAGS).exists() {
        let mut doc = read_json(&base.join(BIOME_TAGS))?;
        for (id, merge) in biome_tags {
            let text = match doc.get(&id) {
                Some(J::Str(t)) if !merge.replaced => Some(t.clone()),
                _ => None,
            };
            let merged = merge_tag(text.as_deref(), &merge.values, BIOME_TAGS)?;
            doc.set(&id, J::Str(merged));
        }
        overlay.write(BIOME_TAGS, &serde_json::to_vec(&doc)?)?;
    }
    // Structures and the pools, sets, processors and features they use, then
    // what mods add to sets and pools, in mod and file order.
    for (bundle, entries) in &worldgen {
        if !base.join(bundle).exists() {
            continue;
        }
        let mut doc = read_json(&base.join(bundle))?;
        for (id, text) in entries {
            doc.set(id, J::Str(text.clone()));
        }
        for addition in additions.get(bundle).into_iter().flatten() {
            addition
                .apply(&mut doc)
                .map_err(|e| format!("{bundle}: {e}"))?;
        }
        overlay.write(bundle, &serde_json::to_vec(&doc)?)?;
    }
    // The commands suggest ids from `command-registries.json`: add the
    // structures, pools, sets and features the mods bring.
    let registries = out_path(&overlay, "command-registries.json");
    if let Ok(mut doc) = read_json(&registries) {
        // Registry names hold `/`, so each key is looked up by itself.
        let mut add = |keys: &[&str], new: &[J]| {
            let found = keys.iter().try_fold(&mut doc, |j, k| j.get_mut(k));
            if let Some(J::Arr(ids)) = found {
                for id in new {
                    if !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
            }
        };
        for (bundle, registry) in COMMAND_REGISTRIES {
            let new: Vec<J> = worldgen
                .get(bundle)
                .into_iter()
                .flatten()
                .map(|(id, _)| J::Str(id.clone()))
                .collect();
            add(&["by_registry", registry, "ids"], &new);
        }
        overlay.write("command-registries.json", &serde_json::to_vec(&doc)?)?;
    }
    // A block without its own blockstate looks like its template, so every
    // state still has a model.
    for m in mods {
        for block in &m.content.blocks {
            let (ns, path) = block.id.split_once(':').unwrap_or((&m.id, &block.id));
            let own = format!("assets/{ns}/blockstates/{path}.json");
            if m.content.files.contains(&own) {
                continue;
            }
            let template = block.template.trim_start_matches("minecraft:");
            let vanilla = base.join(format!("assets/minecraft/blockstates/{template}.json"));
            overlay.write(&own, &fs::read(&vanilla)?)?;
        }
    }
    let mut namespaces: Vec<&str> = mods
        .iter()
        .flat_map(|m| m.content.namespaces.iter().map(String::as_str))
        .collect();
    namespaces.sort();
    overlay.write(MARKER, &serde_json::to_vec(&namespaces)?)?;
    if mods.iter().any(|m| !m.content.blocks.is_empty()) || !plan.server.is_empty() {
        overlay.write(crate::content_identity::MARKER, b"1")?;
    }
    overlay.write(WARNINGS, warnings.join("\n").as_bytes())?;
    log::info!(
        "Horizon: stage=mod_content_tags_tail tags={} elapsed_ms={} dirs_ms={} write_ms={}",
        tags.len(),
        tail_started.elapsed().as_millis(),
        tag_dirs_ms,
        tag_write_ms
    );
    let served = std::mem::take(&mut *overlay.served());
    served.save(out)?;
    served.mount(out, &archives(mods))?;
    Ok(warnings)
}

/// Keep packaged files in the archive when their loaders use `content_view`.
/// This includes texture metadata alongside the images it describes.
fn served_from_archive(file: &str) -> bool {
    let mut parts = file.split('/');
    let path = (parts.next(), parts.next(), parts.next(), parts.next());
    if matches!(path, (Some("assets"), Some(_), Some("sounds"), Some(_))) {
        return file.ends_with(".ogg");
    }
    matches!(
        path,
        (
            Some("assets"),
            Some(_),
            Some(
                "models"
                    | "blockstates"
                    | "items"
                    | "textures"
                    | "lang"
                    | "block_hit_effects"
                    | "projectile_hit_effects"
            ),
            Some(_)
        ) | (
            Some("data"),
            Some(_),
            Some("recipe" | "advancement" | "loot_table" | "worldgen" | "structure" | "function"),
            Some(_)
        )
    )
}

/// What the root serves of each mod's files (`listed[mod][file]`), each with
/// its archive's number in `index`. Folder mods are edited in place
/// (`--dev-mod`) and stay copies, and so does a file the base has too (a
/// resource pack's in a mod's namespace): the mod's copy overwrites the
/// base's on disk, as before, for every reader.
fn served_files(
    base: &Path,
    mods: &[ContentMod],
    listed: &[Vec<decode::Listed>],
    index: &mut Index,
) -> Result<Vec<Vec<Option<Stored>>>> {
    let mut base_namespaces: HashMap<String, bool> = HashMap::new();
    let mut base_templates: HashMap<&str, bool> = HashMap::new();
    let template_root = base.join("worldgen/structure");
    let mut served = Vec::with_capacity(mods.len());
    for (m, files) in mods.iter().zip(listed) {
        let servable =
            |file: &decode::Listed, name: &str| file.stored.filter(|_| served_from_archive(name));
        let names = &m.content.files;
        if !files
            .iter()
            .zip(names)
            .any(|(f, n)| servable(f, n.as_str()).is_some())
        {
            served.push(vec![None; files.len()]);
            continue;
        }
        let archive = index.add_archive(m.content.source())?;
        let mut of_mod = Vec::with_capacity(files.len());
        for (file, name) in files.iter().zip(names) {
            let stored = servable(file, name.as_str()).filter(|_| {
                // `assets/<ns>` or `data/<ns>`: vanilla's base has no mod's.
                let namespace = name
                    .match_indices('/')
                    .nth(1)
                    .map_or(name.as_str(), |(at, _)| &name[..at]);
                let in_base = *base_namespaces
                    .entry(namespace.to_owned())
                    .or_insert_with_key(|ns| base.join(ns).is_dir());
                if in_base && base.join(name).exists() {
                    return false;
                }
                // Keep a template on disk when its generated path replaces a base file.
                !structure_template(name).is_some_and(|(ns, path)| {
                    let in_base = *base_templates
                        .entry(ns)
                        .or_insert_with(|| template_root.join(ns).is_dir());
                    in_base && template_root.join(ns).join(path).exists()
                })
            });
            of_mod.push(stored.map(|stored| Stored { archive, ..stored }));
        }
        served.push(of_mod);
    }
    Ok(served)
}

/// Where the mods' files are read from: each mod's `.hmod` or folder.
fn archives(mods: &[ContentMod]) -> Vec<PathBuf> {
    mods.iter().map(|m| m.content.source().to_owned()).collect()
}

/// Serves `out`'s packaged files again, from the `archives` its index names.
fn mount_saved(out: &Path, archives: &[PathBuf]) -> Result<()> {
    let started = std::time::Instant::now();
    let index = Index::load(out)?;
    let files = index.len();
    index.mount(out, archives)?;
    log::info!(
        "Horizon: stage=mod_content_view files={files} elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(())
}

/// The root to load: `base` itself with no content, else the mods' overlay
/// over it, built in `cache/mod-content`.
///
/// The root is built once and kept: later starts with the same build, base
/// and mod files reuse it. With ATM10's carried mods a build takes about
/// 25 s and peaks above 3 GB, more than everything the game keeps.
pub fn resource_root(base: &Path, mods: &[ContentMod], cache: &Path) -> Result<PathBuf> {
    if mods.iter().all(|m| m.content.is_empty()) {
        return Ok(base.to_owned());
    }
    let out = cache.join("mod-content");
    let server_table = server::table();
    let stamp_started = std::time::Instant::now();
    let stamp = stamp(base, mods, server_table.as_deref())?;
    log::info!(
        "Horizon: stage=mod_content_stamp elapsed_ms={}",
        stamp_started.elapsed().as_millis()
    );
    if root_has_base(&out) && fs::read(out.join(STAMP)).is_ok_and(|s| s == stamp.as_bytes()) {
        match mount_saved(&out, &archives(mods)) {
            Ok(()) => {
                // A reused root warns as the build did.
                let warnings = fs::read_to_string(out.join(WARNINGS)).unwrap_or_default();
                warnings.lines().for_each(|w| log::warn!("Horizon: {w}"));
                return Ok(out);
            }
            Err(e) => log::warn!("Horizon: mod content root rebuilt: {e}"),
        }
    }
    let started = std::time::Instant::now();
    for warning in build_with_server(base, mods, server_table.as_deref(), &out)? {
        log::warn!("Horizon: {warning}");
    }
    log::info!(
        "Horizon: mod content root built in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    let pending = out.join(format!("{STAMP}.{}.tmp", std::process::id()));
    fs::write(&pending, stamp)?;
    fs::rename(&pending, out.join(STAMP))?;
    Ok(out)
}

/// In a root built by [`build`]: its warnings, one per line.
const WARNINGS: &str = "horizon-mod-content-warnings.txt";

/// In a root built by [`resource_root`]: what it was built from.
const STAMP: &str = "horizon-mod-content.stamp";

/// Whether `out` holds its base's files, not only a matching stamp: a copied cache (the
/// friends launcher seeds one without Minecraft's own files) keeps the stamp but would load
/// without the base, and the game would then fall back to vanilla and drop every mod.
fn root_has_base(out: &Path) -> bool {
    out.join("horizon-resources.json").is_file()
}

/// Folder `packs::resource_root` keeps one overlay per resource-pack selection in.
const PACK_SELECTIONS: &str = "resourcepacks";

/// Which resource-pack selection `base` is, portably: `selection <hash>` for
/// a `packs::resource_root` overlay (`.horizon-cache/resourcepacks/<fnv of
/// the pack ids>`), else `vanilla`. Those overlays' tables are vanilla's
/// bytes, so without it a root built over one selection was reused for
/// another and its textures never showed. Never the path: prebuilt roots
/// move between installs.
fn base_identity(base: &Path) -> String {
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_owned());
    let selection = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| n.len() == 16 && n.bytes().all(|b| b.is_ascii_hexdigit()));
    let in_selections = base
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == PACK_SELECTIONS);
    match selection {
        Some(selection) if in_selections => format!("selection {selection}"),
        _ => "vanilla".to_owned(),
    }
}

/// What a content root is built from: this build of the game, the base's
/// manifest, its pack selection ([`base_identity`]) and the tables [`build`]
/// rewrites, every file of every mod in
/// order, and the server's state table when there is one. Editable directory mods
/// count by size and modification time; packaged archives, prepared base tables,
/// and the server state table count by portable logical name and SHA-256 bytes so
/// derived caches stay valid across directories and machines.
fn stamp(base: &Path, mods: &[ContentMod], server_table: Option<&Path>) -> Result<String> {
    use sha2::{Digest, Sha256};
    let meta = |path: &Path| -> Result<String> {
        let m = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let modified = m
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(format!(
            "{} {} {}",
            path.display(),
            m.len(),
            modified.as_nanos()
        ))
    };
    let content_digest = |label: &str, path: &Path| -> Result<String> {
        use std::io::Read;
        let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        let mut total = 0u64;
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
            total = total.saturating_add(read as u64);
        }
        Ok(format!("{label} {total} {:x}", hasher.finalize()))
    };
    let mut parts = vec!["horizon mod content 1".to_owned()];
    // The builder's own source, not the executable: every rebuild of the
    // game changed the exe's stamp and re-extracted every mod (minutes) on
    // the next launch, though only this file decides what a root holds.
    parts.push(format!(
        "builder {:x}",
        Sha256::digest(include_str!("mod_content.rs").as_bytes())
    ));
    parts.push(format!(
        "server blocks {:x}",
        Sha256::digest(include_str!("mod_content_server.rs").as_bytes())
    ));
    let base = std::path::absolute(base)?;
    parts.push(
        String::from_utf8_lossy(&fs::read(base.join("horizon-resources.json"))?).into_owned(),
    );
    parts.push(format!("base {}", base_identity(&base)));
    for name in JSON_TABLES.iter().chain(&NBT_TABLES).chain(&TAG_BUNDLES) {
        let path = base.join(name);
        if path.exists() {
            parts.push(content_digest(&format!("base {name}"), &path)?);
        }
    }
    for m in mods {
        parts.push(m.id.clone());
        let source = m.content.source();
        if source.is_dir() {
            let manifest = source.join(horizon_mods::manifest::MANIFEST);
            if manifest.exists() {
                parts.push(meta(&manifest)?);
            }
            for file in &m.content.files {
                parts.push(meta(&source.join(file))?);
            }
        } else {
            let name = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            parts.push(content_digest(&format!("mod {name}"), source)?);
        }
    }
    // Without it a root built for another server's blocks would be reused.
    if let Some(table) = server_table.filter(|t| t.is_file()) {
        parts.push(content_digest("server states", table)?);
    }
    let mut hasher = Sha256::new();
    for part in &parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
#[path = "mod_content_tests.rs"]
mod tests;
