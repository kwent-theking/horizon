//! The block state ids of a modded NeoForge 1.21.1 server (protocol 767), from the table the dev
//! oracle exports (`-Dhorizon.oracle.stateids=FILE`, mods/horizon/src/neoforge-oracle): the
//! server's chunk palettes and block updates carry its own state ids (1.2 million with ~130 mods),
//! which our registry (vanilla plus the ported .hmod content) numbers differently. Each server
//! state maps to ours by name and properties, else to the block's default state with the
//! properties it shares, else to a vanilla block of the same shape (planks, stairs, a door...),
//! else to air.
use horizon_protocol::registry::{Builtin, Ids};
use horizon_world::{
    chunk::{BlockProperties, BlockState},
    registry::{BlockRegistry, BlockStateId},
};
#[cfg(test)]
use std::io::Cursor;
use std::{
    collections::{HashMap, HashSet},
    io::{self, BufRead, BufReader},
    path::PathBuf,
    sync::OnceLock,
};

/// Where the table is: `HORIZON_SERVER_STATES`, else `server-states/server-states.tsv` in the
/// game's folder (the connection loads the codec trees beside it before play,
/// horizon_client::net).
fn table_path() -> PathBuf {
    horizon_client::net::server_states_path()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Stats {
    pub exact: usize,
    pub default: usize,
    pub template: usize,
    pub everycomp: usize,
    pub air: usize,
}

/// A consecutive run of states belonging to the same server block.
struct BlockNameRun {
    first: usize,
    last: usize,
    name: Box<str>,
}

#[derive(Default)]
struct BlockNames(Vec<BlockNameRun>);

impl BlockNames {
    fn add(&mut self, state: usize, name: &str) {
        if let Some(last) = self.0.last_mut()
            && state.checked_sub(1) == Some(last.last)
            && name == last.name.as_ref()
        {
            last.last = state;
        } else {
            self.0.push(BlockNameRun {
                first: state,
                last: state,
                name: name.into(),
            });
        }
    }

    fn get(&self, id: usize) -> Option<&str> {
        let i = self
            .0
            .partition_point(|run| run.first <= id)
            .checked_sub(1)?;
        let run = &self.0[i];
        (id <= run.last).then_some(run.name.as_ref())
    }
}

#[derive(Default)]
struct BlockMap {
    states: Vec<u32>,
    server_names: BlockNames,
}

static MAP: OnceLock<BlockMap> = OnceLock::new();

/// The exact server block name prior to local state substitutions.
pub(super) fn server_block_name(state: i32) -> Option<&'static str> {
    MAP.get()?.server_names.get(usize::try_from(state).ok()?)
}

/// Server state id to local state id; empty without a table. Built once per process.
pub(super) fn block_map(registry: &BlockRegistry) -> &'static [u32] {
    &MAP.get_or_init(|| {
        let path = table_path();
        let started = std::time::Instant::now();
        match build_file(&path, registry) {
            Ok((map, stats)) => {
                log::info!(
                    "Horizon: server block states from {}: {} states ({} exact, {} default, \
                     {} lookalike, {} every-compat, {} air) in {:?}",
                    path.display(),
                    map.states.len(),
                    stats.exact,
                    stats.default,
                    stats.template,
                    stats.everycomp,
                    stats.air,
                    started.elapsed()
                );
                map
            }
            Err(e) => {
                log::info!(
                    "Horizon: no server block state table at {}: {e}",
                    path.display()
                );
                BlockMap::default()
            }
        }
    })
    .states
}

/// The built-in registries the server numbers with its mods' entries in them (items, entity
/// types, menus, sounds ...), from the oracle's `*.registries.tsv` (`registry \t id \t name`);
/// the rest stay ours. Names then resolve to our content as usual. Particle types stay ours: the
/// 767 translation renumbers them by name (horizon_protocol::entity_data::v767).
const SERVER_NUMBERED: &[&str] = &[
    "minecraft:item",
    "minecraft:block",
    "minecraft:entity_type",
    "minecraft:block_entity_type",
    "minecraft:menu",
    "minecraft:sound_event",
    "minecraft:mob_effect",
    "minecraft:attribute",
    "minecraft:fluid",
    "minecraft:potion",
    "minecraft:villager_profession",
    "minecraft:villager_type",
    "minecraft:game_event",
    "minecraft:stat_type",
    "minecraft:custom_stat",
];

pub(super) fn builtin(local: &Builtin) -> Option<Builtin> {
    let path = table_path().with_extension("registries.tsv");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            log::info!(
                "Horizon: no server registry table at {}: {e}",
                path.display()
            );
            return None;
        }
    };
    // Names for the log lines about mod ids the 767 translation can't read.
    horizon_protocol::translation::set_server_names(&text);
    let mut names: HashMap<&str, Vec<String>> = HashMap::new();
    for line in text.lines() {
        let mut cols = line.splitn(3, '\t');
        let (Some(registry), Some(id), Some(name)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        if !SERVER_NUMBERED.contains(&registry) {
            continue;
        }
        let Ok(id) = id.parse::<usize>() else {
            continue;
        };
        let list = names.entry(registry).or_default();
        if list.len() <= id {
            list.resize(id + 1, String::new());
        }
        list[id] = name.to_string();
    }
    let mut builtin = local.clone();
    let mut counts = Vec::new();
    for (registry, list) in names {
        counts.push(format!(
            "{} {}",
            registry.trim_start_matches("minecraft:"),
            list.len()
        ));
        builtin
            .registries
            .insert(registry.to_string(), Ids::from_names(list));
    }
    counts.sort();
    log::info!("Horizon: server-numbered registries: {}", counts.join(", "));
    Some(builtin)
}

/// Open the server table twice so wood discovery and state mapping never hold
/// its full (often 100+ MiB) text in memory alongside the mapping.
fn build_file(path: &std::path::Path, registry: &BlockRegistry) -> io::Result<(BlockMap, Stats)> {
    let woods = wood_names_reader(BufReader::new(std::fs::File::open(path)?))?;
    build_with_woods(BufReader::new(std::fs::File::open(path)?), registry, &woods)
}

/// The table's lines are `id \t block \t key=value,...` after a `#` header.
/// Kept as a small in-memory adapter for fixtures; the live path uses `build_file`.
#[cfg(test)]
fn build(text: &str, registry: &BlockRegistry) -> (Vec<u32>, Stats) {
    let woods = wood_names(text);
    let (table, stats) = build_with_woods(Cursor::new(text.as_bytes()), registry, &woods).unwrap();
    (table.states, stats)
}

#[cfg(test)]
#[test]
fn cbc_server_block_names_preserve_identity_across_state_runs() {
    let mut names = BlockNames::default();
    names.add(10, "example:original");
    names.add(11, "example:original");
    names.add(14, "minecraft:oak_planks");
    assert_eq!(names.get(10), Some("example:original"));
    assert_eq!(names.get(11), names.get(10));
    assert_eq!(names.get(12), None);
    assert_eq!(names.get(14), Some("minecraft:oak_planks"));
}

fn build_with_woods(
    mut reader: impl BufRead,
    registry: &BlockRegistry,
    woods: &Woods,
) -> io::Result<(BlockMap, Stats)> {
    let mut map: Vec<u32> = Vec::new();
    let mut server_names = BlockNames::default();
    let mut stats = Stats::default();
    let mut blocks: HashMap<String, Option<Target>> = HashMap::new();
    let mut buffer = String::new();
    loop {
        buffer.clear();
        if reader.read_line(&mut buffer)? == 0 {
            break;
        }
        let line = line_text(&buffer);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let (Some(id), Some(name)) = (cols.next(), cols.next()) else {
            continue;
        };
        let Ok(id) = id.parse::<usize>() else {
            continue;
        };
        if map.len() <= id {
            map.resize(id + 1, 0);
        }
        server_names.add(id, name);
        let properties = parse_properties(cols.next().unwrap_or(""));
        let state = BlockState {
            name: name.to_string(),
            properties,
        };
        // Most exported rows are complete states: avoid cloning all default
        // property strings before resolving an identity that already exists.
        let exact = registry
            .default_state(name)
            .and_then(|id| registry.state(id))
            .filter(|default| default.properties.keys().eq(state.properties.keys()))
            .and_then(|_| registry.id(&state));
        if let Some(local) = exact.or_else(|| registry.resolve(&state).ok()) {
            stats.exact += 1;
            map[id] = local.0;
            continue;
        }
        let target = if let Some(&known) = blocks.get(name) {
            known
        } else {
            let known = target(name, registry, woods);
            blocks.insert(name.to_owned(), known);
            known
        };
        map[id] = match target {
            Some(Target::Same(default)) => {
                stats.default += 1;
                with_shared(registry, default, &state.properties).0
            }
            Some(Target::EveryComp(default)) => {
                stats.everycomp += 1;
                with_shared(registry, default, &state.properties).0
            }
            Some(Target::Lookalike(default)) => {
                stats.template += 1;
                with_shared(registry, default, &state.properties).0
            }
            None => {
                stats.air += 1;
                0
            }
        };
    }
    server_names.0.sort_by_key(|run| run.first);
    Ok((
        BlockMap {
            states: map,
            server_names,
        },
        stats,
    ))
}

fn line_text(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(line) => line.strip_suffix('\r').unwrap_or(line),
        None => line,
    }
}

fn parse_properties(text: &str) -> BlockProperties {
    text.split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Target {
    /// The block is ours, but this state's properties are not (a ported block's template).
    Same(BlockStateId),
    /// The module's own block for another wood (`farmersdelight:oak_cabinet` for a baobab cabinet).
    EveryComp(BlockStateId),
    /// A vanilla block of the same shape stands in.
    Lookalike(BlockStateId),
}

fn target(name: &str, registry: &BlockRegistry, woods: &Woods) -> Option<Target> {
    if let Some(default) = registry.default_state(name) {
        return Some(Target::Same(default));
    }
    if let Some(default) = everycomp_candidates(name, woods)
        .iter()
        .find_map(|candidate| registry.default_state(candidate))
    {
        return Some(Target::EveryComp(default));
    }
    horizon_client::lookalike::lookalike(name)
        .and_then(|vanilla| registry.default_state(&vanilla))
        .map(Target::Lookalike)
}

/// The woods each wood-owning mod declares, most words first: a `<mod>:<wood>_planks` block per wood.
type Woods = HashMap<String, Vec<String>>;

/// The module mods Every Compat generates blocks for, by their code in `everycomp:<code>/...`.
const EVERYCOMP_MODULES: &[(&str, &str)] = &[
    ("ch", "chipped"),
    ("hc", "handcrafted"),
    ("tf", "twilightforest"),
    ("fd", "farmersdelight"),
    ("c", "create"),
    ("ru", "regions_unexplored"),
];

/// The woods a module's own blocks are made for, tried in this order as stand-ins.
const TEMPLATE_WOODS: &[&str] = &[
    "oak", "spruce", "birch", "dark_oak", "jungle", "acacia", "mangrove", "cherry", "bamboo",
    "crimson", "warped",
];

/// Every wood of every mod, from the table's `<mod>:<wood>_planks` blocks (the planks of Chipped
/// and Every Compat are patterns, not woods).
#[cfg(test)]
fn wood_names(text: &str) -> Woods {
    wood_names_reader(Cursor::new(text.as_bytes())).expect("in-memory text is readable")
}

fn wood_names_reader(mut reader: impl BufRead) -> io::Result<Woods> {
    let mut planks: HashSet<String> = HashSet::new();
    let mut buffer = String::new();
    loop {
        buffer.clear();
        if reader.read_line(&mut buffer)? == 0 {
            break;
        }
        if let Some(name) = line_text(&buffer).split('\t').nth(1)
            && name.ends_with("_planks")
        {
            planks.insert(name.to_owned());
        }
    }
    let mut woods = Woods::new();
    for name in planks {
        let Some((owner, path)) = name.split_once(':') else {
            continue;
        };
        if owner == "chipped" || owner == "everycomp" {
            continue;
        }
        let wood = path.trim_end_matches("_planks");
        woods
            .entry(owner.to_string())
            .or_default()
            .push(wood.to_string());
    }
    for list in woods.values_mut() {
        list.sort_by(|a, b| {
            b.split('_')
                .count()
                .cmp(&a.split('_').count())
                .then_with(|| b.cmp(a))
        });
    }
    Ok(woods)
}

/// `everycomp:<module>/<wood mod>/<path>` as (module mod, wood mod, path).
fn split_everycomp(name: &str) -> Option<(&'static str, &str, &str)> {
    let (code, rest) = name.strip_prefix("everycomp:")?.split_once('/')?;
    let (_, module) = EVERYCOMP_MODULES.iter().find(|(c, _)| *c == code)?;
    let (wood_mod, path) = rest.split_once('/')?;
    Some((module, wood_mod, path))
}

/// Where the longest of `woods` stands in `words` as a whole-word run: (first word, word count).
fn wood_run(words: &[&str], woods: &[String]) -> Option<(usize, usize)> {
    woods.iter().find_map(|wood| {
        let wood_words: Vec<&str> = wood.split('_').collect();
        words
            .windows(wood_words.len())
            .position(|window| window == wood_words.as_slice())
            .map(|start| (start, wood_words.len()))
    })
}

/// The module's own names for an Every Compat block, one per template wood: its path with the
/// wood swapped (`everycomp:fd/regions_unexplored/baobab_cabinet` is `farmersdelight:oak_cabinet`).
fn everycomp_candidates(name: &str, woods: &Woods) -> Vec<String> {
    let Some((module, wood_mod, path)) = split_everycomp(name) else {
        return Vec::new();
    };
    let words: Vec<&str> = path.split('_').collect();
    let Some((start, len)) = woods.get(wood_mod).and_then(|list| wood_run(&words, list)) else {
        return Vec::new();
    };
    TEMPLATE_WOODS
        .iter()
        .map(|template| {
            let swapped = [&words[..start], &[*template], &words[start + len..]].concat();
            format!("{module}:{}", swapped.join("_"))
        })
        .collect()
}

/// `default` with every server property it also has, when that state exists.
fn with_shared(
    registry: &BlockRegistry,
    default: BlockStateId,
    properties: &BlockProperties,
) -> BlockStateId {
    let Some(base) = registry.state(default) else {
        return default;
    };
    let mut state = base.clone();
    let mut changed = false;
    for (key, value) in properties {
        if let Some(slot) = state.properties.get_mut(key) {
            if slot != value {
                *slot = value.clone();
                changed = true;
            }
        }
    }
    if !changed {
        return default;
    }
    registry.id(&state).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn woods() -> Woods {
        wood_names(
            "0\tminecraft:oak_planks\t\n1\tminecraft:dark_oak_planks\t\n2\tminecraft:oak_planks\tx=y\n\
             3\tregions_unexplored:baobab_planks\t\n4\tbiomeswevegone:cika_planks\t\n\
             5\ttwilightforest:twilight_oak_planks\t\n6\tregions_unexplored:blue_bioshroom_planks\t\n\
             7\tbiomesoplenty:dead_planks\t\n8\tchipped:boxed_acacia_planks\t\n\
             9\teverycomp:ch/aether/skyroot_planks\t\n",
        )
    }

    fn first_candidate(name: &str) -> Option<String> {
        everycomp_candidates(name, &woods()).into_iter().next()
    }

    #[test]
    fn wood_names_come_from_each_mods_planks() {
        let woods = woods();
        assert_eq!(woods["minecraft"], ["dark_oak", "oak"]);
        assert_eq!(woods["regions_unexplored"], ["blue_bioshroom", "baobab"]);
        assert!(!woods.contains_key("chipped") && !woods.contains_key("everycomp"));
    }

    #[test]
    fn streamed_builder_preserves_sparse_and_duplicate_ids_and_crlf() {
        let registry = BlockRegistry::from_facts_json(
            &br#"{"defaults":{"minecraft:air":0,"minecraft:stone":1,"minecraft:oak_planks":2},
               "states":[["minecraft:air",true,false,"air","minecraft:air"],
                         ["minecraft:stone",false,false,"stone","minecraft:stone"],
                         ["minecraft:oak_planks",false,false,"planks","minecraft:oak_planks"]]}"#[..],
        )
        .unwrap();
        let text = "# header\r\n0\tminecraft:stone\t\r\n2\tminecraft:oak_planks\t\r\n4\tunknown:block\tfoo=bar\r\n4\tminecraft:stone\t\r\ninvalid\tignored:block\t\r\n";
        let (map, stats) = build(text, &registry);
        assert_eq!(map, [1, 0, 2, 0, 1]);
        assert_eq!(
            stats,
            Stats {
                exact: 3,
                air: 1,
                ..Stats::default()
            }
        );
    }

    #[test]
    fn streamed_wood_pass_rejects_invalid_utf8_like_read_to_string() {
        let error = wood_names_reader(Cursor::new(b"0\tmod:oak_planks\t\xff\n")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn incomplete_descriptors_still_inherit_default_properties() {
        // A registry need not give all states of a block the same property keys.
        // An identity hit with fewer keys must still pass through resolve().
        let registry = BlockRegistry::from_facts_json(
            &br#"{"defaults":{"minecraft:air":0,"test:block":1},"states":[
                ["minecraft:air",true,false,"","minecraft:air"],
                ["test:block",false,false,"",{"id":"test:block","properties":{"axis":"x"}}],
                ["test:block",false,false,"","test:block"]]}"#[..],
        )
        .unwrap();
        let (map, stats) = build("0\ttest:block\t\n1\ttest:block\taxis=x\n", &registry);
        assert_eq!(map, [1, 1]);
        assert_eq!(stats.exact, 2);
    }

    #[test]
    fn everycomp_names_swap_the_wood_for_the_modules_oak() {
        assert_eq!(
            first_candidate("everycomp:fd/regions_unexplored/baobab_cabinet").as_deref(),
            Some("farmersdelight:oak_cabinet")
        );
        assert_eq!(
            first_candidate("everycomp:c/biomeswevegone/cika_window").as_deref(),
            Some("create:oak_window")
        );
        assert_eq!(
            first_candidate("everycomp:hc/regions_unexplored/baobab_chair").as_deref(),
            Some("handcrafted:oak_chair")
        );
        assert_eq!(
            first_candidate("everycomp:tf/regions_unexplored/hollow_blue_bioshroom_log_climbable")
                .as_deref(),
            Some("twilightforest:hollow_oak_log_climbable")
        );
        assert_eq!(
            first_candidate("everycomp:ch/biomesoplenty/vertically_railed_dead_planks").as_deref(),
            Some("chipped:vertically_railed_oak_planks")
        );
    }

    #[test]
    fn everycomp_names_prefer_the_longest_wood_and_whole_words() {
        assert_eq!(
            first_candidate("everycomp:tf/twilightforest/twilight_oak_banister").as_deref(),
            Some("twilightforest:oak_banister")
        );
        assert_eq!(
            first_candidate("everycomp:hc/minecraft/dark_oak_chair").as_deref(),
            Some("handcrafted:oak_chair")
        );
        assert_eq!(
            first_candidate("everycomp:hc/regions_unexplored/baobabs_chair"),
            None
        );
    }

    #[test]
    fn everycomp_names_without_a_known_wood_or_module_have_no_candidates() {
        assert_eq!(
            first_candidate("everycomp:ch/aether/apple_golden_oak_leaves"),
            None
        );
        assert_eq!(first_candidate("everycomp:zz/minecraft/oak_chair"), None);
        assert_eq!(first_candidate("handcrafted:oak_chair"), None);
        let all = everycomp_candidates("everycomp:c/biomeswevegone/cika_window", &woods());
        assert_eq!(all.len(), TEMPLATE_WOODS.len());
        assert_eq!(all[3], "create:dark_oak_window");
    }

    #[test]
    fn properties_parse_from_the_table() {
        let p = parse_properties("facing=north,half=top");
        assert_eq!(p.get("facing").map(String::as_str), Some("north"));
        assert_eq!(p.get("half").map(String::as_str), Some("top"));
        assert!(parse_properties("").is_empty());
    }
}
