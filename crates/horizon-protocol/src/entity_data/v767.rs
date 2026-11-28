//! 1.21.1 (protocol 767) synched entity data and particles in the 26.3
//! shape, after EntityDataSerializers and ParticleTypes of both versions
//! (javap of the 1.21.1 client NeoForge 21.1 runs on, and of the 26.3
//! client). Particle ids are renumbered by name to 26.3's (the client names
//! them with its own table); every other id inside a value stays the server's.
use super::serializer as s;
use crate::{
    Error, Result,
    buf::{Reader, Writer},
    codec_tree,
    item::v767 as items,
    translation::{left_out_767, unknown_767, warn_once},
};

/// NeoForge numbers its own registry's serializers from 256 on the wire
/// (CommonHooks.getSerializerId: VANILLA_SERIALIZER_LIMIT plus the
/// `neoforge:entity_data_serializers` id).
const MODDED_SERIALIZERS: i32 = 256;

/// `minecraft:particle_type` in 1.21.1's registration order: the server's
/// ids 0-108 (mods register theirs after them).
const PARTICLES_767: [&str; 109] = [
    "angry_villager",
    "block",
    "block_marker",
    "bubble",
    "cloud",
    "crit",
    "damage_indicator",
    "dragon_breath",
    "dripping_lava",
    "falling_lava",
    "landing_lava",
    "dripping_water",
    "falling_water",
    "dust",
    "dust_color_transition",
    "effect",
    "elder_guardian",
    "enchanted_hit",
    "enchant",
    "end_rod",
    "entity_effect",
    "explosion_emitter",
    "explosion",
    "gust",
    "small_gust",
    "gust_emitter_large",
    "gust_emitter_small",
    "sonic_boom",
    "falling_dust",
    "firework",
    "fishing",
    "flame",
    "infested",
    "cherry_leaves",
    "sculk_soul",
    "sculk_charge",
    "sculk_charge_pop",
    "soul_fire_flame",
    "soul",
    "flash",
    "happy_villager",
    "composter",
    "heart",
    "instant_effect",
    "item",
    "vibration",
    "item_slime",
    "item_cobweb",
    "item_snowball",
    "large_smoke",
    "lava",
    "mycelium",
    "note",
    "poof",
    "portal",
    "rain",
    "smoke",
    "white_smoke",
    "sneeze",
    "spit",
    "squid_ink",
    "sweep_attack",
    "totem_of_undying",
    "underwater",
    "splash",
    "witch",
    "bubble_pop",
    "current_down",
    "bubble_column_up",
    "nautilus",
    "dolphin",
    "campfire_cosy_smoke",
    "campfire_signal_smoke",
    "dripping_honey",
    "falling_honey",
    "landing_honey",
    "falling_nectar",
    "falling_spore_blossom",
    "ash",
    "crimson_spore",
    "warped_spore",
    "spore_blossom_air",
    "dripping_obsidian_tear",
    "falling_obsidian_tear",
    "landing_obsidian_tear",
    "reverse_portal",
    "white_ash",
    "small_flame",
    "snowflake",
    "dripping_dripstone_lava",
    "falling_dripstone_lava",
    "dripping_dripstone_water",
    "falling_dripstone_water",
    "glow_squid_ink",
    "glow",
    "wax_on",
    "wax_off",
    "electric_spark",
    "scrape",
    "shriek",
    "egg_crack",
    "dust_plume",
    "trial_spawner_detection",
    "trial_spawner_detection_ominous",
    "vault_connection",
    "dust_pillar",
    "ominous_spawning",
    "raid_omen",
    "trial_omen",
];

/// `minecraft:particle_type` in 26.3's registration order (the order
/// `Particle::read` decodes by).
const PARTICLES_777: [&str; 128] = [
    "angry_villager",
    "block",
    "block_marker",
    "bubble",
    "sulfur_bubbles",
    "noxious_gas",
    "noxious_gas_cloud",
    "geyser",
    "geyser_base",
    "geyser_poof",
    "geyser_plume",
    "cloud",
    "copper_fire_flame",
    "crit",
    "damage_indicator",
    "dragon_breath",
    "dripping_lava",
    "falling_lava",
    "landing_lava",
    "dripping_water",
    "falling_water",
    "dust",
    "dust_color_transition",
    "effect",
    "elder_guardian",
    "enchanted_hit",
    "enchant",
    "end_rod",
    "entity_effect",
    "explosion_emitter",
    "explosion",
    "gust",
    "small_gust",
    "gust_emitter_large",
    "gust_emitter_small",
    "sonic_boom",
    "falling_dust",
    "firework",
    "fishing",
    "flame",
    "infested",
    "cherry_leaves",
    "pale_oak_leaves",
    "red_poplar_leaves",
    "orange_poplar_leaves",
    "yellow_poplar_leaves",
    "tinted_leaves",
    "sculk_soul",
    "sculk_charge",
    "sculk_charge_pop",
    "soul_fire_flame",
    "soul",
    "flash",
    "happy_villager",
    "composter",
    "heart",
    "instant_effect",
    "item",
    "vibration",
    "trail",
    "pause_mob_growth",
    "reset_mob_growth",
    "item_slime",
    "item_cobweb",
    "item_snowball",
    "large_smoke",
    "lava",
    "mycelium",
    "note",
    "poof",
    "portal",
    "rain",
    "smoke",
    "white_smoke",
    "sneeze",
    "spit",
    "squid_ink",
    "sweep_attack",
    "totem_of_undying",
    "underwater",
    "splash",
    "witch",
    "bubble_pop",
    "current_down",
    "bubble_column_up",
    "nautilus",
    "dolphin",
    "campfire_cosy_smoke",
    "campfire_signal_smoke",
    "dripping_honey",
    "falling_honey",
    "landing_honey",
    "falling_nectar",
    "falling_spore_blossom",
    "ash",
    "crimson_spore",
    "warped_spore",
    "spore_blossom_air",
    "dripping_obsidian_tear",
    "falling_obsidian_tear",
    "landing_obsidian_tear",
    "reverse_portal",
    "white_ash",
    "small_flame",
    "snowflake",
    "dripping_dripstone_lava",
    "falling_dripstone_lava",
    "dripping_dripstone_water",
    "falling_dripstone_water",
    "glow_squid_ink",
    "glow",
    "wax_on",
    "wax_off",
    "electric_spark",
    "scrape",
    "shriek",
    "egg_crack",
    "dust_plume",
    "trial_spawner_detection",
    "trial_spawner_detection_ominous",
    "vault_connection",
    "dust_pillar",
    "ominous_spawning",
    "raid_omen",
    "trial_omen",
    "block_crumble",
    "firefly",
    "sulfur_cube_goo",
];

/// A synched entity data list, from its first entry to the 0xFF end, in
/// 26.3's numbering. An entry 26.3 has no serializer for is left out, as is a
/// mod's serializer or particle the server's codec tree (crate::codec_tree)
/// reads past; one Horizon can't read (a mod's serializer, item component or
/// particle without a tree) ends the list there, keeping the entries before it.
pub(crate) fn values(r: &mut Reader, w: &mut Writer) {
    if let Err(e) = entries(r, w) {
        warn_once(format!(
            "Horizon: 1.21.1 entity data cut short (later values dropped): {e}"
        ));
        r.rest();
    }
    w.u8(0xFF);
}

fn entries(r: &mut Reader, w: &mut Writer) -> Result<()> {
    loop {
        let index = r.u8()?;
        if index == 0xFF {
            return Ok(());
        }
        let kind = r.varint()?;
        let mut value = Writer::new();
        if let Some(serializer) = value_767(kind, r, &mut value)? {
            w.u8(index);
            w.varint(serializer);
            w.bytes(&value.data);
        }
    }
}

/// Reads one value of 1.21.1 serializer `kind` and writes it in 26.3's
/// codec. Returns the 26.3 serializer id, or `None` when the entry has no
/// 26.3 form (read and left out).
fn value_767(kind: i32, r: &mut Reader, w: &mut Writer) -> Result<Option<i32>> {
    Ok(Some(match kind {
        0 => copy(r, w, |r| r.u8().map(drop), s::BYTE)?,
        1 => copy(r, w, varint, s::INT)?,
        2 => copy(r, w, |r| r.varlong().map(drop), s::LONG)?,
        3 => copy(r, w, f32, s::FLOAT)?,
        4 => copy(r, w, |r| r.string(32767).map(drop), s::STRING)?,
        5 => copy(r, w, nbt, s::COMPONENT)?,
        6 => copy(r, w, |r| optional(r, nbt), s::OPTIONAL_COMPONENT)?,
        7 => {
            items::slot(r, w)?;
            s::ITEM_STACK
        }
        8 => copy(r, w, |r| r.bool().map(drop), s::BOOLEAN)?,
        9 => copy(r, w, |r| (0..3).try_for_each(|_| f32(r)), s::ROTATIONS)?,
        10 => copy(r, w, i64, s::BLOCK_POS)?,
        11 => copy(r, w, |r| optional(r, i64), s::OPTIONAL_BLOCK_POS)?,
        12 => copy(r, w, varint, s::DIRECTION)?,
        // OPTIONAL_UUID: 26.3's living entity reference has the same codec.
        13 => copy(
            r,
            w,
            |r| optional(r, |r| r.uuid().map(drop)),
            s::OPTIONAL_LIVING_ENTITY_REFERENCE,
        )?,
        14 => copy(r, w, varint, s::BLOCK_STATE)?,
        15 => copy(r, w, varint, s::OPTIONAL_BLOCK_STATE)?,
        // COMPOUND_TAG: no 26.3 serializer.
        16 => {
            nbt(r)?;
            return Ok(None);
        }
        // A mod's particle read past by the server's codec tree: the entry,
        // or that particle of the list, is left out.
        17 => {
            let id = r.varint()?;
            if skipped_mod_particle(id, r)? {
                return Ok(None);
            }
            particle_of(id, r, w)?;
            s::PARTICLE
        }
        18 => {
            let n = r.count(r.remaining())?;
            let mut list = Writer::new();
            let mut kept = 0;
            for _ in 0..n {
                let id = r.varint()?;
                if !skipped_mod_particle(id, r)? {
                    particle_of(id, r, &mut list)?;
                    kept += 1;
                }
            }
            w.varint(kept);
            w.bytes(&list.data);
            s::PARTICLES
        }
        // VillagerData: type, profession, level.
        19 => copy(
            r,
            w,
            |r| (0..3).try_for_each(|_| varint(r)),
            s::VILLAGER_DATA,
        )?,
        20 => copy(r, w, varint, s::OPTIONAL_UNSIGNED_INT)?,
        21 => copy(r, w, varint, s::POSE)?,
        22 => copy(r, w, varint, s::CAT_VARIANT)?,
        // WolfVariant is a holder in 1.21.1 (a reference's id plus one, or
        // 0 and wildTexture, tameTexture, angryTexture, biomes) and a plain
        // registry id in 26.3.
        23 => match r.varint()? {
            0 => {
                (0..3).try_for_each(|_| string(r))?;
                holder_set(r)?;
                return Ok(None);
            }
            id => {
                w.varint(id - 1);
                s::WOLF_VARIANT
            }
        },
        24 => copy(r, w, varint, s::FROG_VARIANT)?,
        // GlobalPos: dimension, BlockPos.
        25 => copy(
            r,
            w,
            |r| {
                optional(r, |r| {
                    string(r)?;
                    i64(r)
                })
            },
            s::OPTIONAL_GLOBAL_POS,
        )?,
        // Holder<PaintingVariant> in both; a direct one (width, height,
        // assetId) is left out: 26.3's reader takes a reference.
        26 => match r.varint()? {
            0 => {
                varint(r)?;
                varint(r)?;
                string(r)?;
                return Ok(None);
            }
            id => {
                w.varint(id);
                s::PAINTING_VARIANT
            }
        },
        27 => copy(r, w, varint, s::SNIFFER_STATE)?,
        28 => copy(r, w, varint, s::ARMADILLO_STATE)?,
        29 => copy(r, w, |r| (0..3).try_for_each(|_| f32(r)), s::VECTOR3)?,
        30 => copy(r, w, |r| (0..4).try_for_each(|_| f32(r)), s::QUATERNION)?,
        MODDED_SERIALIZERS.. => {
            let id = kind - MODDED_SERIALIZERS;
            if let Some(read) = codec_tree::skip_server(codec_tree::SERIALIZERS, id, r, 0) {
                read?;
                left_out_767(codec_tree::SERIALIZERS, id);
                return Ok(None);
            }
            unknown_767(
                "neoforge:entity_data_serializers",
                kind - MODDED_SERIALIZERS,
            );
            return Err(Error::new(format!(
                "no codec for NeoForge entity data serializer {}",
                kind - MODDED_SERIALIZERS
            )));
        }
        other => {
            unknown_767("minecraft:entity_data_serializer", other);
            return Err(Error::new(format!(
                "no codec for 1.21.1 entity data serializer {other}"
            )));
        }
    }))
}

/// Where a mod's particle ids start once a LevelParticles packet is translated: the server's id
/// plus this, far above 26.3's own 128 types. The options are dropped.
pub const MODDED_PARTICLE_BASE: i32 = 1 << 16;

/// The server's `minecraft:particle_type` id behind a reserved id, if it is one.
pub fn reserved_particle_server_id(id: i32) -> Option<i32> {
    id.checked_sub(MODDED_PARTICLE_BASE).filter(|server| *server >= 0)
}

/// [`particle`] for a particle that is the last field of its packet: a mod's type (a server id
/// past 1.21.1's) becomes a reserved id with no options, as its options can't be read to skip.
pub(crate) fn trailing_particle(rest: &[u8], w: &mut Writer) -> Result<()> {
    let id = Reader::new(rest).varint()?;
    if usize::try_from(id).is_ok_and(|i| i >= PARTICLES_767.len()) {
        w.varint(MODDED_PARTICLE_BASE.saturating_add(id));
        return Ok(());
    }
    particle(&mut Reader::new(rest), w)
}

/// `ParticleTypes.STREAM_CODEC` (1.21.1) to 26.3's: the type id renumbered
/// by name, then its options in 26.3's codec.
pub(crate) fn particle(r: &mut Reader, w: &mut Writer) -> Result<()> {
    let id = r.varint()?;
    particle_of(id, r, w)
}

/// Whether particle `id` (its id already read) is a mod's the server's codec
/// tree read the options of.
fn skipped_mod_particle(id: i32, r: &mut Reader) -> Result<bool> {
    if usize::try_from(id).is_ok_and(|i| i < PARTICLES_767.len()) {
        return Ok(false);
    }
    match codec_tree::skip_server(codec_tree::PARTICLES, id, r, 0) {
        Some(read) => {
            read?;
            left_out_767(codec_tree::PARTICLES, id);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// [`particle`] after its type id.
fn particle_of(id: i32, r: &mut Reader, w: &mut Writer) -> Result<()> {
    let Some(name) = usize::try_from(id)
        .ok()
        .and_then(|i| PARTICLES_767.get(i))
        .copied()
    else {
        unknown_767("minecraft:particle_type", id);
        return Err(Error::new(format!("no codec for 1.21.1 particle {id}")));
    };
    let target = PARTICLES_777
        .iter()
        .position(|n| *n == name)
        .ok_or_else(|| Error::new(format!("26.3 has no particle {name}")))?;
    w.varint(target as i32);
    match name {
        // BlockParticleOption: a block state id.
        "block" | "block_marker" | "falling_dust" | "dust_pillar" => w.varint(r.varint()?),
        // DustParticleOptions: the colour's three floats become an RGB int,
        // then the scale.
        "dust" => {
            let color = rgb(r)?;
            w.i32(color);
            w.f32(r.f32()?);
        }
        // DustColorTransitionOptions: from, to, scale.
        "dust_color_transition" => {
            let from = rgb(r)?;
            let to = rgb(r)?;
            w.i32(from);
            w.i32(to);
            w.f32(r.f32()?);
        }
        // ColorParticleOption: ARGB.
        "entity_effect" => w.i32(r.i32()?),
        // SculkChargeParticleOptions: the roll.
        "sculk_charge" => w.f32(r.f32()?),
        // ShriekParticleOption: the delay.
        "shriek" => w.varint(r.varint()?),
        // ItemParticleOption: a stack (count first) becomes a template.
        "item" => items::template(r, w)?,
        // VibrationParticleOption: PositionSource (block 0: a BlockPos;
        // entity 1: its id and y offset), then the arrival time.
        "vibration" => {
            let source = r.varint()?;
            w.varint(source);
            match source {
                0 => w.i64(r.i64()?),
                1 => {
                    w.varint(r.varint()?);
                    w.f32(r.f32()?);
                }
                other => {
                    return Err(Error::new(format!("1.21.1 position source type {other}")));
                }
            }
            w.varint(r.varint()?);
        }
        // Simple types in 1.21.1 that carry options in 26.3: vanilla's
        // defaults (PowerParticleOption power 1; white SpellParticleOption
        // of power 1; white ColorParticleOption).
        "dragon_breath" => w.f32(1.0),
        "effect" | "instant_effect" => {
            w.i32(-1);
            w.f32(1.0);
        }
        "flash" => w.i32(-1),
        _ => {}
    }
    Ok(())
}

/// A colour as three floats (ByteBufCodecs.VECTOR3F) to an opaque RGB int
/// (`ARGB.colorFromFloat(1, r, g, b)`: each channel floor(v * 255)).
fn rgb(r: &mut Reader) -> Result<i32> {
    let mut argb = 0xFF_u32;
    for _ in 0..3 {
        let channel = (r.f32()? * 255.0).floor().clamp(0.0, 255.0) as u32;
        argb = (argb << 8) | channel;
    }
    Ok(argb as i32)
}

/// Reads one value, writes its bytes unchanged and returns `serializer`.
fn copy(
    r: &mut Reader,
    w: &mut Writer,
    value: fn(&mut Reader) -> Result<()>,
    serializer: i32,
) -> Result<i32> {
    let start = r.position();
    value(r)?;
    w.bytes(r.since(start));
    Ok(serializer)
}

fn varint(r: &mut Reader) -> Result<()> {
    r.varint().map(drop)
}
fn f32(r: &mut Reader) -> Result<()> {
    r.f32().map(drop)
}
fn i64(r: &mut Reader) -> Result<()> {
    r.i64().map(drop)
}
fn string(r: &mut Reader) -> Result<()> {
    r.string(32767).map(drop)
}
fn nbt(r: &mut Reader) -> Result<()> {
    r.nbt().map(drop)
}
fn optional(r: &mut Reader, value: fn(&mut Reader) -> Result<()>) -> Result<()> {
    if r.bool()? { value(r) } else { Ok(()) }
}
/// ByteBufCodecs.holderSet: 0 and a tag id, or the size plus one and the ids.
fn holder_set(r: &mut Reader) -> Result<()> {
    match r.varint()? {
        0 => string(r),
        n => (1..n).try_for_each(|_| varint(r)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        buf::Codec,
        entity_data::{EntityData, Particle, ParticleData, Value},
    };

    fn convert(input: &[u8]) -> Vec<u8> {
        let mut r = Reader::new(input);
        let mut w = Writer::new();
        values(&mut r, &mut w);
        assert_eq!(r.remaining(), 0);
        w.data
    }

    fn native(bytes: &[u8]) -> Vec<(u8, i32, Value)> {
        let mut r = Reader::new(bytes);
        let out = EntityData::read(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        out
    }

    fn particle_777(input: &[u8]) -> Vec<u8> {
        let mut r = Reader::new(input);
        let mut w = Writer::new();
        particle(&mut r, &mut w).unwrap();
        assert_eq!(r.remaining(), 0);
        w.data
    }

    #[test]
    fn serializers_are_renumbered_as_registered() {
        // A villager: shared flags (byte), villager data (19 -> 18), pose
        // (21 -> 20), an optional unsigned int (20 -> 19), a vector (29 -> 39).
        let mut input = vec![0, 0, 0x02, 18, 19, 2, 5, 1, 6, 21, 5, 9, 20, 3, 12, 29];
        for v in [1.0f32, 2.0, 3.0] {
            input.extend_from_slice(&v.to_be_bytes());
        }
        input.push(0xFF);
        let out = convert(&input);
        assert_eq!(&out[..12], [0, 0, 0x02, 18, 18, 2, 5, 1, 6, 20, 5, 9]);
        assert_eq!(
            native(&out),
            vec![
                (0, s::BYTE, Value::Byte(2)),
                (18, s::VILLAGER_DATA, Value::Ids(vec![2, 5, 1])),
                (6, s::POSE, Value::Id(5)),
                (9, s::OPTIONAL_UNSIGNED_INT, Value::Id(3)),
                (12, s::VECTOR3, Value::Floats(vec![1.0, 2.0, 3.0])),
            ]
        );
    }

    #[test]
    fn compound_tag_entries_are_left_out() {
        // 16 COMPOUND_TAG (an empty compound), then a boolean.
        let out = convert(&[15, 16, 10, 0, 4, 8, 1, 0xFF]);
        assert_eq!(out, [4, s::BOOLEAN as u8, 1, 0xFF]);
    }

    #[test]
    fn variant_holders_become_26_3_ids() {
        // Wolf variant reference 3 (id 2), painting reference 8 (kept as
        // 26.3's holder), cat variant 4.
        let out = convert(&[17, 23, 3, 9, 26, 8, 19, 22, 4, 0xFF]);
        assert_eq!(
            native(&out),
            vec![
                (17, s::WOLF_VARIANT, Value::Id(2)),
                (9, s::PAINTING_VARIANT, Value::Id(8)),
                (19, s::CAT_VARIANT, Value::Id(4)),
            ]
        );
        // A direct wolf variant: three textures and a biome tag, left out.
        let mut direct = vec![17, 23, 0];
        for t in ["a:w", "a:t", "a:x"] {
            direct.push(t.len() as u8);
            direct.extend_from_slice(t.as_bytes());
        }
        direct.extend_from_slice(&[0, 3]);
        direct.extend_from_slice(b"a:b");
        direct.extend_from_slice(&[1, 8, 0, 0xFF]);
        assert_eq!(convert(&direct), [1, s::BOOLEAN as u8, 0, 0xFF]);
    }

    #[test]
    fn item_stacks_go_through_the_item_rewrite() {
        // ItemEntity.DATA_ITEM: 1 of item 30 with potion_contents (31 -> 53).
        let out = convert(&[8, 7, 1, 30, 1, 0, 31, 1, 5, 0, 0, 0xFF]);
        assert_eq!(out, [8, 7, 1, 30, 1, 0, 53, 1, 5, 0, 0, 0, 0xFF]);
        native(&out);
    }

    #[test]
    fn a_mod_serializer_ends_the_list() {
        // Health (float), then NeoForge serializer 256 + 3 (a mod's list),
        // then more entries that can't be found.
        let mut input = vec![9, 3];
        input.extend_from_slice(&20.0f32.to_be_bytes());
        input.extend_from_slice(&[17, 0x83, 0x02, 7, 7, 7, 0, 0, 1, 0xFF]);
        let out = convert(&input);
        let mut expected = vec![9, 3];
        expected.extend_from_slice(&20.0f32.to_be_bytes());
        expected.push(0xFF);
        assert_eq!(out, expected);
        assert_eq!(native(&out), vec![(9, s::FLOAT, Value::Float(20.0))]);
    }

    #[test]
    fn mod_serializers_and_particles_with_server_codec_trees_are_left_out() {
        let tsv = "neoforge:entity_data_serializers\t3\tmod:state\t{\"seq\":[\"bool\",\"int\"]}\n\
                   minecraft:particle_type\t150\tmod:spark\t{\"seq\":[\"float\",\"varint\"]}\n";
        codec_tree::with_test_tables(tsv, || {
            // NeoForge serializer 256 + 3 (a flag and an int), then a byte.
            let out = convert(&[17, 0x83, 0x02, 1, 0, 0, 0, 7, 4, 0, 9, 0xFF]);
            assert_eq!(native(&out), vec![(4, s::BYTE, Value::Byte(9))]);
            // The mod's spark (150: a float and a VarInt) as a PARTICLE entry,
            // then in a PARTICLES list before angry_villager, then a byte.
            let mut spark = vec![0x96, 0x01];
            spark.extend_from_slice(&0.5f32.to_be_bytes());
            spark.push(3);
            let mut input = vec![5, 17];
            input.extend_from_slice(&spark);
            input.extend_from_slice(&[6, 18, 2]);
            input.extend_from_slice(&spark);
            input.extend_from_slice(&[0, 4, 0, 9, 0xFF]);
            let out = convert(&input);
            assert_eq!(
                native(&out),
                vec![
                    (
                        6,
                        s::PARTICLES,
                        Value::Particles(vec![Particle { id: 0, data: None }])
                    ),
                    (4, s::BYTE, Value::Byte(9)),
                ]
            );
        });
    }

    #[test]
    fn particles_are_renumbered_by_name() {
        // angry_villager 0 -> 0, cloud 4 -> 11, heart 42 -> 55, trial_omen
        // 108 -> 124.
        assert_eq!(particle_777(&[0]), [0]);
        assert_eq!(particle_777(&[4]), [11]);
        assert_eq!(particle_777(&[42]), [55]);
        assert_eq!(particle_777(&[108]), [124]);
        // block 1 with state 77.
        assert_eq!(particle_777(&[1, 77]), [1, 77]);
        // A mod's particle (aether's 109) has no codec.
        assert!(particle(&mut Reader::new(&[109]), &mut Writer::new()).is_err());
        for name in PARTICLES_767 {
            assert!(PARTICLES_777.contains(&name), "{name}");
        }
    }

    #[test]
    fn particle_options_take_26_3_codecs() {
        // dust 13: red (1, 0, 0) at scale 1.5 -> 21: 0xFFFF0000, 1.5.
        let mut dust = vec![13];
        for v in [1.0f32, 0.0, 0.0, 1.5] {
            dust.extend_from_slice(&v.to_be_bytes());
        }
        let out = particle_777(&dust);
        let p = Particle::read(&mut Reader::new(&out)).unwrap();
        assert_eq!(p.id, 21);
        assert_eq!(
            p.data,
            Some(ParticleData::ColorScale(0xFFFF0000u32 as i32, 1.5))
        );
        // dust_color_transition 14: from (0, 0, 1) to (0.5, 0.5, 0.5), scale 1.
        let mut transition = vec![14];
        for v in [0.0f32, 0.0, 1.0, 0.5, 0.5, 0.5, 1.0] {
            transition.extend_from_slice(&v.to_be_bytes());
        }
        let p = Particle::read(&mut Reader::new(&particle_777(&transition))).unwrap();
        assert_eq!(
            p.data,
            Some(ParticleData::ColorScale2(
                0xFF0000FFu32 as i32,
                0xFF7F7F7Fu32 as i32,
                1.0
            ))
        );
        // Simple in 1.21.1, options in 26.3: effect 15 -> 23, dragon_breath
        // 7 -> 15, flash 39 -> 52.
        let p = Particle::read(&mut Reader::new(&particle_777(&[15]))).unwrap();
        assert_eq!(
            (p.id, p.data),
            (23, Some(ParticleData::ColorPower(-1, 1.0)))
        );
        let p = Particle::read(&mut Reader::new(&particle_777(&[7]))).unwrap();
        assert_eq!((p.id, p.data), (15, Some(ParticleData::Scalar(1.0))));
        let p = Particle::read(&mut Reader::new(&particle_777(&[39]))).unwrap();
        assert_eq!((p.id, p.data), (52, Some(ParticleData::Color(-1))));
        // item 44: a stack (count 1, item 9) -> 57 with a template.
        let p = Particle::read(&mut Reader::new(&particle_777(&[44, 1, 9, 0, 0]))).unwrap();
        assert_eq!(p.id, 57);
        let Some(ParticleData::Item(stack)) = p.data else {
            panic!("item particle");
        };
        assert_eq!((stack.item, stack.count), (9, 1));
        // vibration 45: to entity 12 at y offset 0.5, 20 ticks -> 58.
        let mut vibration = vec![45, 1, 12];
        vibration.extend_from_slice(&0.5f32.to_be_bytes());
        vibration.push(20);
        let out = particle_777(&vibration);
        assert_eq!(out[0], 58);
        assert_eq!(&out[1..], &vibration[1..]);
        // shriek 99 -> 115, sculk_charge 35 -> 48.
        assert_eq!(particle_777(&[99, 5]), [115, 5]);
        let mut charge = vec![35];
        charge.extend_from_slice(&0.25f32.to_be_bytes());
        assert_eq!(particle_777(&charge)[0], 48);
    }

    #[test]
    fn particle_lists_and_entries_after_them_survive() {
        // PARTICLES (18 -> 17): entity_effect 20 (ARGB) and the omen 107,
        // then a byte.
        let input = [10, 18, 2, 20, 0x7F, 0, 0xFF, 0, 107, 11, 0, 4, 0xFF];
        let out = convert(&input);
        assert_eq!(
            native(&out),
            vec![
                (
                    10,
                    s::PARTICLES,
                    Value::Particles(vec![
                        Particle {
                            id: 28,
                            data: Some(ParticleData::Color(0x7F00FF00)),
                        },
                        Particle {
                            id: 123,
                            data: None,
                        },
                    ])
                ),
                (11, s::BYTE, Value::Byte(4)),
            ]
        );
    }
}
