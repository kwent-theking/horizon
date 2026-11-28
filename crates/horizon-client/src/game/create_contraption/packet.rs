//! NeoForge 1.21.1 spawn and Create 6.0.10 contraption packets.

use horizon_core::position::BlockPos;
use horizon_protocol::buf::Reader;
use horizon_world::chunk::BlockState;
use horizon_world::identifier::Identifier;
use horizon_world::nbt::{self, Tag};
use std::collections::HashSet;
use std::sync::Arc;

// Create's default spawn budget is slightly below one MiB. The additional
// limits also bound the cost of assembling a mesh for a decoded snapshot.
const MAX_NBT_BYTES: usize = 1024 * 1024;
const MAX_BLOCKS: usize = 16_384;
const MAX_LOCAL_COORDINATE: u32 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PackedBlock<'a> {
    pub local: [i32; 3],
    pub state: Arc<BlockState>,
    pub data: Option<&'a Tag>,
}

pub(super) fn decode_advanced_spawn(data: &[u8]) -> Option<(i32, Tag)> {
    // NeoForge's AdvancedAddEntityPayload is an entity VarInt followed by a
    // VarInt-length byte array. Create writes an unnamed network NBT root into
    // that array, or TAG_End when the contraption exceeds its packet budget.
    let mut input = Reader::new(data);
    let entity = input.varint().ok()?;
    let size = input.count(MAX_NBT_BYTES).ok()?;
    if input.remaining() != size {
        return None;
    }
    let limits = nbt::Limits {
        max_bytes: MAX_NBT_BYTES,
        max_nodes: 128 * 1024,
        max_depth: 64,
    };
    let (tag, used) = nbt::read_network(input.rest(), limits).ok()?;
    (used == size && matches!(tag.get("Contraption"), Some(Tag::Compound(_))))
        .then_some((entity, tag))
}

pub(super) fn decode_stall(data: &[u8]) -> Option<(i32, [f64; 3])> {
    // Create's ContraptionStallPacket uses a fixed int ID and includes the
    // rotation angle even for translating contraptions.
    let mut input = Reader::new(data);
    let id = input.i32().ok()?;
    let position = [input.f64().ok()?, input.f64().ok()?, input.f64().ok()?];
    let angle = input.f32().ok()?;
    (input.remaining() == 0 && position.into_iter().all(f64::is_finite) && angle.is_finite())
        .then_some((id, position))
}

pub(super) fn decode_pos(tag: &Tag) -> Option<[i32; 3]> {
    match tag {
        Tag::Long(packed) => {
            let BlockPos { x, y, z } = BlockPos::unpack(*packed);
            Some([x, y, z])
        }
        Tag::IntArray(pos) => pos.as_slice().try_into().ok(),
        Tag::Compound(_) => {
            let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))) =
                (tag.get("X"), tag.get("Y"), tag.get("Z"))
            else {
                return None;
            };
            Some([*x, *y, *z])
        }
        _ => None,
    }
}

/// Decodes the complete visual snapshot. One invalid entry invalidates the snapshot.
pub(super) fn decode_blocks(spawn: &Tag) -> Option<Vec<PackedBlock<'_>>> {
    let blocks = spawn.get("Contraption")?.get("Blocks")?;
    let (list, palette) = match blocks {
        Tag::Compound(_) => {
            let Tag::List(list) = blocks.get("BlockList")? else {
                return None;
            };
            let Tag::List(palette) = blocks.get("Palette")? else {
                return None;
            };
            if list.len() > MAX_BLOCKS || palette.len() > MAX_BLOCKS {
                return None;
            }
            let palette = palette
                .iter()
                .map(decode_state)
                .collect::<Option<Vec<_>>>()?;
            (list, Some(palette))
        }
        Tag::List(list) if list.len() <= MAX_BLOCKS => (list, None),
        _ => return None,
    };

    let mut occupied = HashSet::with_capacity(list.len());
    list.iter()
        .map(|entry| {
            let local = decode_pos(entry.get("Pos")?)?;
            if local
                .iter()
                .any(|axis| axis.unsigned_abs() > MAX_LOCAL_COORDINATE)
                || !occupied.insert(local)
            {
                return None;
            }
            let state = match &palette {
                Some(palette) => {
                    let Tag::Int(index) = entry.get("State")? else {
                        return None;
                    };
                    Arc::clone(palette.get(usize::try_from(*index).ok()?)?)
                }
                // Older contraptions embedded the full block state in each entry.
                None => decode_state(entry.get("Block")?)?,
            };
            Some(PackedBlock {
                local,
                state,
                data: entry.get("Data"),
            })
        })
        .collect()
}

pub(super) fn decode_state(tag: &Tag) -> Option<Arc<BlockState>> {
    let Tag::String(name) = tag.get("Name")? else {
        return None;
    };
    let name = Identifier::parse(&name.to_string().ok()?).ok()?;
    if name.path().is_empty() {
        return None;
    }
    let properties = match tag.get("Properties") {
        None => Default::default(),
        Some(Tag::Compound(properties)) => properties
            .iter()
            .map(|(key, value)| {
                let Tag::String(value) = value else {
                    return None;
                };
                Some((key.to_string().ok()?, value.to_string().ok()?))
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    Some(Arc::new(BlockState {
        name: name.to_string(),
        properties,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;

    fn spawn(blocks: Tag) -> Tag {
        Tag::compound([
            ("ControllerRelative", Tag::IntArray(vec![-3, 0, 0])),
            (
                "Contraption",
                Tag::compound([
                    ("Type", Tag::String("create:piston".into())),
                    ("Blocks", blocks),
                ]),
            ),
        ])
    }

    fn palette(entries: Vec<Tag>) -> Tag {
        Tag::compound([
            (
                "Palette",
                Tag::List(vec![Tag::compound([
                    ("Name", Tag::String("minecraft:oak_stairs".into())),
                    (
                        "Properties",
                        Tag::compound([
                            ("facing", Tag::String("west".into())),
                            ("half", Tag::String("top".into())),
                            ("shape", Tag::String("straight".into())),
                            ("waterlogged", Tag::String("false".into())),
                        ]),
                    ),
                ])]),
            ),
            ("BlockList", Tag::List(entries)),
        ])
    }

    fn entry(x: i32, y: i32, z: i32, index: i32) -> Tag {
        Tag::compound([
            ("Pos", Tag::Long(BlockPos::new(x, y, z).pack())),
            ("State", Tag::Int(index)),
        ])
    }

    #[test]
    fn decodes_neoforge_spawn_envelope_and_native_create_palette() {
        let root = spawn(palette(vec![entry(-7, 2, -4, 0), entry(0, -1, 0, 0)]));
        let nbt = nbt::write_network(&root, nbt::Limits::default()).unwrap();
        let mut writer = Writer::new();
        writer.varint(300);
        writer.byte_array(&nbt);
        let data = writer.data;
        assert_eq!(&data[..2], &[0xac, 0x02]);
        assert_ne!(data[2] & 0x80, 0);

        let (id, decoded) = decode_advanced_spawn(&data).unwrap();
        assert_eq!(id, 300);
        assert_eq!(
            decode_pos(decoded.get("ControllerRelative").unwrap()),
            Some([-3, 0, 0])
        );
        let blocks = decode_blocks(&decoded).unwrap();
        assert_eq!(blocks[0].local, [-7, 2, -4]);
        assert_eq!(blocks[1].local, [0, -1, 0]);
        assert_eq!(blocks[0].state.name, "minecraft:oak_stairs");
        assert_eq!(blocks[0].state.properties["facing"], "west");
        assert_eq!(blocks[0].state.properties["half"], "top");
        assert_eq!(blocks[0].state.properties["shape"], "straight");
        assert_eq!(blocks[0].state.properties["waterlogged"], "false");
        assert!(Arc::ptr_eq(&blocks[0].state, &blocks[1].state));
        assert!(decode_advanced_spawn(&data[..data.len() - 1]).is_none());
    }

    #[test]
    fn rejects_incomplete_oversize_and_trailing_spawn_data() {
        // VarInt ID, VarInt size, unnamed root compound, empty Contraption compound.
        let native = b"\xac\x02\x11\x0a\x0a\x00\x0bContraption\0\0";
        assert_eq!(decode_advanced_spawn(native).unwrap().0, 300);
        assert!(decode_advanced_spawn(&[1, 1, 0]).is_none());
        let mut extra = native.to_vec();
        extra.push(0);
        assert!(decode_advanced_spawn(&extra).is_none());
        extra[2] += 1;
        assert!(decode_advanced_spawn(&extra).is_none());

        let mut writer = Writer::new();
        writer.varint(1);
        writer.varint(MAX_NBT_BYTES as i32 + 1);
        assert!(decode_advanced_spawn(&writer.data).is_none());
    }

    #[test]
    fn malformed_blocks_fail_the_whole_snapshot() {
        let valid = entry(0, 0, 0, 0);
        for invalid in [
            entry(2, 0, 0, -1),
            entry(2, 0, 0, 1),
            entry(4097, 0, 0, 0),
            valid.clone(),
        ] {
            assert!(decode_blocks(&spawn(palette(vec![valid.clone(), invalid]))).is_none());
        }
        assert!(decode_blocks(&spawn(palette(vec![valid; MAX_BLOCKS + 1]))).is_none());
    }

    #[test]
    fn older_blocks_accept_int_array_and_compound_positions() {
        let old_pos = Tag::compound([("X", Tag::Int(3)), ("Y", Tag::Int(-4)), ("Z", Tag::Int(5))]);
        let old_block = Tag::compound([("Name", Tag::String("minecraft:stone".into()))]);
        let snapshot = spawn(Tag::List(vec![Tag::compound([
            ("Pos", old_pos),
            ("Block", old_block),
        ])]));
        let blocks = decode_blocks(&snapshot).unwrap();
        assert_eq!(blocks[0].local, [3, -4, 5]);
        assert_eq!(blocks[0].state.name, "minecraft:stone");
        assert_eq!(decode_pos(&Tag::IntArray(vec![1, -2, 3])), Some([1, -2, 3]));
        assert!(decode_pos(&Tag::IntArray(vec![1, 2])).is_none());
    }

    #[test]
    fn stall_decodes_fixed_width_id_and_exact_coordinates() {
        let position = [-5.25, 64.0, 0.125];
        let encode = |position: [f64; 3], angle: f32| {
            let mut writer = Writer::new();
            writer.i32(300);
            for value in position {
                writer.f64(value);
            }
            writer.f32(angle);
            writer.data
        };
        let mut wire = encode(position, -12.0);
        assert_eq!(wire.len(), 32);
        assert_eq!(&wire[..4], &[0, 0, 1, 0x2c]);
        assert_eq!(decode_stall(&wire), Some((300, position)));
        assert!(decode_stall(&wire[..31]).is_none());
        wire.push(0);
        assert!(decode_stall(&wire).is_none());
        assert!(decode_stall(&encode([f64::NAN, 1.0, 2.0], 0.0)).is_none());
        assert!(decode_stall(&encode(position, f32::INFINITY)).is_none());
    }
}
