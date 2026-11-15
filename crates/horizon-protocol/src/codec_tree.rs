//! The network codecs of a modded 1.21.1 server's own item components, entity
//! data serializers and particle types, as the small trees the dev oracle
//! exports next to the state ids (`*.codecs.tsv`, mods/horizon/src/neoforge-oracle
//! CodecTrees.java: `registry \t id \t name \t json`). The 767 translation can't
//! rewrite a mod's value, but with its tree it reads past it and leaves it out
//! instead of losing the rest of the stack or entity data list. Without the
//! table nothing changes.
use crate::{Error, Result, buf::Reader};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

pub const COMPONENTS: &str = "minecraft:data_component_type";
/// Keyed by the `neoforge:entity_data_serializers` id (the wire id less 256).
pub const SERIALIZERS: &str = "neoforge:entity_data_serializers";
pub const PARTICLES: &str = "minecraft:particle_type";

/// Nodes below one another a read may pass through (a recursive codec's
/// value nested this deep is refused).
const MAX_NESTING: u32 = 256;

/// A StreamCodec as the oracle describes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    /// `StreamCodec.unit`: no bytes.
    Unit,
    Bool,
    Byte,
    /// SHORT, UNSIGNED_SHORT.
    Short,
    Int,
    /// A big-endian long (BlockPos, ChunkPos).
    Long,
    Float,
    Double,
    /// VAR_INT, an idMapper, an enum, a registry or holderRegistry id.
    VarInt,
    VarLong,
    /// A VarInt byte length and UTF-8 (STRING_UTF8, ResourceLocation, a registry key).
    String,
    /// A VarInt length and the bytes.
    Bytes,
    /// A network tag (TAG, COMPOUND_TAG, fromCodec, fromCodecWithRegistries, a text component).
    Nbt,
    Uuid,
    /// VECTOR3F: three floats.
    Vec3f,
    /// QUATERNIONF: four floats.
    Quaternion,
    /// GAME_PROFILE_PROPERTIES: name, value and an optional signature each.
    Properties,
    /// GAME_PROFILE: id, name, properties.
    GameProfile,
    /// ItemStack.STREAM_CODEC or OPTIONAL_STREAM_CODEC (the 1.21.1 stack reader).
    Item,
    /// DataComponentPatch.STREAM_CODEC.
    Patch,
    /// NeoForge's FluidStack: an amount, then (when positive) the fluid id and a patch.
    Fluid,
    /// ByteBufCodecs.holderSet (NeoForge's): 0 and a tag, or the size plus one and the ids.
    HolderSet,
    /// StreamCodec.composite: the fields in order.
    Seq(Vec<Tree>),
    /// ByteBufCodecs.optional: a flag, then the value.
    Optional(Box<Tree>),
    /// ByteBufCodecs.collection / list: a VarInt count, then the elements.
    List(Box<Tree>),
    /// ByteBufCodecs.map: a VarInt count, then each key and value.
    Map(Box<Tree>, Box<Tree>),
    /// ByteBufCodecs.either: `true` and the left value, or `false` and the right.
    Either(Box<Tree>, Box<Tree>),
    /// ByteBufCodecs.holder: a reference's id plus one, or 0 and the value itself.
    Holder(Box<Tree>),
    /// A registry id selecting the codec of what follows (`dispatch`), by id.
    Dispatch(HashMap<i32, Tree>),
    /// A codec that contains itself; [`Tree::Recurse`] with its number reads it again.
    Rec(u32, Box<Tree>),
    Recurse(u32),
    /// What the oracle couldn't describe (a mod's own StreamCodec class).
    Unknown(String),
}

impl Tree {
    /// A tree from the oracle's JSON: a leaf's name, or an object with one key.
    pub fn parse(value: &Value) -> std::result::Result<Tree, String> {
        match value {
            Value::String(name) => Ok(leaf(name)),
            Value::Object(map) => {
                if let (Some(rec), Some(body)) = (map.get("rec"), map.get("body")) {
                    return Ok(Tree::Rec(number(rec)?, Box::new(Tree::parse(body)?)));
                }
                let mut entries = map.iter();
                let (Some((key, inner)), None) = (entries.next(), entries.next()) else {
                    return Err(format!("a tree node with {} keys", map.len()));
                };
                let boxed = |v: &Value| Tree::parse(v).map(Box::new);
                Ok(match key.as_str() {
                    "seq" => Tree::Seq(
                        array(inner)?
                            .iter()
                            .map(Tree::parse)
                            .collect::<std::result::Result<_, _>>()?,
                    ),
                    "optional" => Tree::Optional(boxed(inner)?),
                    "list" => Tree::List(boxed(inner)?),
                    "holder" => Tree::Holder(boxed(inner)?),
                    "map" => {
                        let [k, v] = pair(inner)?;
                        Tree::Map(boxed(k)?, boxed(v)?)
                    }
                    "either" => {
                        let [l, r] = pair(inner)?;
                        Tree::Either(boxed(l)?, boxed(r)?)
                    }
                    "dispatch" => {
                        let Value::Object(cases) = inner else {
                            return Err("dispatch cases are not an object".to_owned());
                        };
                        let mut out = HashMap::with_capacity(cases.len());
                        for (id, case) in cases {
                            let id = id
                                .parse::<i32>()
                                .map_err(|_| format!("dispatch case id {id}"))?;
                            out.insert(id, Tree::parse(case)?);
                        }
                        Tree::Dispatch(out)
                    }
                    "self" => Tree::Recurse(number(inner)?),
                    "unknown" => Tree::Unknown(inner.as_str().unwrap_or("?").to_owned()),
                    other => Tree::Unknown(format!("tree node {other}")),
                })
            }
            other => Err(format!("a tree node that is {other}")),
        }
    }

    /// Whether every value of this codec can be read past: no [`Tree::Unknown`]
    /// outside a dispatch's cases (a case nobody sends shouldn't cost the rest).
    pub fn complete(&self) -> bool {
        match self {
            Tree::Unknown(_) => false,
            Tree::Seq(fields) => fields.iter().all(Tree::complete),
            Tree::Optional(t) | Tree::List(t) | Tree::Holder(t) | Tree::Rec(_, t) => t.complete(),
            Tree::Map(a, b) | Tree::Either(a, b) => a.complete() && b.complete(),
            _ => true,
        }
    }
}

fn leaf(name: &str) -> Tree {
    match name {
        "unit" => Tree::Unit,
        "bool" => Tree::Bool,
        "byte" => Tree::Byte,
        "short" => Tree::Short,
        "int" => Tree::Int,
        "long" => Tree::Long,
        "float" => Tree::Float,
        "double" => Tree::Double,
        "varint" => Tree::VarInt,
        "varlong" => Tree::VarLong,
        "string" => Tree::String,
        "bytes" => Tree::Bytes,
        "nbt" => Tree::Nbt,
        "uuid" => Tree::Uuid,
        "vec3f" => Tree::Vec3f,
        "quaternion" => Tree::Quaternion,
        "properties" => Tree::Properties,
        "game_profile" => Tree::GameProfile,
        "item" => Tree::Item,
        "patch" => Tree::Patch,
        "fluid" => Tree::Fluid,
        "holder_set" => Tree::HolderSet,
        other => Tree::Unknown(format!("leaf {other}")),
    }
}

fn number(v: &Value) -> std::result::Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("{v} is not a recursion number"))
}

fn array(v: &Value) -> std::result::Result<&Vec<Value>, String> {
    v.as_array().ok_or_else(|| format!("{v} is not a list"))
}

fn pair(v: &Value) -> std::result::Result<[&Value; 2], String> {
    match array(v)?.as_slice() {
        [a, b] => Ok([a, b]),
        other => Err(format!("{} codecs where two are needed", other.len())),
    }
}

/// The trees of one server, by registry and the server's id.
#[derive(Debug, Default)]
pub struct Tables {
    trees: HashMap<String, HashMap<i32, Tree>>,
}

/// What [`Tables::parse`] made of a table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    /// Trees every value of which can be read past.
    pub usable: usize,
    /// Entries with a codec the oracle couldn't describe (they keep failing as before).
    pub unknown: usize,
    /// Lines that aren't `registry \t id \t name \t json`.
    pub bad: usize,
}

impl Tables {
    pub fn parse(tsv: &str) -> (Tables, Loaded) {
        let mut tables = Tables::default();
        let mut loaded = Loaded::default();
        for line in tsv.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut cols = line.splitn(4, '\t');
            let (Some(registry), Some(id), Some(_name), Some(json)) =
                (cols.next(), cols.next(), cols.next(), cols.next())
            else {
                loaded.bad += 1;
                continue;
            };
            let tree = id.parse::<i32>().ok().and_then(|id| {
                let value = serde_json::from_str::<Value>(json).ok()?;
                Some((id, Tree::parse(&value).ok()?))
            });
            match tree {
                Some((id, tree)) if tree.complete() => {
                    tables
                        .trees
                        .entry(registry.to_owned())
                        .or_default()
                        .insert(id, tree);
                    loaded.usable += 1;
                }
                Some(_) => loaded.unknown += 1,
                None => loaded.bad += 1,
            }
        }
        (tables, loaded)
    }

    pub fn get(&self, registry: &str, id: i32) -> Option<&Tree> {
        self.trees.get(registry)?.get(&id)
    }
}

static SERVER: RwLock<Option<Arc<Tables>>> = RwLock::new(None);

/// Use the server's codec trees (its `*.codecs.tsv`) from now on.
pub fn set_server_codecs(tsv: &str) -> Loaded {
    let (tables, loaded) = Tables::parse(tsv);
    if let Ok(mut server) = SERVER.write() {
        *server = Some(Arc::new(tables));
    }
    loaded
}

#[cfg(test)]
thread_local! {
    /// A test's own tables: tests run on threads of their own, so one test's
    /// trees never reach another's stacks.
    static TEST: std::cell::RefCell<Option<Arc<Tables>>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `tsv`'s trees in place of the server's, on this thread.
#[cfg(test)]
pub(crate) fn with_test_tables<R>(tsv: &str, f: impl FnOnce() -> R) -> R {
    /// Takes the trees away again, also when `f` panics.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST.with(|t| *t.borrow_mut() = None);
        }
    }
    TEST.with(|t| *t.borrow_mut() = Some(Arc::new(Tables::parse(tsv).0)));
    let _reset = Reset;
    f()
}

fn tables() -> Option<Arc<Tables>> {
    #[cfg(test)]
    {
        if let Some(test) = TEST.with(|t| t.borrow().clone()) {
            return Some(test);
        }
    }
    SERVER.read().ok()?.clone()
}

/// Reads past one value of the server's `registry` entry `id` by its tree;
/// `None` when there is no usable tree for it. `item_depth` is how deep in
/// stacks the value sits (the 767 stack reader's nesting limit).
pub(crate) fn skip_server(
    registry: &str,
    id: i32,
    r: &mut Reader,
    item_depth: u32,
) -> Option<Result<()>> {
    let tables = tables()?;
    let tree = tables.get(registry, id)?;
    Some(skip_at(r, tree, item_depth))
}

/// Reads past exactly what `tree`'s codec writes, keeping none of it.
pub fn skip(r: &mut Reader, tree: &Tree) -> Result<()> {
    skip_at(r, tree, 0)
}

fn skip_at(r: &mut Reader, tree: &Tree, item_depth: u32) -> Result<()> {
    Skipper {
        item_depth,
        recs: Vec::new(),
        nesting: 0,
    }
    .skip(r, tree)
}

struct Skipper<'t> {
    item_depth: u32,
    /// The recursive codecs being read, innermost last.
    recs: Vec<(u32, &'t Tree)>,
    nesting: u32,
}

impl<'t> Skipper<'t> {
    fn skip(&mut self, r: &mut Reader, tree: &'t Tree) -> Result<()> {
        if self.nesting >= MAX_NESTING {
            return Err(Error::new("a mod's value nested too deep"));
        }
        self.nesting += 1;
        let out = self.node(r, tree);
        self.nesting -= 1;
        out
    }

    fn node(&mut self, r: &mut Reader, tree: &'t Tree) -> Result<()> {
        match tree {
            Tree::Unit => Ok(()),
            Tree::Bool | Tree::Byte => r.u8().map(drop),
            Tree::Short => r.u16().map(drop),
            Tree::Int | Tree::Float => r.i32().map(drop),
            Tree::Long | Tree::Double => r.i64().map(drop),
            Tree::VarInt => r.varint().map(drop),
            Tree::VarLong => r.varlong().map(drop),
            Tree::String | Tree::Bytes => sized(r),
            Tree::Nbt => r.nbt().map(drop),
            Tree::Uuid | Tree::Quaternion => r.bytes(16).map(drop),
            Tree::Vec3f => r.bytes(12).map(drop),
            Tree::Properties => properties(r),
            Tree::GameProfile => {
                r.bytes(16)?;
                sized(r)?;
                properties(r)
            }
            Tree::Item => crate::item::v767::skip_slot(r, self.item_depth),
            Tree::Patch => crate::item::v767::skip_patch(r, self.item_depth),
            Tree::Fluid => {
                if r.varint()? > 0 {
                    r.varint()?;
                    crate::item::v767::skip_patch(r, self.item_depth)?;
                }
                Ok(())
            }
            Tree::HolderSet => holder_set(r),
            Tree::Seq(fields) => {
                for field in fields {
                    self.skip(r, field)?;
                }
                Ok(())
            }
            Tree::Optional(value) => {
                if r.bool()? {
                    self.skip(r, value)?;
                }
                Ok(())
            }
            Tree::List(element) => {
                for _ in 0..r.count(r.remaining())? {
                    self.skip(r, element)?;
                }
                Ok(())
            }
            Tree::Map(key, value) => {
                for _ in 0..r.count(r.remaining())? {
                    self.skip(r, key)?;
                    self.skip(r, value)?;
                }
                Ok(())
            }
            Tree::Either(left, right) => {
                let side = if r.bool()? { left } else { right };
                self.skip(r, side)
            }
            Tree::Holder(direct) => {
                if r.varint()? == 0 {
                    self.skip(r, direct)?;
                }
                Ok(())
            }
            Tree::Dispatch(cases) => {
                let key = r.varint()?;
                let case = cases.get(&key).ok_or_else(|| {
                    Error::new(format!("no dispatch case {key} in a mod's codec"))
                })?;
                self.skip(r, case)
            }
            Tree::Rec(id, body) => {
                self.recs.push((*id, body));
                let out = self.skip(r, body);
                self.recs.pop();
                out
            }
            Tree::Recurse(id) => {
                let body = self
                    .recs
                    .iter()
                    .rev()
                    .find(|(rec, _)| rec == id)
                    .map(|&(_, body)| body)
                    .ok_or_else(|| Error::new(format!("recursion {id} outside its codec")))?;
                self.skip(r, body)
            }
            Tree::Unknown(what) => Err(Error::new(format!("no codec known for {what}"))),
        }
    }
}

/// A VarInt length and that many bytes, bounded by what is left rather than
/// by the codec's own maximum.
fn sized(r: &mut Reader) -> Result<()> {
    let n = r.count(r.remaining())?;
    r.bytes(n).map(drop)
}

/// GAME_PROFILE_PROPERTIES: a count, then name, value and an optional signature each.
fn properties(r: &mut Reader) -> Result<()> {
    for _ in 0..r.count(r.remaining())? {
        sized(r)?;
        sized(r)?;
        if r.bool()? {
            sized(r)?;
        }
    }
    Ok(())
}

/// NeoForge's ByteBufCodecs.holderSet: 0 and a tag id, or the size plus one
/// and the holder ids; below 0 a custom holder set type, which can't be read.
fn holder_set(r: &mut Reader) -> Result<()> {
    match r.varint()? {
        0 => sized(r),
        n if n > 0 => {
            // Every id is at least a byte.
            let ids = (n - 1) as usize;
            if ids > r.remaining() {
                return Err(Error::new("holder set past the end"));
            }
            (0..ids).try_for_each(|_| r.varint().map(drop))
        }
        n => Err(Error::new(format!("custom holder set type {}", -1 - n))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree(v: Value) -> Tree {
        Tree::parse(&v).unwrap()
    }

    /// Skips `bytes` by `t` and checks every byte was read.
    fn skips(t: &Tree, bytes: &[u8]) {
        let mut r = Reader::new(bytes);
        skip(&mut r, t).unwrap();
        assert_eq!(r.remaining(), 0, "{t:?} read all of {bytes:?}");
    }

    fn fails(t: &Tree, bytes: &[u8]) {
        assert!(
            skip(&mut Reader::new(bytes), t).is_err(),
            "{t:?} on {bytes:?}"
        );
    }

    #[test]
    fn leaves_and_nodes_parse() {
        assert_eq!(tree(json!("varint")), Tree::VarInt);
        assert_eq!(
            tree(json!({"seq": ["bool", {"optional": "string"}, {"map": ["varint", "nbt"]}]})),
            Tree::Seq(vec![
                Tree::Bool,
                Tree::Optional(Box::new(Tree::String)),
                Tree::Map(Box::new(Tree::VarInt), Box::new(Tree::Nbt)),
            ])
        );
        assert_eq!(
            tree(json!({"rec": 3, "body": {"list": {"self": 3}}})),
            Tree::Rec(3, Box::new(Tree::List(Box::new(Tree::Recurse(3)))))
        );
        assert_eq!(
            tree(json!("something_new")),
            Tree::Unknown("leaf something_new".into())
        );
        assert!(Tree::parse(&json!({"map": ["varint"]})).is_err());
        assert!(Tree::parse(&json!(7)).is_err());
    }

    #[test]
    fn unknown_codecs_are_incomplete_except_in_dispatch_cases() {
        let unknown = json!({"unknown": "a.mod.Codec$1"});
        assert!(!tree(json!({"seq": ["int", unknown]})).complete());
        assert!(!tree(json!({"optional": unknown})).complete());
        assert!(tree(json!({"dispatch": {"0": "int", "1": unknown}})).complete());
        assert!(tree(json!({"seq": ["int", "uuid"]})).complete());
    }

    #[test]
    fn primitives_read_their_width() {
        skips(&Tree::Unit, &[]);
        skips(&Tree::Bool, &[1]);
        skips(&Tree::Short, &[0, 5]);
        skips(&Tree::Int, &[0, 0, 0, 5]);
        skips(&Tree::Float, &1.5f32.to_be_bytes());
        skips(&Tree::Long, &[0; 8]);
        skips(&Tree::Double, &[0; 8]);
        skips(&Tree::VarInt, &[0xFF, 0x01]);
        skips(&Tree::VarLong, &[0x80, 0x80, 0x01]);
        skips(&Tree::Uuid, &[7; 16]);
        skips(&Tree::Vec3f, &[0; 12]);
        skips(&Tree::Quaternion, &[0; 16]);
        skips(&Tree::String, &[3, b'a', b':', b'b']);
        skips(&Tree::Bytes, &[2, 0xFE, 0xFF]);
        fails(&Tree::Int, &[0, 0]);
        fails(&Tree::String, &[5, b'a']);
    }

    #[test]
    fn network_nbt_is_read_whole() {
        // A compound holding byte "a" = 5, then an End tag (an absent compound).
        skips(&Tree::Nbt, &[10, 1, 0, 1, b'a', 5, 0]);
        skips(&Tree::Nbt, &[0]);
        let t = tree(json!({"seq": ["nbt", "varint"]}));
        skips(&t, &[10, 1, 0, 1, b'a', 5, 0, 42]);
    }

    #[test]
    fn optionals_lists_maps_and_eithers_follow_their_flags_and_counts() {
        let t = tree(json!({"seq": [{"optional": "int"}, {"optional": "int"}]}));
        skips(&t, &[0, 1, 0, 0, 0, 9]);
        let list = tree(json!({"list": "string"}));
        skips(&list, &[2, 1, b'x', 0]);
        fails(&list, &[200, 1]);
        let map = tree(json!({"map": ["string", "varint"]}));
        skips(&map, &[2, 1, b'a', 0x80, 0x01, 1, b'b', 3]);
        let either = tree(json!({"either": ["int", "bool"]}));
        skips(&either, &[1, 0, 0, 0, 1]);
        skips(&either, &[0, 1]);
    }

    #[test]
    fn holders_and_holder_sets() {
        let holder = tree(json!({"holder": {"seq": ["string", "float"]}}));
        skips(&holder, &[5]);
        let mut direct = vec![0, 1, b's'];
        direct.extend_from_slice(&2.0f32.to_be_bytes());
        skips(&holder, &direct);
        skips(&Tree::HolderSet, &[0, 3, b'a', b':', b'b']);
        skips(&Tree::HolderSet, &[3, 7, 0x80, 0x01]);
        // NeoForge's custom holder sets (-2 - type) can't be read.
        fails(&Tree::HolderSet, &[0xFE, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn dispatch_reads_the_case_its_key_picks() {
        let t = tree(
            json!({"dispatch": {"0": "unit", "4": {"seq": ["varint", "bool"]}, "5": {"unknown": "x"}}}),
        );
        skips(&t, &[0]);
        skips(&t, &[4, 9, 1]);
        fails(&t, &[5, 0]);
        fails(&t, &[6]);
    }

    #[test]
    fn recursive_codecs_read_themselves_again() {
        // A tree of nodes: a name and an optional list of child nodes.
        let t = tree(
            json!({"rec": 0, "body": {"seq": ["string", {"optional": {"list": {"self": 0}}}]}}),
        );
        skips(&t, &[1, b'r', 1, 2, 1, b'a', 0, 1, b'b', 1, 0]);
        fails(&tree(json!({"self": 1})), &[]);
        // A recursion that never ends on its own is cut off.
        let endless = Tree::Rec(0, Box::new(Tree::Seq(vec![Tree::Recurse(0)])));
        fails(&endless, &[]);
    }

    #[test]
    fn profiles_and_properties() {
        let mut profile = vec![1; 16];
        profile.extend_from_slice(&[2, b'h', b'i', 1, 1, b't', 1, b'v', 1, 1, b's']);
        skips(&Tree::GameProfile, &profile);
        skips(&Tree::Properties, &[2, 1, b'a', 1, b'b', 0, 1, b'c', 0, 0]);
    }

    #[test]
    fn stacks_patches_and_fluids_go_through_the_1_21_1_stack_reader() {
        // An empty stack, then 2 of item 9 with max_damage (2) 100.
        let t = tree(json!({"list": "item"}));
        skips(&t, &[2, 0, 2, 9, 1, 0, 2, 100]);
        // A patch removing unbreakable (4).
        skips(&Tree::Patch, &[0, 1, 4]);
        // An empty fluid, and 1000 mB of fluid 3 with no components.
        skips(&Tree::Fluid, &[0]);
        skips(&Tree::Fluid, &[0xE8, 0x07, 3, 0, 0]);
        // A stack with a component nothing knows still fails.
        fails(&Tree::Item, &[1, 9, 1, 0, 120, 1]);
    }

    #[test]
    fn tables_keep_complete_trees_by_registry_and_id() {
        let tsv = "# horizon-oracle codecs v1\n\
            minecraft:data_component_type\t70\tmod:charge\t\"varint\"\n\
            minecraft:data_component_type\t71\tmod:odd\t{\"unknown\":\"mod.Codec$2\"}\n\
            neoforge:entity_data_serializers\t3\tmod:state\t{\"seq\":[\"bool\",\"int\"]}\n\
            not a line\n\
            minecraft:particle_type\tx\tmod:p\t\"unit\"\n";
        let (tables, loaded) = Tables::parse(tsv);
        assert_eq!(
            loaded,
            Loaded {
                usable: 2,
                unknown: 1,
                bad: 2
            }
        );
        assert_eq!(tables.get(COMPONENTS, 70), Some(&Tree::VarInt));
        assert_eq!(tables.get(COMPONENTS, 71), None);
        assert_eq!(
            tables.get(SERIALIZERS, 3),
            Some(&Tree::Seq(vec![Tree::Bool, Tree::Int]))
        );
        assert_eq!(tables.get(PARTICLES, 3), None);
    }

    #[test]
    fn server_trees_are_looked_up_by_registry_and_id() {
        let tsv = "minecraft:data_component_type\t70\tmod:charge\t\"varint\"\n";
        with_test_tables(tsv, || {
            let mut r = Reader::new(&[0x80, 0x01]);
            assert!(skip_server(COMPONENTS, 70, &mut r, 0).unwrap().is_ok());
            assert_eq!(r.remaining(), 0);
            assert!(skip_server(COMPONENTS, 71, &mut r, 0).is_none());
            assert!(skip_server(SERIALIZERS, 70, &mut r, 0).is_none());
        });
        assert!(skip_server(COMPONENTS, 70, &mut Reader::new(&[1]), 0).is_none());
    }
}
