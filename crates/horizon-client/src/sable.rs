//! Sable 2.0.6 clientbound TCP payloads and plot addressing.

use crate::sublevel::SubLevelPose;
use horizon_protocol::{Error, Result, buf::Reader};
use horizon_world::nbt::Tag;

pub const CHANNEL: &str = "sable:snapshot_packet";

/// Plot size and grid origin in Sable's backing chunk coordinates.
pub const LOG_PLOT_SIZE: i32 = 7;
pub const PLOT_ORIGIN: i32 = 10_000;
const PLOTS_PER_AXIS: i32 = 1 << LOG_PLOT_SIZE;
const ENTRY_BYTES: usize = 96;

pub type Plot = (i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartTracking {
    pub plot: Plot,
    pub id: u128,
    pub last_pose: SubLevelPose,
    pub pose: SubLevelPose,
    pub bounds: Bounds,
    pub name: Option<String>,
    pub tick: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotInfo {
    pub elapsed_ms: i32,
    pub tick: i32,
    pub stopped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DimensionPhysics {
    pub dimension: String,
    pub priority: i32,
    pub drag: Option<f32>,
    pub gravity: Option<[f32; 3]>,
    pub pressure: Option<f64>,
    pub pressure_curve: Option<Vec<[f64; 3]>>,
    pub magnetic_north: Option<[f32; 3]>,
    pub ignore_chunks: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Start(StartTracking),
    Finalize(Plot),
    StopTracking(Plot),
    Bounds(Plot, Bounds),
    Name(u128, Option<String>),
    Split {
        id: u128,
        parent: u128,
        pose: SubLevelPose,
    },
    StopMoving(Plot),
    SnapshotInfo(SnapshotInfo),
    Snapshot(SableSnapshot),
    FreezePlayer(u128, [f64; 3]),
    PhysicsProperty(Tag),
    DimensionPhysics(Vec<DimensionPhysics>),
    FloatingMaterial(String, Tag),
    UdpActivation(u128),
    EnterGizmo,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SableSnapshotEntry {
    /// ChunkPos.asLong, with X in the low 32 bits and Z in the high 32.
    pub plot_coordinate: i64,
    pub position: [f64; 3],
    pub rotation: [f32; 4],
    pub rotation_point: [f64; 3],
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct SableSnapshot {
    pub interpolation_tick: i32,
    pub entries: Vec<SableSnapshotEntry>,
}

pub fn is_clientbound(channel: &str) -> bool {
    matches!(
        channel,
        CHANNEL
            | "sable:snapshot_info_packet"
            | "sable:start_tracking_sub_level"
            | "sable:finalize_sub_level"
            | "sable:stop_tracking_sub_level"
            | "sable:change_bounds_sublevel"
            | "sable:change_sub_level_name"
            | "sable:recently_split_sub_level"
            | "sable:stop_moving_sub_level"
            | "sable:freeze_player"
            | "sable:physics_property"
            | "sable:dimension_physics"
            | "sable:floating_material"
            | "sable:udp_activation"
            | "sable:enter_gizmo_mode"
    )
}

pub fn decode(channel: &str, data: &[u8]) -> Result<Option<Packet>> {
    let mut r = Reader::new(data);
    let packet = match channel {
        CHANNEL => Packet::Snapshot(read_snapshot(&mut r)?),
        "sable:snapshot_info_packet" => Packet::SnapshotInfo(SnapshotInfo {
            elapsed_ms: r.i32()?,
            tick: r.i32()?,
            stopped: r.bool()?,
        }),
        "sable:start_tracking_sub_level" => Packet::Start(StartTracking {
            plot: read_plot(&mut r)?,
            id: r.uuid()?,
            last_pose: read_pose(&mut r)?,
            pose: read_pose(&mut r)?,
            bounds: read_bounds(&mut r)?,
            name: read_name(&mut r)?,
            tick: r.i32()?,
        }),
        "sable:finalize_sub_level" => Packet::Finalize(read_plot(&mut r)?),
        "sable:stop_tracking_sub_level" => Packet::StopTracking(read_plot(&mut r)?),
        "sable:change_bounds_sublevel" => Packet::Bounds(read_plot(&mut r)?, read_bounds(&mut r)?),
        "sable:change_sub_level_name" => Packet::Name(r.uuid()?, read_name(&mut r)?),
        "sable:recently_split_sub_level" => Packet::Split {
            id: r.uuid()?,
            parent: r.uuid()?,
            pose: read_pose(&mut r)?,
        },
        "sable:stop_moving_sub_level" => Packet::StopMoving(read_plot(&mut r)?),
        "sable:freeze_player" => Packet::FreezePlayer(r.uuid()?, read_vec3d(&mut r)?),
        "sable:physics_property" => Packet::PhysicsProperty(r.nbt()?),
        "sable:floating_material" => Packet::FloatingMaterial(r.string(32767)?, r.nbt()?),
        "sable:dimension_physics" => {
            // String length, priority, five presence flags, and ignoreChunks.
            let count = r.count(r.remaining() / 11)?;
            Packet::DimensionPhysics(
                (0..count)
                    .map(|_| read_dimension(&mut r))
                    .collect::<Result<_>>()?,
            )
        }
        "sable:udp_activation" => Packet::UdpActivation(r.uuid()?),
        "sable:enter_gizmo_mode" => Packet::EnterGizmo,
        _ => return Ok(None),
    };
    if r.remaining() != 0 {
        return Err(Error::new(format!("{channel}: trailing bytes")));
    }
    Ok(Some(packet))
}

fn read_snapshot(r: &mut Reader<'_>) -> Result<SableSnapshot> {
    let interpolation_tick = r
        .i32()
        .map_err(|e| Error::new(format!("interpolation tick: {e}")))?;
    let count = r
        .varint()
        .map_err(|e| Error::new(format!("entry count: {e}")))?;
    let count = usize::try_from(count)
        .ok()
        .filter(|&n| n <= r.remaining() / ENTRY_BYTES)
        .ok_or_else(|| Error::new("entry count exceeds snapshot payload"))?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let plot_coordinate = r.i64()?;
        let pose = read_pose(r)?;
        entries.push(SableSnapshotEntry {
            plot_coordinate,
            position: pose.position,
            rotation: pose.rotation,
            rotation_point: pose.rotation_point,
            linear_velocity: read_vec3f(r)?,
            angular_velocity: read_vec3f(r)?,
        });
    }
    Ok(SableSnapshot {
        interpolation_tick,
        entries,
    })
}

pub fn decode_snapshot(data: &[u8]) -> std::result::Result<SableSnapshot, String> {
    match decode(CHANNEL, data).map_err(|e| e.to_string())? {
        Some(Packet::Snapshot(snapshot)) => Ok(snapshot),
        _ => unreachable!("snapshot channel"),
    }
}

fn read_pose(r: &mut Reader<'_>) -> Result<SubLevelPose> {
    let position = read_vec3d(r)?;
    let rotation = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let rotation_point = read_vec3d(r)?;
    let norm: f64 = rotation.iter().map(|&v| f64::from(v).powi(2)).sum();
    if !position
        .into_iter()
        .chain(rotation_point)
        .all(f64::is_finite)
        || !norm.is_finite()
        || norm < 1e-12
    {
        return Err(Error::new("invalid Sable pose"));
    }
    Ok(SubLevelPose {
        position,
        rotation,
        rotation_point,
    })
}

fn read_plot(r: &mut Reader<'_>) -> Result<Plot> {
    Ok(plot_coord_parts(r.i64()?))
}

fn read_bounds(r: &mut Reader<'_>) -> Result<Bounds> {
    Ok(Bounds {
        min: [r.i32()?, r.i32()?, r.i32()?],
        max: [r.i32()?, r.i32()?, r.i32()?],
    })
}

fn read_vec3d(r: &mut Reader<'_>) -> Result<[f64; 3]> {
    Ok([r.f64()?, r.f64()?, r.f64()?])
}

fn read_vec3f(r: &mut Reader<'_>) -> Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn optional<'a, T>(
    r: &mut Reader<'a>,
    read: impl FnOnce(&mut Reader<'a>) -> Result<T>,
) -> Result<Option<T>> {
    r.bool()?.then(|| read(r)).transpose()
}

fn read_name(r: &mut Reader<'_>) -> Result<Option<String>> {
    optional(r, |r| r.string(32767))
}

fn read_dimension(r: &mut Reader<'_>) -> Result<DimensionPhysics> {
    Ok(DimensionPhysics {
        dimension: r.string(32767)?,
        priority: r.i32()?,
        drag: optional(r, Reader::f32)?,
        gravity: optional(r, read_vec3f)?,
        pressure: optional(r, Reader::f64)?,
        pressure_curve: optional(r, |r| {
            let count = r.count(r.remaining() / 24)?;
            (0..count).map(|_| read_vec3d(r)).collect()
        })?,
        magnetic_north: optional(r, read_vec3f)?,
        ignore_chunks: r.bool()?,
    })
}

pub fn plot_coord_parts(v: i64) -> Plot {
    (v as i32, (v >> 32) as i32)
}

pub fn plot_coord_long(x: i32, z: i32) -> i64 {
    (i64::from(x) & 0xFFFF_FFFF) | (i64::from(z) << 32)
}

pub fn plot_from_chunk(x: i32, z: i32) -> Option<Plot> {
    let px = (x >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    let pz = (z >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    ((0..PLOTS_PER_AXIS).contains(&px) && (0..PLOTS_PER_AXIS).contains(&pz)).then_some((px, pz))
}

pub fn is_plot_chunk(x: i32, z: i32) -> bool {
    plot_from_chunk(x, z).is_some()
}

pub fn plot_chunk_origin(px: i32, pz: i32) -> Option<(i32, i32)> {
    ((0..PLOTS_PER_AXIS).contains(&px) && (0..PLOTS_PER_AXIS).contains(&pz)).then(|| {
        (
            (PLOT_ORIGIN + px) << LOG_PLOT_SIZE,
            (PLOT_ORIGIN + pz) << LOG_PLOT_SIZE,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::buf::Writer;

    /// Build a payload the way the Sable server does: int tick, VarInt
    /// count, then fixed 96-byte entries.
    fn payload(tick: i32, entries: &[SableSnapshotEntry]) -> Vec<u8> {
        let mut w = Writer::new();
        w.i32(tick);
        w.varint(entries.len() as i32);
        for e in entries {
            w.i64(e.plot_coordinate);
            for c in e.position {
                w.f64(c);
            }
            for c in e.rotation {
                w.f32(c);
            }
            for c in e.rotation_point {
                w.f64(c);
            }
            for c in e.linear_velocity {
                w.f32(c);
            }
            for c in e.angular_velocity {
                w.f32(c);
            }
        }
        w.data
    }

    fn entry(plot: (i32, i32)) -> SableSnapshotEntry {
        SableSnapshotEntry {
            plot_coordinate: plot_coord_long(plot.0, plot.1),
            position: [1.5, -64.25, 3.0],
            rotation: [0.0, 0.7071068, 0.0, 0.7071068],
            rotation_point: [0.0, 0.5, 0.0],
            linear_velocity: [0.1, 0.0, -0.2],
            angular_velocity: [0.0, 0.05, 0.0],
        }
    }

    #[test]
    fn empty_snapshot_decodes() {
        let s = decode_snapshot(&payload(1234, &[])).unwrap();
        assert_eq!(s.interpolation_tick, 1234);
        assert!(s.entries.is_empty());
        // Exactly 5 bytes: 4 tick + 1 count.
        assert_eq!(payload(1234, &[]).len(), 5);
    }

    #[test]
    fn entries_round_trip_and_are_96_bytes() {
        let entries = [entry((0, 0)), entry((5, 127)), entry((-3, -7))];
        let bytes = payload(42, &entries);
        assert_eq!(bytes.len(), 4 + 1 + 3 * ENTRY_BYTES);
        let s = decode_snapshot(&bytes).unwrap();
        assert_eq!(s.interpolation_tick, 42);
        assert_eq!(s.entries, entries);
    }

    #[test]
    fn truncated_and_overflowing_payloads_are_rejected() {
        let bytes = payload(42, &[entry((1, 2))]);
        // A declared count with no entry bytes behind it.
        let e = decode_snapshot(&bytes[..5]).unwrap_err();
        assert!(e.contains("entry count"));
        // Cut mid-entry: entries are a fixed 96 bytes and the count is
        // capped at remaining/96, so a deep truncation shows up as a count
        // error before a read ever runs past the end.
        let e = decode_snapshot(&bytes[..bytes.len() - 10]).unwrap_err();
        assert!(e.contains("entry count"));
        // A one-byte cut leaves 95 bytes for a 96-byte entry: the entry
        // count check rejects the truncated entry.
        let e = decode_snapshot(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(e.contains("entry count"));
        // No tick at all.
        let e = decode_snapshot(b"").unwrap_err();
        assert!(e.contains("interpolation tick"));
        // A huge declared count is capped by the bytes left.
        let mut w = Writer::new();
        w.i32(0);
        w.varint(i32::MAX);
        let e = decode_snapshot(&w.data).unwrap_err();
        assert!(e.contains("entry count"));
    }

    #[test]
    fn plot_coordinate_matches_chunk_pos_long() {
        // ChunkPos.asLong packs x low, z high; negative z sign-extends.
        assert_eq!(plot_coord_parts(plot_coord_long(-7, 12)), (-7, 12));
        let v = plot_coord_long(3, -5);
        assert_eq!(plot_coord_parts(v), (3, -5));
    }

    #[test]
    fn plot_from_chunk_matches_sable_in_bounds() {
        // Plot (5, 7), local chunk (3, 2): global = origin + local.
        let (ox, oz) = plot_chunk_origin(5, 7).unwrap();
        assert_eq!((ox, oz), ((10000 + 5) << 7, (10000 + 7) << 7));
        assert_eq!(plot_from_chunk(ox + 3, oz + 2), Some((5, 7)));
        assert!(is_plot_chunk(ox + 127, oz + 127));
        // Corners of the whole parking lot.
        assert_eq!(plot_from_chunk(10000 << 7, 10000 << 7), Some((0, 0)));
        let last = ((10000 + 128) << 7) - 1;
        assert_eq!(plot_from_chunk(last, last), Some((127, 127)));
        // Just outside on each axis.
        assert!(!is_plot_chunk((10000 << 7) - 1, 10000 << 7));
        assert!(!is_plot_chunk(10000 << 7, (10000 << 7) - 1));
        assert!(!is_plot_chunk((10000 + 128) << 7, 10000 << 7));
        assert!(!is_plot_chunk(10000 << 7, (10000 + 128) << 7));
        // Ordinary spawn-area and negative chunks are not plot chunks.
        assert!(!is_plot_chunk(0, 0));
        assert!(!is_plot_chunk(-1, -1));
        assert!(!is_plot_chunk(i32::MIN, i32::MIN));
        assert!(!is_plot_chunk(i32::MAX, i32::MAX));
        // plot_chunk_origin rejects out-of-range plots.
        assert!(plot_chunk_origin(-1, 0).is_none());
        assert!(plot_chunk_origin(0, 128).is_none());
    }

    #[test]
    fn local_coords_round_trip_through_global() {
        // For every valid plot, chunk-local offsets map back to the plot.
        for px in [0, 1, 63, 127] {
            for pz in [0, 1, 64, 127] {
                let (ox, oz) = plot_chunk_origin(px, pz).unwrap();
                for (lx, lz) in [(0, 0), (127, 127), (64, 32)] {
                    assert_eq!(plot_from_chunk(ox + lx, oz + lz), Some((px, pz)));
                }
            }
        }
    }

    #[test]
    fn java_tcp_packet_layouts() {
        fn fixture(channel: &str, packet: Packet, write: impl FnOnce(&mut Writer)) {
            let mut w = Writer::new();
            write(&mut w);
            assert_eq!(decode(channel, &w.data).unwrap(), Some(packet), "{channel}");
            w.data.push(0);
            assert!(decode(channel, &w.data).is_err(), "{channel}");
        }
        fn write_bounds(w: &mut Writer, bounds: Bounds) {
            for n in bounds.min.into_iter().chain(bounds.max) {
                w.i32(n);
            }
        }
        fn write_pose(w: &mut Writer, pose: SubLevelPose) {
            for n in pose.position {
                w.f64(n);
            }
            for n in pose.rotation {
                w.f32(n);
            }
            for n in pose.rotation_point {
                w.f64(n);
            }
        }
        let plot = (2, 7);
        let packed = 0x0000_0007_0000_0002;
        let id = 0x0102_0304_0506_0708_f0e0_d0c0_b0a0_9080;
        let bounds = Bounds {
            min: [20_484_096, -64, 20_494_336],
            max: [20_484_099, 75, 20_494_347],
        };
        let last_pose = SubLevelPose::default();
        let pose = SubLevelPose {
            position: [10.5, -3.25, 4.0],
            rotation: [0.5, -0.5, 0.5, 0.5],
            rotation_point: [20_484_097.5, 66.25, 20_494_339.5],
        };
        fixture(
            "sable:start_tracking_sub_level",
            Packet::Start(StartTracking {
                plot,
                id,
                last_pose,
                pose,
                bounds,
                name: Some("oak ship".into()),
                tick: 51,
            }),
            |w| {
                w.i64(packed);
                w.uuid(id);
                write_pose(w, last_pose);
                write_pose(w, pose);
                write_bounds(w, bounds);
                w.bool(true);
                w.string("oak ship");
                w.i32(51);
            },
        );
        for (channel, packet) in [
            ("sable:finalize_sub_level", Packet::Finalize(plot)),
            ("sable:stop_tracking_sub_level", Packet::StopTracking(plot)),
            ("sable:stop_moving_sub_level", Packet::StopMoving(plot)),
        ] {
            fixture(channel, packet, |w| w.i64(packed));
        }
        fixture(
            "sable:change_bounds_sublevel",
            Packet::Bounds(plot, bounds),
            |w| {
                w.i64(packed);
                write_bounds(w, bounds);
            },
        );
        fixture("sable:change_sub_level_name", Packet::Name(id, None), |w| {
            w.uuid(id);
            w.bool(false);
        });
        fixture(
            "sable:recently_split_sub_level",
            Packet::Split {
                id,
                parent: 17,
                pose,
            },
            |w| {
                w.uuid(id);
                w.uuid(17);
                write_pose(w, pose);
            },
        );
        fixture(
            "sable:snapshot_info_packet",
            Packet::SnapshotInfo(SnapshotInfo {
                elapsed_ms: 67,
                tick: 52,
                stopped: true,
            }),
            |w| {
                w.i32(67);
                w.i32(52);
                w.bool(true);
            },
        );
        fixture(
            "sable:freeze_player",
            Packet::FreezePlayer(id, pose.rotation_point),
            |w| {
                w.uuid(id);
                for n in pose.rotation_point {
                    w.f64(n);
                }
            },
        );
        let properties = Tag::Compound(
            [(
                "selector".into(),
                Tag::String("minecraft:oak_planks".into()),
            )]
            .into(),
        );
        fixture(
            "sable:physics_property",
            Packet::PhysicsProperty(properties.clone()),
            |w| w.nbt(&properties),
        );
        let material =
            Tag::Compound([("lift_strength".into(), Tag::Double(4.0f64.to_bits()))].into());
        fixture(
            "sable:floating_material",
            Packet::FloatingMaterial("sable:test".into(), material.clone()),
            |w| {
                w.string("sable:test");
                w.nbt(&material);
            },
        );
        fixture(
            "sable:dimension_physics",
            Packet::DimensionPhysics(vec![DimensionPhysics {
                dimension: "minecraft:overworld".into(),
                priority: 1000,
                drag: Some(0.25),
                gravity: Some([1.0, -11.0, 2.0]),
                pressure: Some(1.5),
                pressure_curve: Some(vec![[64.0, 1.5, 0.25]]),
                magnetic_north: None,
                ignore_chunks: true,
            }]),
            |w| {
                w.varint(1);
                w.string("minecraft:overworld");
                w.i32(1000);
                w.bool(true);
                w.f32(0.25);
                w.bool(true);
                for n in [1.0, -11.0, 2.0] {
                    w.f32(n);
                }
                w.bool(true);
                w.f64(1.5);
                w.bool(true);
                w.varint(1);
                for n in [64.0, 1.5, 0.25] {
                    w.f64(n);
                }
                w.bool(false);
                w.bool(true);
            },
        );
        fixture("sable:udp_activation", Packet::UdpActivation(id), |w| {
            w.uuid(id)
        });
        fixture("sable:enter_gizmo_mode", Packet::EnterGizmo, |_| {});
        assert_eq!(decode("sable:punch_sub_level", &[]).unwrap(), None);
    }
}
