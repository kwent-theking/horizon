//! Matches a server entity id to a GeckoLib model, texture and animation
//! files of its namespace. Mods name these three differently
//! (`baby_skeleton` / `babyskeleton2.geo.json` / `babyskeleton.animation.json`),
//! so names are compared after `normalize`. Mirrored by
//! `horizon-test/research/scripts/gecko_resolve_dry.py`.
use std::path::PathBuf;

const SUFFIXES: [&str; 3] = ["entity", "mob", "model"];
const ANIMATION_SUFFIXES: [&str; 6] = [
    ".animation",
    ".animations",
    "_animations",
    "_animation",
    "animations",
    "animation",
];
/// Texture names that are overlays of the main skin, tried last.
const OVERLAY_WORDS: [&str; 8] = [
    "glow", "emissive", "layer", "overlay", "armor", "eyes", "mask", "sleep",
];
/// Shortest normalized name that may match by prefix, suffix or containment.
const MIN_FUZZY: usize = 4;
/// The two texture folders mods use; the first is preferred.
pub const TEXTURE_DIRS: [&str; 2] = ["entity", "entities"];

/// Lowercase letters and digits only, without trailing digits, a trailing
/// `entity`/`mob` and a plural `s`: `Baby_Skeleton2` and `babyskeletons` agree.
pub fn normalize(name: &str) -> String {
    let mut current: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    loop {
        let mut next = current.trim_end_matches(|c: char| c.is_ascii_digit());
        if let Some(rest) = SUFFIXES
            .iter()
            .find_map(|s| next.strip_suffix(s).filter(|rest| rest.len() >= 3))
        {
            next = rest;
        }
        if next.len() > 3 && next.ends_with('s') && !next.ends_with("ss") {
            next = &next[..next.len() - 1];
        }
        if next == current {
            return current;
        }
        current = next.to_owned();
    }
}

/// An animation file's name without `.json` and its `animation(s)` marker.
pub fn animation_stem(file_name: &str) -> String {
    let lower = file_name
        .strip_suffix(".json")
        .unwrap_or(file_name)
        .to_ascii_lowercase();
    ANIMATION_SUFFIXES
        .iter()
        .find(|s| lower.len() > s.len() && lower.ends_with(**s))
        .map_or_else(
            || lower.clone(),
            |s| lower[..lower.len() - s.len()].to_owned(),
        )
}

pub struct GeoEntry {
    pub stem: String,
    /// Folder names between `geo/` and the file.
    pub folders: Vec<String>,
    pub path: PathBuf,
}

pub struct TextureEntry {
    pub stem: String,
    pub folders: Vec<String>,
    pub path: PathBuf,
    /// Index into `TEXTURE_DIRS` of the folder holding it.
    pub dir: usize,
}

pub struct AnimationEntry {
    /// File name, e.g. `boneimp.animation.json`.
    pub name: String,
    pub path: PathBuf,
}

/// Everything a namespace ships for GeckoLib mobs.
#[derive(Default)]
pub struct NamespaceIndex {
    pub geo: Vec<GeoEntry>,
    pub textures: Vec<TextureEntry>,
    pub animations: Vec<AnimationEntry>,
}

pub struct Resolved<'a> {
    pub geo: &'a GeoEntry,
    pub texture: &'a TextureEntry,
    /// Best match first: clips of earlier files win a name clash.
    pub animations: Vec<&'a AnimationEntry>,
}

fn folder_is(folders: &[String], key: &str) -> bool {
    folders.iter().any(|f| normalize(f) == key)
}

fn fuzzy(key: &str) -> bool {
    key.len() >= MIN_FUZZY
}

fn is_overlay(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    OVERLAY_WORDS.iter().any(|w| lower.contains(w))
}

/// The texture for geo `stem` of entity `path`: exact stem, the entity id,
/// the same normalized name, a folder named after the mob, then a variant
/// (`starbuncle_black`) by prefix, suffix or containment. A normalized match
/// does not take the exact-stem texture of another model in `geos`.
fn pick_texture<'a>(
    textures: &'a [TextureEntry],
    geos: &[GeoEntry],
    stem: &str,
    path: &str,
) -> Option<&'a TextureEntry> {
    let keys = [normalize(stem), normalize(path)];
    let owned_by_other_geo =
        |t: &TextureEntry| geos.iter().any(|g| g.stem == t.stem && g.stem != stem);
    let tiers: [&dyn Fn(&TextureEntry) -> bool; 7] = [
        &|t| t.stem == stem,
        &|t| t.stem == path,
        &|t| keys.contains(&normalize(&t.stem)) && !owned_by_other_geo(t),
        &|t| keys.iter().any(|k| folder_is(&t.folders, k)),
        &|t| {
            let n = normalize(&t.stem);
            keys.iter().any(|k| fuzzy(k) && n.starts_with(k.as_str()))
        },
        &|t| {
            let n = normalize(&t.stem);
            keys.iter().any(|k| fuzzy(k) && n.ends_with(k.as_str()))
        },
        &|t| {
            let n = normalize(&t.stem);
            keys.iter().any(|k| fuzzy(k) && n.contains(k.as_str()))
        },
    ];
    tiers.iter().find_map(|tier| {
        textures.iter().filter(|t| tier(t)).min_by_key(|t| {
            (
                is_overlay(&t.stem),
                t.folders.len(),
                t.dir,
                t.stem.len(),
                t.stem.clone(),
            )
        })
    })
}

/// The model of entity `path` with a texture, best tier first: exact stem,
/// normalized stem, a folder named after the mob, a model named
/// `<mob>_variant`, then a model whose name the entity id starts or ends with.
fn pick_geo<'a>(
    index: &'a NamespaceIndex,
    path: &str,
) -> Option<(&'a GeoEntry, &'a TextureEntry)> {
    let key = normalize(path);
    let tiers: [&dyn Fn(&GeoEntry) -> bool; 5] = [
        &|g| g.stem == path,
        &|g| normalize(&g.stem) == key,
        &|g| folder_is(&g.folders, &key),
        &|g| fuzzy(&key) && normalize(&g.stem).starts_with(&key),
        &|g| {
            let n = normalize(&g.stem);
            fuzzy(&n) && (key.starts_with(&n) || key.ends_with(&n))
        },
    ];
    tiers.iter().enumerate().find_map(|(i, tier)| {
        let mut found: Vec<&GeoEntry> = index.geo.iter().filter(|g| tier(g)).collect();
        found.sort_by_key(|g| {
            let longest_first = if i == 4 {
                usize::MAX - normalize(&g.stem).len()
            } else {
                0
            };
            (longest_first, g.folders.len(), g.stem.len(), g.stem.clone())
        });
        found.into_iter().find_map(|g| {
            pick_texture(&index.textures, &index.geo, &g.stem, path).map(|texture| (g, texture))
        })
    })
}

/// Model, texture and animation files for entity `path` (the id without its
/// namespace) in `index`; `None` when no model with a texture matches.
pub fn resolve<'a>(index: &'a NamespaceIndex, path: &str) -> Option<Resolved<'a>> {
    let (geo, texture) = pick_geo(index, path)?;
    let keys = [normalize(&animation_stem(&geo.stem)), normalize(path)];
    let mut animations: Vec<&AnimationEntry> = index
        .animations
        .iter()
        .filter(|a| keys.contains(&normalize(&animation_stem(&a.name))))
        .collect();
    animations.sort_by_key(|a| {
        let own = animation_stem(&a.name) == geo.stem.to_ascii_lowercase();
        (!own, a.path.clone())
    });
    Some(Resolved {
        geo,
        texture,
        animations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(stem: &str, folders: &[&str]) -> GeoEntry {
        GeoEntry {
            stem: stem.into(),
            folders: folders.iter().map(|f| (*f).into()).collect(),
            path: PathBuf::from(format!("geo/{stem}.geo.json")),
        }
    }

    fn texture(stem: &str, folders: &[&str], dir: usize) -> TextureEntry {
        TextureEntry {
            stem: stem.into(),
            folders: folders.iter().map(|f| (*f).into()).collect(),
            path: PathBuf::from(format!("tex/{stem}.png")),
            dir,
        }
    }

    fn animation(name: &str) -> AnimationEntry {
        AnimationEntry {
            name: name.into(),
            path: PathBuf::from(format!("anim/{name}")),
        }
    }

    fn index(geos: Vec<GeoEntry>, textures: Vec<TextureEntry>) -> NamespaceIndex {
        NamespaceIndex {
            geo: geos,
            textures,
            animations: Vec::new(),
        }
    }

    #[test]
    fn normalize_drops_case_separators_digits_suffixes_and_plurals() {
        assert_eq!(normalize("Baby_Skeleton2"), "babyskeleton");
        assert_eq!(normalize("babyskeletons"), "babyskeleton");
        assert_eq!(normalize("bone_imp_entity"), "boneimp");
        assert_eq!(normalize("dark_vortex_mob2"), "darkvortex");
        assert_eq!(normalize("boss"), "boss");
        assert_eq!(normalize("mob"), "mob");
        assert_eq!(normalize("gas"), "gas");
    }

    #[test]
    fn animation_stem_strips_markers() {
        assert_eq!(animation_stem("babyskeleton.animation.json"), "babyskeleton");
        assert_eq!(
            animation_stem("weald_waddler_animations.json"),
            "weald_waddler"
        );
        assert_eq!(animation_stem("Wixie_Animation.json"), "wixie");
        assert_eq!(animation_stem("animation.json"), "animation");
    }

    #[test]
    fn exact_stem_beats_normalized_stem() {
        let index = index(
            vec![geo("babyskeleton2", &[]), geo("baby_skeleton", &[])],
            vec![
                texture("babyskeleton2", &[], 1),
                texture("baby_skeleton", &[], 0),
            ],
        );
        let r = resolve(&index, "baby_skeleton").unwrap();
        assert_eq!(r.geo.stem, "baby_skeleton");
        assert_eq!(r.texture.stem, "baby_skeleton");
    }

    #[test]
    fn entity_id_matches_geo_stem_by_normalization_in_entities_dir() {
        let mut index = index(
            vec![geo("babyskeleton2", &[])],
            vec![texture("babyskeleton2", &["sub"], 1)],
        );
        index.animations = vec![animation("babyskeleton.animation.json")];
        let r = resolve(&index, "baby_skeleton").unwrap();
        assert_eq!(r.geo.stem, "babyskeleton2");
        assert_eq!(r.texture.dir, 1);
        assert_eq!(r.animations.len(), 1);
    }

    #[test]
    fn texture_falls_back_to_entity_id_then_folder_then_variant() {
        let by_id = index(vec![geo("wolf", &[])], vec![texture("wolf", &["x"], 0)]);
        assert_eq!(resolve(&by_id, "wolf").unwrap().texture.stem, "wolf");

        let by_normalized = index(
            vec![geo("wolf_model", &[])],
            vec![texture("wolf", &[], 0), texture("other", &[], 0)],
        );
        assert!(resolve(&by_normalized, "wolf_model").is_some());

        let by_folder = index(
            vec![geo("wolf", &[])],
            vec![texture("main", &["wolf"], 0), texture("other", &["bear"], 0)],
        );
        assert_eq!(resolve(&by_folder, "wolf").unwrap().texture.stem, "main");

        let by_variant = index(vec![geo("wolf", &[])], vec![texture("wolf_skin", &[], 0)]);
        assert_eq!(resolve(&by_variant, "wolf").unwrap().texture.stem, "wolf_skin");
    }

    #[test]
    fn variants_prefer_the_plain_skin_over_overlays() {
        let index = index(
            vec![geo("starbuncle", &[])],
            vec![
                texture("starbuncle_eyes", &[], 0),
                texture("starbuncle_black", &[], 0),
                texture("starbuncle_light_blue", &[], 0),
            ],
        );
        assert_eq!(
            resolve(&index, "starbuncle").unwrap().texture.stem,
            "starbuncle_black"
        );
    }

    #[test]
    fn geo_in_a_folder_named_after_the_mob_matches() {
        let index = index(vec![geo("body", &["Marid"])], vec![texture("body", &[], 0)]);
        assert_eq!(resolve(&index, "marid").unwrap().geo.stem, "body");
    }

    #[test]
    fn model_without_texture_is_skipped_for_the_next_candidate() {
        let index = index(
            vec![geo("imp", &[]), geo("imp2", &[])],
            vec![texture("imp2", &[], 0)],
        );
        assert_eq!(resolve(&index, "imp").unwrap().geo.stem, "imp2");
    }

    #[test]
    fn prefix_models_need_four_letters_and_prefer_the_longest() {
        let index = index(
            vec![geo("umvuthana", &[]), geo("umvu", &[]), geo("cat", &[])],
            vec![
                texture("umvuthana", &[], 0),
                texture("umvu", &[], 0),
                texture("cat", &[], 0),
            ],
        );
        assert_eq!(
            resolve(&index, "umvuthana_raptor").unwrap().geo.stem,
            "umvuthana"
        );
        assert!(resolve(&index, "cat_toy").is_none());
        assert!(resolve(&index, "unknown").is_none());
    }

    #[test]
    fn animations_merge_every_matching_file_exact_name_first() {
        let mut index = index(
            vec![geo("babyskeleton2", &[])],
            vec![texture("babyskeleton2", &[], 0)],
        );
        index.animations = vec![
            animation("babyskeleton.animation.json"),
            animation("babyskeleton2.animation.json"),
            animation("babyskeletondeamon.animation.json"),
        ];
        let names: Vec<_> = resolve(&index, "baby_skeleton")
            .unwrap()
            .animations
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["babyskeleton2.animation.json", "babyskeleton.animation.json"]
        );
    }
}
