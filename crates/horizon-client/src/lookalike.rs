//! A vanilla block that looks like a modded server block, by its name: the stand-in
//! `game::server_states` maps to, and whose shapes `mod_content_server` borrows.

/// A vanilla block that looks like `name`'s block: the longest vanilla name ending its path
/// (`chipped:mossy_stone_bricks_2` is mossy stone bricks), else one of the same shape by its
/// last word (any `*_stairs` stairs, of stone or of wood by its name).
pub fn lookalike(name: &str) -> Option<String> {
    let path = name.rsplit([':', '/']).next()?;
    let path = path.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_');
    let words: Vec<&str> = path.split('_').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    if let Some(vanilla) = (0..words.len())
        .map(|k| words[k..].join("_"))
        .find(|candidate| VANILLA_SUFFIXES.contains(&candidate.as_str()))
    {
        return Some(format!("minecraft:{vanilla}"));
    }
    let stony = STONY.iter().any(|w| path.contains(w));
    let shape = |n: usize| words[words.len().saturating_sub(n)..].join("_");
    SHAPES
        .iter()
        .find(|(suffix, _, _)| {
            let n = suffix.split('_').count();
            words.len() >= n && shape(n) == *suffix
        })
        .map(|(_, wood, stone)| format!("minecraft:{}", if stony { stone } else { wood }))
}

const STONY: &[&str] = &[
    "stone", "brick", "cobble", "deepslate", "tile", "granite", "diorite", "andesite", "basalt",
    "marble", "slate", "concrete", "rock", "quartz", "prismarine", "terracotta", "mud", "tuff",
    "calcite", "obsidian", "sandstone", "limestone", "shale", "gneiss", "iron", "copper", "metal",
];

/// (last words, wood stand-in, stone stand-in)
const SHAPES: &[(&str, &str, &str)] = &[
    ("wall_hanging_sign", "oak_wall_hanging_sign", "oak_wall_hanging_sign"),
    ("hanging_sign", "oak_hanging_sign", "oak_hanging_sign"),
    ("wall_sign", "oak_wall_sign", "oak_wall_sign"),
    ("fence_gate", "oak_fence_gate", "oak_fence_gate"),
    ("pressure_plate", "oak_pressure_plate", "stone_pressure_plate"),
    ("trapdoor", "oak_trapdoor", "iron_trapdoor"),
    ("door", "oak_door", "iron_door"),
    ("stairs", "oak_stairs", "stone_brick_stairs"),
    ("slab", "oak_slab", "stone_brick_slab"),
    ("fence", "oak_fence", "nether_brick_fence"),
    ("wall", "cobblestone_wall", "stone_brick_wall"),
    ("button", "oak_button", "stone_button"),
    ("sign", "oak_sign", "oak_sign"),
    ("planks", "oak_planks", "oak_planks"),
    ("log", "oak_log", "oak_log"),
    ("stem", "crimson_stem", "crimson_stem"),
    ("wood", "oak_wood", "oak_wood"),
    ("hyphae", "crimson_hyphae", "crimson_hyphae"),
    ("leaves", "oak_leaves", "oak_leaves"),
    ("sapling", "oak_sapling", "oak_sapling"),
    ("bricks", "bricks", "stone_bricks"),
    ("tiles", "stone_bricks", "stone_bricks"),
    ("pane", "glass_pane", "glass_pane"),
    ("carpet", "white_carpet", "white_carpet"),
    ("wool", "white_wool", "white_wool"),
    ("ore", "stone", "stone"),
];

/// Vanilla blocks a mod block's name may end with (`*_crafting_table`, `*_lantern`...).
const VANILLA_SUFFIXES: &[&str] = &[
    "crafting_table", "bookshelf", "chiseled_bookshelf", "barrel", "chest", "ladder", "lantern",
    "soul_lantern", "torch", "wall_torch", "glass", "tinted_glass", "stone", "cobblestone", "dirt",
    "sand", "gravel", "clay", "ice", "packed_ice", "snow", "snow_block", "vine", "fern", "short_grass",
    "tall_grass", "dead_bush", "lily_pad", "moss_block", "moss_carpet", "obsidian", "glowstone",
    "sea_lantern", "lectern", "composter", "cauldron", "anvil", "beehive", "bee_nest", "campfire",
    "chain", "iron_bars", "scaffolding", "hay_block", "mushroom_stem", "mud", "mud_bricks",
    "stone_bricks", "mossy_stone_bricks", "cracked_stone_bricks", "chiseled_stone_bricks",
    "polished_andesite", "polished_diorite", "polished_granite", "smooth_stone", "quartz_block",
    "furnace", "smoker", "blast_furnace", "cobweb", "fire", "soul_fire", "water", "lava", "spawner",
    "terracotta", "basalt", "blackstone", "deepslate", "tuff", "calcite", "netherrack", "end_stone",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookalike_uses_the_vanilla_name_a_path_ends_with() {
        assert_eq!(
            lookalike("chipped:mossy_stone_bricks_2").as_deref(),
            Some("minecraft:mossy_stone_bricks")
        );
        assert_eq!(
            lookalike("everycomp:c/biomesoplenty/fir_crafting_table").as_deref(),
            Some("minecraft:crafting_table")
        );
    }

    #[test]
    fn lookalike_picks_a_shape_by_the_last_words() {
        assert_eq!(lookalike("everycomp:c/bop/fir_stairs").as_deref(), Some("minecraft:oak_stairs"));
        assert_eq!(
            lookalike("chipped:cobbled_deepslate_slab").as_deref(),
            Some("minecraft:stone_brick_slab")
        );
        assert_eq!(
            lookalike("everycomp:fir_fence_gate").as_deref(),
            Some("minecraft:oak_fence_gate")
        );
        assert_eq!(lookalike("bop:fir_log").as_deref(), Some("minecraft:oak_log"));
    }

    #[test]
    fn lookalike_leaves_furniture_and_food_alone() {
        assert_eq!(lookalike("handcrafted:oak_table"), None);
        assert_eq!(lookalike("createfood:apple_pie"), None);
    }
}
