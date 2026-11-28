//! 1.21.1 (protocol 767) item stacks in the 26.3 shape: data component type
//! ids are renumbered by name and the values whose codec changed are
//! rewritten, after DataComponents' network codecs in both versions (javap of
//! the 1.21.1 client NeoForge 21.1 runs on, and of the 26.3 client). Registry
//! ids inside a stack (the item, enchantments, potions, effects, attributes)
//! stay the server's: the client names them through the server's tables.
//! Item data has no length prefix, so a component without a known codec (a
//! mod's) leaves the rest of its stack, and of the packet, unreadable, unless
//! the oracle's tree of the server's codec for it ([`codec_tree`]) reads past
//! it; it is then left out.
use super::skip::{self, Read};
use crate::{
    Error, Result,
    buf::{Reader, Writer},
    codec_tree,
};

const COMPONENT_REGISTRY: &str = codec_tree::COMPONENTS;

/// `minecraft:data_component_type` in 1.21.1's registration order: the
/// server's ids 0-56 (mods register theirs after them).
const NAMES_767: [&str; 57] = [
    "custom_data",
    "max_stack_size",
    "max_damage",
    "damage",
    "unbreakable",
    "custom_name",
    "item_name",
    "lore",
    "rarity",
    "enchantments",
    "can_place_on",
    "can_break",
    "attribute_modifiers",
    "custom_model_data",
    "hide_additional_tooltip",
    "hide_tooltip",
    "repair_cost",
    "creative_slot_lock",
    "enchantment_glint_override",
    "intangible_projectile",
    "food",
    "fire_resistant",
    "tool",
    "stored_enchantments",
    "dyed_color",
    "map_color",
    "map_id",
    "map_decorations",
    "map_post_processing",
    "charged_projectiles",
    "bundle_contents",
    "potion_contents",
    "suspicious_stew_effects",
    "writable_book_content",
    "written_book_content",
    "trim",
    "debug_stick_state",
    "entity_data",
    "bucket_entity_data",
    "block_entity_data",
    "instrument",
    "ominous_bottle_amplifier",
    "jukebox_playable",
    "recipes",
    "lodestone_tracker",
    "firework_explosion",
    "fireworks",
    "profile",
    "note_block_sound",
    "banner_patterns",
    "base_color",
    "pot_decorations",
    "container",
    "block_state",
    "bees",
    "lock",
    "container_loot",
];

/// `minecraft:data_component_type` in 26.3's registration order.
const NAMES_777: [&str; 122] = [
    "custom_data",
    "max_stack_size",
    "max_damage",
    "damage",
    "unbreakable",
    "use_effects",
    "custom_name",
    "minimum_attack_charge",
    "damage_type",
    "item_name",
    "item_model",
    "lore",
    "rarity",
    "enchantments",
    "can_place_on",
    "can_break",
    "attribute_modifiers",
    "custom_model_data",
    "tooltip_display",
    "repair_cost",
    "creative_slot_lock",
    "enchantment_glint_override",
    "intangible_projectile",
    "food",
    "consumable",
    "use_remainder",
    "use_cooldown",
    "damage_resistant",
    "tool",
    "weapon",
    "attack_range",
    "enchantable",
    "equippable",
    "repairable",
    "glider",
    "tooltip_style",
    "death_protection",
    "blocks_attacks",
    "piercing_weapon",
    "kinetic_weapon",
    "attack_animation",
    "interact_animation",
    "additional_trade_cost",
    "block_transformer",
    "villager_food",
    "stored_enchantments",
    "dye",
    "dyed_color",
    "map_id",
    "map_decorations",
    "map_post_processing",
    "charged_projectiles",
    "bundle_contents",
    "potion_contents",
    "potion_duration_scale",
    "suspicious_stew_effects",
    "writable_book_content",
    "written_book_content",
    "trim",
    "debug_stick_state",
    "entity_data",
    "bucket_entity_data",
    "block_entity_data",
    "instrument",
    "provides_trim_material",
    "ominous_bottle_amplifier",
    "jukebox_playable",
    "provides_banner_patterns",
    "recipes",
    "lodestone_tracker",
    "firework_explosion",
    "fireworks",
    "profile",
    "note_block_sound",
    "banner_patterns",
    "base_color",
    "pot_decorations",
    "container",
    "block_state",
    "bees",
    "sulfur_cube_content",
    "lock",
    "container_loot",
    "break_sound",
    "compostable",
    "cooking_fuel",
    "brewing_fuel",
    "mob_visibility",
    "villager/variant",
    "wolf/variant",
    "wolf/sound_variant",
    "wolf/collar",
    "fox/variant",
    "salmon/size",
    "parrot/variant",
    "tropical_fish/pattern",
    "tropical_fish/base_color",
    "tropical_fish/pattern_color",
    "mooshroom/variant",
    "rabbit/variant",
    "pig/variant",
    "pig/sound_variant",
    "cow/variant",
    "cow/sound_variant",
    "chicken/variant",
    "chicken/sound_variant",
    "zombie_nautilus/variant",
    "frog/variant",
    "horse/variant",
    "painting/variant",
    "llama/variant",
    "axolotl/variant",
    "cat/variant",
    "cat/sound_variant",
    "cat/collar",
    "sheep/color",
    "shulker/color",
    "provides_pottery_pattern",
    "sign_text_front",
    "sign_text_back",
    "waxed",
    "cushion/color",
];

/// Stacks inside stacks (bundles, containers, a food's leftover) deeper
/// than this are refused.
const MAX_DEPTH: u32 = 16;

fn name_767(kind: i32) -> Option<&'static str> {
    usize::try_from(kind)
        .ok()
        .and_then(|k| NAMES_767.get(k))
        .copied()
}

fn id_777(name: &str) -> Option<i32> {
    NAMES_777.iter().position(|n| *n == name).map(|i| i as i32)
}

/// The 26.3 id a 1.21.1 component type becomes, by name (`hide_tooltip`
/// turns into `tooltip_display`); `None` when 26.3 has no counterpart.
fn renumber(kind: i32) -> Option<i32> {
    let name = name_767(kind)?;
    id_777(if name == "hide_tooltip" {
        "tooltip_display"
    } else {
        name
    })
}

/// `ItemStack.OPTIONAL_STREAM_CODEC` to 26.3's: the count (0 is the empty
/// stack), the item's registry id (a plain id in both, `holderRegistry(ITEM)`),
/// then the patch.
pub(crate) fn slot(r: &mut Reader, w: &mut Writer) -> Result<()> {
    slot_at(r, w, 0)
}

fn slot_at(r: &mut Reader, w: &mut Writer, depth: u32) -> Result<()> {
    let count = r.varint()?;
    if count <= 0 {
        w.varint(0);
        return Ok(());
    }
    w.varint(count);
    w.varint(r.varint()?);
    patch(r, w, depth)
}

/// A 1.21.1 optional stack `depth` stacks deep, read and dropped (one inside
/// a mod's component, which is left out whole).
pub(crate) fn skip_slot(r: &mut Reader, depth: u32) -> Result<()> {
    slot_at(r, &mut Writer::new(), depth)
}

/// A 1.21.1 `DataComponentPatch`, read and dropped.
pub(crate) fn skip_patch(r: &mut Reader, depth: u32) -> Result<()> {
    patch(r, &mut Writer::new(), depth)
}

/// `ItemStack.STREAM_CODEC` (a stack that may not be empty, count first) to
/// 26.3's `ItemStackTemplate` (the item first).
pub(crate) fn template(r: &mut Reader, w: &mut Writer) -> Result<()> {
    template_at(r, w, 0)
}

fn template_at(r: &mut Reader, w: &mut Writer, depth: u32) -> Result<()> {
    let count = r.varint()?;
    if count <= 0 {
        return Err(Error::new("empty 1.21.1 item stack where one is required"));
    }
    template_rest(count, r, w, depth)
}

fn template_rest(count: i32, r: &mut Reader, w: &mut Writer, depth: u32) -> Result<()> {
    w.varint(r.varint()?);
    w.varint(count);
    patch(r, w, depth)
}

/// `DataComponentPatch.STREAM_CODEC`: the added and removed counts, each
/// added type id and its value, then the removed type ids. Components 26.3
/// lacks are left out.
fn patch(r: &mut Reader, w: &mut Writer, depth: u32) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(Error::new("1.21.1 item stacks nested too deep"));
    }
    let positive = r.count(65536)?;
    let negative = r.count(65536)?;
    let mut added = Writer::new();
    let mut kept = 0;
    for _ in 0..positive {
        kept += typed(r, &mut added, depth + 1)?;
    }
    let mut removed = Writer::new();
    let mut gone = 0;
    for _ in 0..negative {
        let kind = r.varint()?;
        match renumber(kind) {
            Some(id) => {
                removed.varint(id);
                gone += 1;
            }
            None if name_767(kind).is_none() => {
                crate::translation::unknown_767(COMPONENT_REGISTRY, kind)
            }
            None => {}
        }
    }
    w.varint(kept);
    w.varint(gone);
    w.bytes(&added.data);
    w.bytes(&removed.data);
    Ok(())
}

/// `DataComponentPredicate.STREAM_CODEC` (an `ItemCost`'s components): a
/// list of typed components, renumbered as in a patch.
pub(crate) fn exact_components(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let n = r.count(r.remaining())?;
    let mut list = Writer::new();
    let mut kept = 0;
    for _ in 0..n {
        kept += typed(r, &mut list, 1)?;
    }
    w.varint(kept);
    w.bytes(&list.data);
    Ok(())
}

/// One `TypedDataComponent`: the 1.21.1 type id and value, written as the
/// 26.3 id and value. Returns how many it wrote (0 when 26.3 has no such
/// component).
fn typed(r: &mut Reader, w: &mut Writer, depth: u32) -> Result<i32> {
    let kind = r.varint()?;
    let mut value = Writer::new();
    let Some(name) = value_767(kind, r, &mut value, depth)? else {
        return Ok(0);
    };
    let id =
        id_777(name).ok_or_else(|| Error::new(format!("26.3 has no item component {name}")))?;
    w.varint(id);
    w.bytes(&value.data);
    Ok(1)
}

/// Reads one 1.21.1 value of component type `kind` and writes it in 26.3's
/// codec. Returns the 26.3 component it became, or `None` when 26.3 has no
/// counterpart (the value is read and left out).
fn value_767(
    kind: i32,
    r: &mut Reader,
    w: &mut Writer,
    depth: u32,
) -> Result<Option<&'static str>> {
    let Some(name) = name_767(kind) else {
        // A mod's component: read past by the server's codec tree, if the
        // oracle exported one, and left out.
        if let Some(read) = codec_tree::skip_server(COMPONENT_REGISTRY, kind, r, depth) {
            read?;
            crate::translation::left_out_767(COMPONENT_REGISTRY, kind);
            return Ok(None);
        }
        crate::translation::unknown_767(COMPONENT_REGISTRY, kind);
        return Err(Error::new(format!(
            "no codec for 1.21.1 item component {kind}"
        )));
    };
    match name {
        // The same codec in both versions.
        "custom_data"
        | "custom_name"
        | "item_name"
        | "intangible_projectile"
        | "map_decorations"
        | "debug_stick_state"
        | "bucket_entity_data"
        | "recipes"
        | "container_loot" => copy(r, w, skip::nbt)?,
        "max_stack_size"
        | "max_damage"
        | "damage"
        | "rarity"
        | "repair_cost"
        | "map_id"
        | "map_post_processing"
        | "ominous_bottle_amplifier"
        | "base_color" => copy(r, w, skip::varint)?,
        "creative_slot_lock" => {}
        "enchantment_glint_override" => copy(r, w, skip::boolean)?,
        "lore" => copy(r, w, |r| skip::list(r, skip::nbt))?,
        "suspicious_stew_effects" => copy(r, w, |r| skip::list(r, varint_pair))?,
        "writable_book_content" => copy(r, w, |r| skip::list(r, |r| filterable(r, skip::string)))?,
        "written_book_content" => copy(r, w, written_book)?,
        "lodestone_tracker" => copy(r, w, lodestone)?,
        "firework_explosion" => copy(r, w, skip::firework_explosion)?,
        "fireworks" => copy(r, w, |r| {
            skip::varint(r)?;
            skip::list(r, skip::firework_explosion)
        })?,
        "note_block_sound" => copy(r, w, skip::ident)?,
        "banner_patterns" => copy(r, w, |r| skip::list(r, banner_layer))?,
        "block_state" => copy(r, w, |r| {
            skip::list(r, |r| {
                skip::string(r)?;
                skip::string(r)
            })
        })?,
        // 26.3 dropped the show_in_tooltip flags (TooltipDisplay hides
        // components now).
        "unbreakable" => skip::boolean(r)?,
        "enchantments" | "stored_enchantments" => {
            copy(r, w, |r| skip::list(r, varint_pair))?;
            skip::boolean(r)?;
        }
        "dyed_color" => {
            copy(r, w, skip::i32)?;
            skip::boolean(r)?;
        }
        "can_place_on" | "can_break" => {
            let n = r.count(r.remaining())?;
            w.varint(n as i32);
            for _ in 0..n {
                block_predicate(r, w)?;
            }
            skip::boolean(r)?;
        }
        "attribute_modifiers" => {
            let n = r.count(r.remaining())?;
            w.varint(n as i32);
            for _ in 0..n {
                copy(r, w, attribute_modifier)?;
                // ItemAttributeModifiers.Display: the default one.
                w.varint(0);
            }
            skip::boolean(r)?;
        }
        // The one int becomes the first of 26.3's floats (no flags, strings
        // or colours).
        "custom_model_data" => {
            let value = r.varint()?;
            w.varint(1);
            w.f32(value as f32);
            (0..3).for_each(|_| w.varint(0));
        }
        // A TooltipDisplay hiding the whole tooltip.
        "hide_tooltip" => {
            w.bool(true);
            w.varint(0);
            return Ok(Some("tooltip_display"));
        }
        // FoodProperties keeps nutrition, saturation and canAlwaysEat; the
        // eating time, leftover stack and effects are read and dropped (26.3
        // moved them to other components).
        "food" => {
            copy(r, w, |r| {
                skip::varint(r)?;
                skip::f32(r)?;
                skip::boolean(r)
            })?;
            skip::f32(r)?;
            if r.bool()? {
                template_at(r, &mut Writer::new(), depth)?;
            }
            skip::list(r, |r| {
                skip::mob_effect(r)?;
                skip::f32(r)
            })?;
        }
        // Tool, then 26.3's canDestroyBlocksInCreative (true by default).
        "tool" => {
            copy(r, w, tool)?;
            w.bool(true);
        }
        "charged_projectiles" | "bundle_contents" => {
            let n = r.count(r.remaining())?;
            w.varint(n as i32);
            for _ in 0..n {
                template_at(r, w, depth)?;
            }
        }
        // PotionContents, then 26.3's optional custom name.
        "potion_contents" => {
            copy(r, w, |r| {
                skip::optional(r, skip::varint)?;
                skip::optional(r, skip::i32)?;
                skip::list(r, skip::mob_effect)
            })?;
            w.bool(false);
        }
        // ItemContainerContents: optional stacks become optional templates.
        "container" => {
            let n = r.count(r.remaining())?;
            w.varint(n as i32);
            for _ in 0..n {
                let count = r.varint()?;
                w.bool(count > 0);
                if count > 0 {
                    template_rest(count, r, w, depth)?;
                }
            }
        }
        // ResolvableProfile: 26.3's Either takes the partial profile (the
        // same name, id and properties), then an empty PlayerSkin.Patch.
        "profile" => {
            w.u8(0);
            copy(r, w, profile)?;
            (0..4).for_each(|_| w.bool(false));
        }
        // ArmorTrim: the material and pattern holders, then the dropped
        // flag. A direct holder has no 26.3 counterpart.
        "trim" => {
            let material = reference(r, trim_material)?;
            let pattern = reference(r, trim_pattern)?;
            skip::boolean(r)?;
            let (Some(material), Some(pattern)) = (material, pattern) else {
                return Ok(None);
            };
            w.varint(material);
            w.varint(pattern);
        }
        // Holder<Instrument>: soundEvent, useDuration, range when direct.
        "instrument" => {
            let instrument = reference(r, |r| {
                skip::sound(r)?;
                skip::varint(r)?;
                skip::f32(r)
            })?;
            let Some(id) = instrument else {
                return Ok(None);
            };
            w.varint(id);
        }
        // JukeboxPlayable: an EitherHolder (true: a holder, false: the
        // song's key), then the dropped flag. 26.3 takes the holder alone.
        "jukebox_playable" => {
            let song = if r.bool()? {
                reference(r, jukebox_song)?
            } else {
                skip::ident(r)?;
                None
            };
            skip::boolean(r)?;
            let Some(id) = song else {
                return Ok(None);
            };
            w.varint(id);
        }
        // Gone in 26.3, or needing what 1.21.1 doesn't send (the entity or
        // block entity type id, a lock's item predicate): read, left out.
        "hide_additional_tooltip" | "fire_resistant" => return Ok(None),
        "map_color" => {
            skip::i32(r)?;
            return Ok(None);
        }
        "pot_decorations" => {
            skip::list(r, skip::varint)?;
            return Ok(None);
        }
        "entity_data" | "block_entity_data" | "lock" => {
            skip::nbt(r)?;
            return Ok(None);
        }
        "bees" => {
            skip::list(r, |r| {
                skip::nbt(r)?;
                skip::varint(r)?;
                skip::varint(r)
            })?;
            return Ok(None);
        }
        other => {
            return Err(Error::new(format!(
                "no codec for 1.21.1 item component {other}"
            )));
        }
    }
    Ok(Some(name))
}

/// Reads one value and writes its bytes unchanged.
fn copy(r: &mut Reader, w: &mut Writer, value: Read) -> Result<()> {
    let start = r.position();
    value(r)?;
    w.bytes(r.since(start));
    Ok(())
}

fn varint_pair(r: &mut Reader) -> Result<()> {
    skip::varint(r)?;
    skip::varint(r)
}

/// `Filterable.streamCodec`: the raw value, then an optional filtered one.
fn filterable(r: &mut Reader, value: Read) -> Result<()> {
    value(r)?;
    skip::optional(r, value)
}

/// WrittenBookContent: title, author, generation, pages, resolved.
fn written_book(r: &mut Reader) -> Result<()> {
    filterable(r, skip::string)?;
    skip::string(r)?;
    skip::varint(r)?;
    skip::list(r, |r| filterable(r, skip::nbt))?;
    skip::boolean(r)
}

/// LodestoneTracker: target (GlobalPos: dimension, BlockPos), tracked.
fn lodestone(r: &mut Reader) -> Result<()> {
    skip::optional(r, |r| {
        skip::ident(r)?;
        r.i64().map(drop)
    })?;
    skip::boolean(r)
}

/// BannerPatternLayers.Layer: Holder<BannerPattern> (assetId,
/// translationKey), then the dye.
fn banner_layer(r: &mut Reader) -> Result<()> {
    skip::holder(r, |r| {
        skip::ident(r)?;
        skip::string(r)
    })?;
    skip::varint(r)
}

/// 1.21.1's BlockPredicate (blocks, properties, nbt), then 26.3's
/// DataComponentMatchers: no exact components, no partial predicates.
fn block_predicate(r: &mut Reader, w: &mut Writer) -> Result<()> {
    copy(r, w, |r| {
        skip::optional(r, skip::holder_set)?;
        skip::optional(r, |r| skip::list(r, property_matcher))?;
        skip::optional(r, skip::nbt)
    })?;
    w.varint(0);
    w.varint(0);
    Ok(())
}

/// StatePropertiesPredicate.PropertyMatcher: the name, then an Either of an
/// exact value (true) or an optional min and max.
fn property_matcher(r: &mut Reader) -> Result<()> {
    skip::string(r)?;
    if r.bool()? {
        skip::string(r)
    } else {
        skip::optional(r, skip::string)?;
        skip::optional(r, skip::string)
    }
}

/// ItemAttributeModifiers.Entry: attribute, modifier (id, amount,
/// operation), slot group.
fn attribute_modifier(r: &mut Reader) -> Result<()> {
    skip::varint(r)?;
    skip::ident(r)?;
    r.f64()?;
    skip::varint(r)?;
    skip::varint(r)
}

/// Tool: rules (blocks, speed, correctForDrops), defaultMiningSpeed,
/// damagePerBlock.
fn tool(r: &mut Reader) -> Result<()> {
    skip::list(r, |r| {
        skip::holder_set(r)?;
        skip::optional(r, skip::f32)?;
        skip::optional(r, skip::boolean)
    })?;
    skip::f32(r)?;
    skip::varint(r)
}

/// ResolvableProfile: an optional name and id, then the properties (name,
/// value, optional signature).
fn profile(r: &mut Reader) -> Result<()> {
    skip::optional(r, skip::string)?;
    skip::optional(r, |r| r.uuid().map(drop))?;
    skip::list(r, |r| {
        skip::string(r)?;
        skip::string(r)?;
        skip::optional(r, skip::string)
    })
}

/// `ByteBufCodecs.holder`: a reference's registry id plus one (returned as
/// sent: 26.3 encodes it the same way), or 0 and the value itself (read;
/// `None`).
fn reference(r: &mut Reader, direct: Read) -> Result<Option<i32>> {
    match r.varint()? {
        0 => {
            direct(r)?;
            Ok(None)
        }
        id => Ok(Some(id)),
    }
}

/// TrimMaterial: assetName, ingredient, itemModelIndex,
/// overrideArmorMaterials, description.
fn trim_material(r: &mut Reader) -> Result<()> {
    skip::string(r)?;
    skip::varint(r)?;
    skip::f32(r)?;
    skip::list(r, |r| {
        skip::varint(r)?;
        skip::string(r)
    })?;
    skip::nbt(r)
}

/// TrimPattern: assetId, templateItem, description, decal.
fn trim_pattern(r: &mut Reader) -> Result<()> {
    skip::ident(r)?;
    skip::varint(r)?;
    skip::nbt(r)?;
    skip::boolean(r)
}

/// JukeboxSong: soundEvent, description, lengthInSeconds, comparatorOutput.
fn jukebox_song(r: &mut Reader) -> Result<()> {
    skip::sound(r)?;
    skip::nbt(r)?;
    skip::f32(r)?;
    skip::varint(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        buf::Codec,
        item::{Component, Slot, Stack, component},
    };

    /// A network NBT string tag (TAG_String, no root name).
    fn nbt_string(s: &str) -> Vec<u8> {
        let mut out = vec![8, 0, s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// A 1.21.1 optional stack: count, item, then a patch adding `added`
    /// (each a 1.21.1 type id and its value bytes) and removing `removed`.
    fn stack_767(count: u8, item: u8, added: &[(u8, Vec<u8>)], removed: &[u8]) -> Vec<u8> {
        let mut out = vec![count, item, added.len() as u8, removed.len() as u8];
        for (kind, value) in added {
            out.push(*kind);
            out.extend_from_slice(value);
        }
        out.extend_from_slice(removed);
        out
    }

    fn convert(input: &[u8]) -> Vec<u8> {
        let mut r = Reader::new(input);
        let mut w = Writer::new();
        slot(&mut r, &mut w).unwrap();
        assert_eq!(r.remaining(), 0, "the whole 1.21.1 stack is read");
        w.data
    }

    /// The converted bytes decode with the 26.3 reader, to the end.
    fn native(bytes: &[u8]) -> Stack {
        let mut r = Reader::new(bytes);
        let stack = Slot::read(&mut r).unwrap().expect("a stack");
        assert_eq!(r.remaining(), 0);
        stack
    }

    #[test]
    fn component_names_map_to_their_26_3_ids() {
        assert_eq!(renumber(0), Some(component::CUSTOM_DATA));
        assert_eq!(renumber(4), Some(component::UNBREAKABLE));
        assert_eq!(renumber(5), Some(component::CUSTOM_NAME));
        assert_eq!(renumber(9), Some(component::ENCHANTMENTS));
        assert_eq!(renumber(15), Some(component::TOOLTIP_DISPLAY));
        assert_eq!(renumber(23), Some(component::STORED_ENCHANTMENTS));
        assert_eq!(renumber(31), Some(component::POTION_CONTENTS));
        assert_eq!(renumber(47), Some(component::PROFILE));
        assert_eq!(renumber(52), Some(component::CONTAINER));
        assert_eq!(renumber(56), Some(82));
        for gone in [14, 21, 25] {
            assert_eq!(renumber(gone), None, "{}", NAMES_767[gone as usize]);
        }
        // Every other 1.21.1 component has a 26.3 namesake.
        let gone = [
            "hide_additional_tooltip",
            "hide_tooltip",
            "fire_resistant",
            "map_color",
        ];
        for name in NAMES_767.iter().filter(|n| !gone.contains(n)) {
            assert!(id_777(name).is_some(), "{name}");
        }
        assert_eq!(renumber(57), None);
    }

    /// The DUMP case: set_equipment carried 1.21.1 component 31. Body:
    /// optional potion id, optional colour, effects; 26.3 adds the name.
    #[test]
    fn potion_contents_gains_an_absent_custom_name() {
        let input = stack_767(1, 100, &[(31, vec![1, 5, 0, 0])], &[]);
        let out = convert(&input);
        assert_eq!(out, [1, 100, 1, 0, 53, 1, 5, 0, 0, 0]);
        let stack = native(&out);
        assert_eq!(stack.item, 100);
        assert_eq!(
            stack.component(component::POTION_CONTENTS),
            Some(&Component::Potion {
                potion: Some(5),
                color: None,
                rest: Vec::new(),
            })
        );
    }

    #[test]
    fn potion_effects_are_kept() {
        // Effect 3: amplifier 1, duration 600, ambient, particles, icon, no
        // hidden effect; colour 0x00FF00.
        let mut body = vec![0, 1, 0, 0, 0xFF, 0, 1, 3, 1, 0xD8, 0x04, 0, 1, 1, 0];
        let input = stack_767(1, 7, &[(31, body.clone())], &[]);
        let out = convert(&input);
        body.push(0);
        let mut expected = vec![1, 7, 1, 0, 53];
        expected.extend_from_slice(&body);
        assert_eq!(out, expected);
        native(&out);
    }

    #[test]
    fn show_in_tooltip_flags_are_dropped() {
        let input = stack_767(
            1,
            9,
            &[
                // unbreakable: showInTooltip.
                (4, vec![1]),
                // enchantments: {12: 3}, showInTooltip.
                (9, vec![1, 12, 3, 1]),
                // dyed_color: rgb, showInTooltip.
                (24, vec![0, 0x11, 0x22, 0x33, 0]),
                // stored_enchantments: {7: 1}, hidden.
                (23, vec![1, 7, 1, 0]),
            ],
            &[],
        );
        let out = convert(&input);
        assert_eq!(
            out,
            [
                1, 9, 4, 0, 4, 13, 1, 12, 3, 47, 0, 0x11, 0x22, 0x33, 45, 1, 7, 1
            ]
        );
        let stack = native(&out);
        assert_eq!(
            stack.component(component::UNBREAKABLE),
            Some(&Component::Unit)
        );
        assert_eq!(
            stack.component(component::ENCHANTMENTS),
            Some(&Component::Enchantments(vec![(12, 3)]))
        );
        assert_eq!(
            stack.component(component::DYED_COLOR),
            Some(&Component::Int(0x112233))
        );
    }

    #[test]
    fn renumbered_components_keep_their_values() {
        let name = nbt_string("Sword");
        let input = stack_767(
            1,
            3,
            &[
                (5, name.clone()),
                (3, vec![25]),
                (16, vec![4]),
                (8, vec![2]),
            ],
            &[],
        );
        let out = convert(&input);
        let mut expected = vec![1, 3, 4, 0, 6];
        expected.extend_from_slice(&name);
        expected.extend_from_slice(&[3, 25, 19, 4, 12, 2]);
        assert_eq!(out, expected);
        let stack = native(&out);
        assert_eq!(stack.damage(), 25);
        assert_eq!(
            stack.component(component::REPAIR_COST),
            Some(&Component::Int(4))
        );
    }

    #[test]
    fn removed_components_are_renumbered_or_left_out() {
        // Removes enchantments (9 -> 13), fire_resistant (gone) and a mod's
        // 300 (a two-byte VarInt).
        let out = convert(&[1, 4, 0, 3, 9, 21, 0xAC, 0x02]);
        assert_eq!(out, [1, 4, 0, 1, 13]);
        assert_eq!(native(&out).removed, vec![13]);
    }

    #[test]
    fn components_26_3_lacks_are_read_and_left_out() {
        let input = stack_767(
            1,
            5,
            &[
                (14, vec![]),
                (21, vec![]),
                (25, vec![0, 0, 0, 7]),
                (51, vec![4, 1, 2, 3, 4]),
                (37, nbt_string("x")),
                (54, {
                    let mut bee = vec![1];
                    bee.extend_from_slice(&nbt_string("b"));
                    bee.extend_from_slice(&[10, 20]);
                    bee
                }),
                (55, nbt_string("key")),
                (1, vec![16]),
            ],
            &[],
        );
        let out = convert(&input);
        assert_eq!(out, [1, 5, 1, 0, 1, 16]);
    }

    #[test]
    fn hide_tooltip_becomes_a_tooltip_display() {
        let out = convert(&stack_767(1, 2, &[(15, vec![])], &[]));
        assert_eq!(out, [1, 2, 1, 0, 18, 1, 0]);
        assert_eq!(
            native(&out).component(component::TOOLTIP_DISPLAY),
            Some(&Component::TooltipDisplay {
                hide: true,
                hidden: Vec::new(),
            })
        );
    }

    #[test]
    fn custom_model_data_int_becomes_the_first_float() {
        let out = convert(&stack_767(1, 2, &[(13, vec![5])], &[]));
        let mut expected = vec![1, 2, 1, 0, 17, 1];
        expected.extend_from_slice(&5.0f32.to_be_bytes());
        expected.extend_from_slice(&[0, 0, 0]);
        assert_eq!(out, expected);
        assert_eq!(
            native(&out).component(component::CUSTOM_MODEL_DATA),
            Some(&Component::CustomModelData {
                floats: vec![5.0],
                flags: Vec::new(),
                strings: Vec::new(),
                colors: Vec::new(),
            })
        );
    }

    #[test]
    fn bundle_and_container_stacks_become_templates() {
        // bundle_contents: one stack (count 2 of item 7, an enchantment).
        let bundle = vec![1, 2, 7, 1, 0, 9, 1, 12, 3, 1];
        // container: an empty slot, then 1 of item 8.
        let container = vec![2, 0, 1, 8, 0, 0];
        let out = convert(&stack_767(1, 6, &[(30, bundle), (52, container)], &[]));
        assert_eq!(
            out,
            [
                1, 6, 2, 0, 52, 1, 7, 2, 1, 0, 13, 1, 12, 3, 77, 2, 0, 1, 8, 1, 0, 0
            ]
        );
        let stack = native(&out);
        let Some(Component::Stacks(inner)) = stack.component(component::BUNDLE_CONTENTS) else {
            panic!("bundle contents");
        };
        assert_eq!((inner[0].item, inner[0].count), (7, 2));
        assert_eq!(
            inner[0].component(component::ENCHANTMENTS),
            Some(&Component::Enchantments(vec![(12, 3)]))
        );
    }

    #[test]
    fn food_keeps_nutrition_saturation_and_always_eat() {
        let mut body = vec![4];
        body.extend_from_slice(&2.4f32.to_be_bytes());
        body.push(0);
        // eatSeconds, a leftover bowl (1 of item 40), one possible effect
        // (effect 2, amplifier 0, 100 ticks, flags, no hidden) at chance 1.
        body.extend_from_slice(&1.6f32.to_be_bytes());
        body.extend_from_slice(&[1, 1, 40, 0, 0]);
        body.extend_from_slice(&[1, 2, 0, 100, 0, 1, 1, 0]);
        body.extend_from_slice(&1.0f32.to_be_bytes());
        let out = convert(&stack_767(1, 9, &[(20, body)], &[]));
        let mut expected = vec![1, 9, 1, 0, 23, 4];
        expected.extend_from_slice(&2.4f32.to_be_bytes());
        expected.push(0);
        assert_eq!(out, expected);
        native(&out);
    }

    #[test]
    fn tool_gains_the_creative_flag() {
        let tag = b"minecraft:mineable/pickaxe";
        let mut body = vec![1, 0, tag.len() as u8];
        body.extend_from_slice(tag);
        body.push(1);
        body.extend_from_slice(&6.0f32.to_be_bytes());
        body.extend_from_slice(&[1, 1]);
        body.extend_from_slice(&1.0f32.to_be_bytes());
        body.push(1);
        let out = convert(&stack_767(1, 9, &[(22, body.clone())], &[]));
        let mut expected = vec![1, 9, 1, 0, 28];
        expected.extend_from_slice(&body);
        expected.push(1);
        assert_eq!(out, expected);
        native(&out);
    }

    #[test]
    fn attribute_modifiers_gain_the_default_display() {
        let id = b"minecraft:base_attack_damage";
        let mut entry = vec![1, id.len() as u8];
        entry.extend_from_slice(id);
        entry.extend_from_slice(&5.0f64.to_be_bytes());
        entry.extend_from_slice(&[0, 1]);
        let mut body = vec![1];
        body.extend_from_slice(&entry);
        body.push(1);
        let out = convert(&stack_767(1, 9, &[(12, body)], &[]));
        let mut expected = vec![1, 9, 1, 0, 16, 1];
        expected.extend_from_slice(&entry);
        expected.push(0);
        assert_eq!(out, expected);
        native(&out);
    }

    #[test]
    fn adventure_predicates_gain_empty_component_matchers() {
        // One predicate: blocks #minecraft:logs, properties axis=y, no nbt;
        // then showInTooltip.
        let tag = b"minecraft:logs";
        let mut predicate = vec![1, 0, tag.len() as u8];
        predicate.extend_from_slice(tag);
        predicate.extend_from_slice(&[1, 1, 4]);
        predicate.extend_from_slice(b"axis");
        predicate.extend_from_slice(&[1, 1, b'y', 0]);
        let mut body = vec![1];
        body.extend_from_slice(&predicate);
        body.push(1);
        let out = convert(&stack_767(1, 9, &[(11, body)], &[]));
        let mut expected = vec![1, 9, 1, 0, 15, 1];
        expected.extend_from_slice(&predicate);
        expected.extend_from_slice(&[0, 0]);
        assert_eq!(out, expected);
        native(&out);
    }

    #[test]
    fn profile_becomes_the_partial_either() {
        let body = vec![1, 3, b'A', b'b', b'c', 0, 0];
        let out = convert(&stack_767(1, 9, &[(47, body)], &[]));
        assert_eq!(
            out,
            [1, 9, 1, 0, 72, 0, 1, 3, b'A', b'b', b'c', 0, 0, 0, 0, 0, 0]
        );
        let Some(Component::Profile(p)) = native(&out).component(component::PROFILE).cloned()
        else {
            panic!("profile");
        };
        assert_eq!(p.name.as_deref(), Some("Abc"));
        assert_eq!(p.id, None);
    }

    #[test]
    fn holder_references_stay_and_direct_holders_drop() {
        // trim: material 3 and pattern 5 (ids + 1), showInTooltip.
        let out = convert(&stack_767(1, 9, &[(35, vec![4, 6, 1])], &[]));
        assert_eq!(out, [1, 9, 1, 0, 58, 4, 6]);
        assert_eq!(
            native(&out).component(component::TRIM),
            Some(&Component::Trim {
                material: 3,
                pattern: 5,
            })
        );
        // A direct material: read through, the trim left out.
        let mut direct = vec![0, 4];
        direct.extend_from_slice(b"gold");
        direct.extend_from_slice(&[17]);
        direct.extend_from_slice(&0.5f32.to_be_bytes());
        direct.push(0);
        direct.extend_from_slice(&nbt_string("Gold"));
        direct.extend_from_slice(&[6, 0]);
        assert_eq!(
            convert(&stack_767(1, 9, &[(35, direct)], &[])),
            [1, 9, 0, 0]
        );
        // jukebox_playable: a holder reference (song 2), showInTooltip.
        let out = convert(&stack_767(1, 9, &[(42, vec![1, 3, 1])], &[]));
        assert_eq!(out, [1, 9, 1, 0, 66, 3]);
        native(&out);
        // jukebox_playable by key: left out.
        let mut by_key = vec![0, 13];
        by_key.extend_from_slice(b"minecraft:cat");
        by_key.push(0);
        assert_eq!(
            convert(&stack_767(1, 9, &[(42, by_key)], &[])),
            [1, 9, 0, 0]
        );
        // instrument: a reference (id 1).
        let out = convert(&stack_767(1, 9, &[(40, vec![2])], &[]));
        assert_eq!(out, [1, 9, 1, 0, 63, 2]);
        assert_eq!(
            native(&out).component(component::INSTRUMENT),
            Some(&Component::Instrument(1))
        );
    }

    #[test]
    fn a_mod_component_fails_the_stack() {
        let input = stack_767(1, 9, &[(57, vec![1])], &[]);
        let mut w = Writer::new();
        assert!(slot(&mut Reader::new(&input), &mut w).is_err());
    }

    #[test]
    fn a_mod_component_with_a_server_codec_tree_is_left_out() {
        // Mod component 100: a charge (VarInt) and a name; 101: a pouch of stacks.
        let tsv = "minecraft:data_component_type\t100\tmod:charge\t{\"seq\":[\"varint\",\"string\"]}\n\
                   minecraft:data_component_type\t101\tmod:pouch\t{\"list\":\"item\"}\n";
        codec_tree::with_test_tables(tsv, || {
            // The charge, then max_stack_size (1) 16 after it; the charge removed too.
            let charge = vec![0x80, 0x01, 2, b'h', b'i'];
            let out = convert(&stack_767(1, 9, &[(100, charge), (1, vec![16])], &[100]));
            assert_eq!(out, [1, 9, 1, 0, 1, 16]);
            assert_eq!(
                native(&out).component(component::MAX_STACK_SIZE),
                Some(&Component::Int(16))
            );
            // A pouch holding a stack with a charge of its own.
            let mut pouch = vec![1];
            pouch.extend_from_slice(&stack_767(1, 5, &[(100, vec![3, 0])], &[]));
            assert_eq!(
                convert(&stack_767(1, 9, &[(101, pouch)], &[])),
                [1, 9, 0, 0]
            );
            // Bytes the tree can't read still fail the stack.
            let short = stack_767(1, 9, &[(100, vec![3, 9])], &[]);
            assert!(slot(&mut Reader::new(&short), &mut Writer::new()).is_err());
        });
    }

    #[test]
    fn templates_put_the_item_first_and_refuse_empty_stacks() {
        let mut w = Writer::new();
        template(&mut Reader::new(&[3, 44, 0, 0]), &mut w).unwrap();
        assert_eq!(w.data, [44, 3, 0, 0]);
        assert!(template(&mut Reader::new(&[0]), &mut Writer::new()).is_err());
        assert_eq!(convert(&[0]), [0]);
    }

    #[test]
    fn exact_components_are_renumbered() {
        // An ItemCost's predicate: custom_name, fire_resistant (left out).
        let name = nbt_string("Gem");
        let mut input = vec![2, 5];
        input.extend_from_slice(&name);
        input.push(21);
        let mut w = Writer::new();
        exact_components(&mut Reader::new(&input), &mut w).unwrap();
        let mut expected = vec![1, 6];
        expected.extend_from_slice(&name);
        assert_eq!(w.data, expected);
    }
}
