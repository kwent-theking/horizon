//! Sable sub-level pose snapshots (`sable:snapshot_packet`).
//!
//! A Sable server streams the movement of every tracked sub-level (a
//! floating island / plot of chunks) as high-frequency pose snapshots on
//! the `sable:snapshot_packet` custom-payload channel. This module decodes
//! those payloads and maps the "plot parking lot" chunk coordinates Sable
//! hides sub-level chunks in back to plot coordinates.
//!
//! Wire format (FriendlyByteBuf, big-endian; see agent2_sable_protocol_report.md):
//! `int interpolationTick` (4-byte BE), `VarInt count`, then per entry a
//! `long plotCoordinate` (`ChunkPos.asLong(plotX, plotZ)`: x in the low 32
//! bits, z in the high 32) and a `Pose3d` — position (3× double),
//! orientation quaternion (4× float, x y z w), rotationPoint (3× double) —
//! followed by `Vector3f linearVelocity` and `Vector3f angularVelocity`
//! (3× float each). Note: `Pose3d` has a `scale` field on the Java side but
//! it is NOT serialized. Every entry is therefore a fixed 96 bytes.
//!
//! Sub-level chunks arrive in the SAME dimension, embedded in a far-out
//! "parking lot" region: a sub-level at plot `(px, pz)` owns real chunk
//! coords `global = local + ((PLOT_ORIGIN + px) << LOG_PLOT_SIZE, ...)`
//! with `PLOT_ORIGIN = 10000` plots and `LOG_PLOT_SIZE = 7` (128 chunks per
//! plot, 128 plots per axis). `plot_from_chunk` / `is_plot_chunk` invert
//! that mapping exactly like Sable's `SubLevelContainer.inBounds`.
//!
//! RU: декодер снапшотов поз Sable (`sable:snapshot_packet`) — тик
//! интерполяции, список (plotCoordinate, поза, скорости), а также
//! преобразование «парковочных» координат чанков обратно в координаты
//! плотов (origin 10000, размер плота 2^7 = 128 чанков).

use horizon_protocol::buf::Reader;

/// The custom-payload channel snapshots arrive on (play phase).
pub const CHANNEL: &str = "sable:snapshot_packet";

/// Plot-grid origin in plot units (`SubLevelContainer` ORIGIN).
pub const PLOT_ORIGIN: i32 = 10000;

/// log2 of the plot size in chunks: a plot is 2^7 = 128 chunks per axis.
pub const LOG_PLOT_SIZE: i32 = 7;

/// Plots per axis in the parking lot (Sable's `inBounds` checks [0, 128)).
const PLOTS_PER_AXIS: i32 = 1 << LOG_PLOT_SIZE;

/// Fixed wire size of one snapshot entry in bytes:
/// 8 (plotCoordinate) + 24 (position) + 16 (quaternion) + 24
/// (rotationPoint) + 12 (linearVelocity) + 12 (angularVelocity).
const ENTRY_BYTES: usize = 96;

/// One tracked sub-level's pose at `SableSnapshot::interpolation_tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SableSnapshotEntry {
    /// `ChunkPos.asLong(plotX, plotZ)`; decode with [`plot_coord_parts`].
    pub plot_coordinate: i64,
    /// Pose3d position, in blocks (doubles on the wire).
    pub position: [f64; 3],
    /// Pose3d orientation quaternion, wire order x, y, z, w (floats).
    pub rotation: [f32; 4],
    /// Pose3d rotationPoint: the pivot the orientation rotates around.
    pub rotation_point: [f64; 3],
    /// Linear velocity in blocks/tick, usable for extrapolation.
    pub linear_velocity: [f32; 3],
    /// Angular velocity, usable for extrapolation.
    pub angular_velocity: [f32; 3],
}

/// A decoded `sable:snapshot_packet` payload: the tick every entry poses
/// towards, plus one entry per tracked sub-level.
#[derive(Debug, Clone, PartialEq)]
pub struct SableSnapshot {
    /// The server tick the poses interpolate towards.
    pub interpolation_tick: i32,
    pub entries: Vec<SableSnapshotEntry>,
}

/// Decode a `sable:snapshot_packet` payload.
///
/// Every error is a reason to drop this one payload, never the connection:
/// the caller reports it and carries on (same contract as `modsync::decode`).
pub fn decode_snapshot(data: &[u8]) -> Result<SableSnapshot, String> {
    let mut r = Reader::new(data);
    let interpolation_tick = r
        .i32()
        .map_err(|e| format!("interpolation tick: {e}"))?;
    // An entry is a fixed ENTRY_BYTES bytes, so capping the count at the
    // bytes left bounds the loop to the input (ByteBufCodecs.readCount).
    let n = r
        .count(r.remaining() / ENTRY_BYTES)
        .map_err(|e| format!("entry count: {e}"))?;
    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        entries.push(read_entry(&mut r).map_err(|e| format!("entry {i}: {e}"))?);
    }
    Ok(SableSnapshot {
        interpolation_tick,
        entries,
    })
}

/// One fixed-layout entry: long + Pose3d (DDD ffff DDD) + 2× Vector3f.
/// Struct-literal fields evaluate top-to-bottom, matching the wire order.
fn read_entry(r: &mut Reader) -> horizon_protocol::Result<SableSnapshotEntry> {
    Ok(SableSnapshotEntry {
        plot_coordinate: r.i64()?,
        position: [r.f64()?, r.f64()?, r.f64()?],
        rotation: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        rotation_point: [r.f64()?, r.f64()?, r.f64()?],
        linear_velocity: [r.f32()?, r.f32()?, r.f32()?],
        angular_velocity: [r.f32()?, r.f32()?, r.f32()?],
    })
}

/// Split a `plotCoordinate` long (`ChunkPos.asLong`) into `(plotX, plotZ)`:
/// x in the low 32 bits, z in the high 32 (matches `ChunkPos.getX/getZ`).
pub fn plot_coord_parts(v: i64) -> (i32, i32) {
    (v as i32, (v >> 32) as i32)
}

/// Pack plot coords into a `plotCoordinate` long (`ChunkPos.asLong`).
pub fn plot_coord_long(x: i32, z: i32) -> i64 {
    (i64::from(x) & 0xFFFF_FFFF) | (i64::from(z) << 32)
}

/// If the global chunk `(x, z)` lies in Sable's plot parking lot, return
/// its plot coordinates `(px, pz)` — mirroring `SubLevelContainer.inBounds`:
/// `((x >> LOG_PLOT_SIZE) - PLOT_ORIGIN, (z >> LOG_PLOT_SIZE) - PLOT_ORIGIN)`
/// must both fall in `[0, PLOTS_PER_AXIS)`. `>>` on `i32` is arithmetic in
/// both Java and Rust, so negative chunk coords fall out of range the same
/// way on both sides.
pub fn plot_from_chunk(x: i32, z: i32) -> Option<(i32, i32)> {
    let px = (x >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    let pz = (z >> LOG_PLOT_SIZE) - PLOT_ORIGIN;
    if (0..PLOTS_PER_AXIS).contains(&px) && (0..PLOTS_PER_AXIS).contains(&pz) {
        Some((px, pz))
    } else {
        None
    }
}

/// Whether the global chunk `(x, z)` belongs to the plot parking lot.
pub fn is_plot_chunk(x: i32, z: i32) -> bool {
    plot_from_chunk(x, z).is_some()
}

/// The global chunk coordinate a plot's local `(0, 0)` chunk sits at
/// (`LevelPlot.toGlobal` origin): `((PLOT_ORIGIN + p) << LOG_PLOT_SIZE)`.
/// Local sub-level chunk coords are `global - origin`. Returns `None` for
/// plot coords outside the parking lot.
pub fn plot_chunk_origin(px: i32, pz: i32) -> Option<(i32, i32)> {
    if !(0..PLOTS_PER_AXIS).contains(&px) || !(0..PLOTS_PER_AXIS).contains(&pz) {
        return None;
    }
    Some((
        (PLOT_ORIGIN + px) << LOG_PLOT_SIZE,
        (PLOT_ORIGIN + pz) << LOG_PLOT_SIZE,
    ))
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
        // read fails (not enough bytes for the fixed layout).
        let e = decode_snapshot(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(e.contains("entry 0"));
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
}
