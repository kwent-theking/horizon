//! Pure safe-Rust BC1/BC7 texture block compression and per-kind VRAM tracking.
//!
//! Used on low-spec GPUs when [`wgpu::Features::TEXTURE_COMPRESSION_BC`] is available.
//! Only textures that never require `STORAGE_BINDING` or runtime sub-rect patches
//! (static panorama faces, glint overlays, world border, and non-animated block
//! sprite arrays when no shader pack is active) use block compression.

use std::sync::atomic::{AtomicU64, Ordering};

/// Block-compressed formats Horizon encodes in pure safe Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcFormat {
    /// 4x4 RGB565 + optional 1-bit punch-through alpha (8 bytes/block, 8:1 over RGBA8).
    Bc1,
    /// 4x4 Mode 6 RGBA 7.7.7.7 + P-bit + 4-bit weights (16 bytes/block, 4:1 over RGBA8).
    Bc7,
}

impl BcFormat {
    pub fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
            Self::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
        }
    }

    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 => 8,
            Self::Bc7 => 16,
        }
    }
}

/// Categories reported in Horizon's VRAM usage summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VramKind {
    SpriteArrays,
    HudAtlas,
    TerrainArena,
    EntityBuffers,
    RenderTargets,
    MiscTextures,
}

impl VramKind {
    pub const ALL: [Self; 6] = [
        Self::SpriteArrays,
        Self::HudAtlas,
        Self::TerrainArena,
        Self::EntityBuffers,
        Self::RenderTargets,
        Self::MiscTextures,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SpriteArrays => "sprites",
            Self::HudAtlas => "atlas",
            Self::TerrainArena => "arena",
            Self::EntityBuffers => "entities",
            Self::RenderTargets => "targets",
            Self::MiscTextures => "misc",
        }
    }
}

/// Snapshot of tracked GPU memory bytes by resource category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VramSnapshot {
    pub sprite_arrays: u64,
    pub hud_atlas: u64,
    pub terrain_arena: u64,
    pub entity_buffers: u64,
    pub render_targets: u64,
    pub misc_textures: u64,
}

impl VramSnapshot {
    pub fn total(self) -> u64 {
        self.sprite_arrays
            + self.hud_atlas
            + self.terrain_arena
            + self.entity_buffers
            + self.render_targets
            + self.misc_textures
    }

    pub fn get(self, kind: VramKind) -> u64 {
        match kind {
            VramKind::SpriteArrays => self.sprite_arrays,
            VramKind::HudAtlas => self.hud_atlas,
            VramKind::TerrainArena => self.terrain_arena,
            VramKind::EntityBuffers => self.entity_buffers,
            VramKind::RenderTargets => self.render_targets,
            VramKind::MiscTextures => self.misc_textures,
        }
    }
}

/// Lock-free tracker of resident GPU allocations per resource kind.
#[derive(Default)]
pub struct VramLedger {
    bytes: [AtomicU64; 6],
}

impl VramLedger {
    pub fn set(&self, kind: VramKind, bytes: u64) {
        self.bytes[kind as usize].store(bytes, Ordering::Relaxed);
    }

    pub fn add(&self, kind: VramKind, bytes: u64) {
        self.bytes[kind as usize].fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> VramSnapshot {
        VramSnapshot {
            sprite_arrays: self.bytes[VramKind::SpriteArrays as usize].load(Ordering::Relaxed),
            hud_atlas: self.bytes[VramKind::HudAtlas as usize].load(Ordering::Relaxed),
            terrain_arena: self.bytes[VramKind::TerrainArena as usize].load(Ordering::Relaxed),
            entity_buffers: self.bytes[VramKind::EntityBuffers as usize].load(Ordering::Relaxed),
            render_targets: self.bytes[VramKind::RenderTargets as usize].load(Ordering::Relaxed),
            misc_textures: self.bytes[VramKind::MiscTextures as usize].load(Ordering::Relaxed),
        }
    }

    pub fn format_summary(&self, vram_mb: Option<u32>) -> String {
        const MIB: f64 = (1024 * 1024) as f64;
        let snap = self.snapshot();
        let mut parts = Vec::with_capacity(VramKind::ALL.len());
        for kind in VramKind::ALL {
            parts.push(format!(
                "{}={:.1} MiB",
                kind.label(),
                snap.get(kind) as f64 / MIB
            ));
        }
        let budget = match vram_mb {
            Some(mb) => format!(" / budget ~{:.0} MiB", f64::from(mb) * 0.75),
            None => String::new(),
        };
        format!(
            "Horizon VRAM: {} (total={:.1} MiB{})",
            parts.join(", "),
            snap.total() as f64 / MIB,
            budget
        )
    }
}

/// Byte size of an uncompressed 2D/2D-array texture's full mip chain.
pub fn rgba_texture_bytes(width: u32, height: u32, layers: u32, mip_levels: u32) -> u64 {
    (0..mip_levels)
        .map(|mip| {
            let w = u64::from((width >> mip).max(1));
            let h = u64::from((height >> mip).max(1));
            w * h * 4 * u64::from(layers.max(1))
        })
        .sum()
}

/// Byte size of a block-compressed 2D/2D-array texture's mip chain (`LayerMajor`).
pub fn bc_texture_bytes(
    width: u32,
    height: u32,
    layers: u32,
    mip_levels: u32,
    format: BcFormat,
) -> u64 {
    let per_layer: u64 = (0..mip_levels)
        .map(|mip| {
            let w = u64::from((width >> mip).max(1)).div_ceil(4);
            let h = u64::from((height >> mip).max(1)).div_ceil(4);
            w * h * format.block_bytes() as u64
        })
        .sum();
    per_layer * u64::from(layers.max(1))
}

/// Chooses `Bc7` when any texel carries partial alpha (`1..=254`), otherwise compact `Bc1`.
pub fn pick_format(rgba: &[u8]) -> BcFormat {
    if rgba.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255) {
        BcFormat::Bc7
    } else {
        BcFormat::Bc1
    }
}

/// Compresses an RGBA8 texture buffer (`width`x`height`) into `BcFormat`.
pub fn compress_rgba(rgba: &[u8], width: u32, height: u32, format: BcFormat) -> Vec<u8> {
    let bx = width.div_ceil(4);
    let by = height.div_ceil(4);
    let mut out = vec![0u8; (bx * by) as usize * format.block_bytes()];
    let row_blocks = bx as usize * format.block_bytes();
    let row_threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(by as usize)
        .max(1);
    let encode_row = |by_idx: usize, dst_row: &mut [u8]| {
        for bx_idx in 0..bx as usize {
            let mut block = [[0u8; 4]; 16];
            for py in 0..4 {
                let y = (by_idx * 4 + py).min(height.saturating_sub(1) as usize);
                for px in 0..4 {
                    let x = (bx_idx * 4 + px).min(width.saturating_sub(1) as usize);
                    let idx = (y * width as usize + x) * 4;
                    if idx + 4 <= rgba.len() {
                        block[py * 4 + px].copy_from_slice(&rgba[idx..idx + 4]);
                    }
                }
            }
            let off = bx_idx * format.block_bytes();
            match format {
                BcFormat::Bc1 => encode_bc1_block(&block, &mut dst_row[off..off + 8]),
                BcFormat::Bc7 => encode_bc7_mode6_block(&block, &mut dst_row[off..off + 16]),
            }
        }
    };
    if row_threads > 1 && out.len() >= 16384 {
        let rows_per_chunk = (by as usize).div_ceil(row_threads);
        std::thread::scope(|s| {
            for (chunk_idx, dst) in out.chunks_mut(rows_per_chunk * row_blocks).enumerate() {
                let start_row = chunk_idx * rows_per_chunk;
                s.spawn(move || {
                    for (local_row, dst_row) in dst.chunks_mut(row_blocks).enumerate() {
                        encode_row(start_row + local_row, dst_row);
                    }
                });
            }
        });
    } else {
        for (by_idx, dst_row) in out.chunks_mut(row_blocks).enumerate() {
            encode_row(by_idx, dst_row);
        }
    }
    out
}

/// Compresses a `LayerMajor` RGBA8 mip chain (`size`x`size`, `levels`, `layers`).
pub fn compress_layer_major_mips(
    rgba_mips: &[u8],
    size: u32,
    levels: u32,
    layers: u32,
    format: BcFormat,
) -> Vec<u8> {
    let mut level_rgba_offsets = Vec::with_capacity(levels as usize);
    let mut off = 0usize;
    for mip in 0..levels {
        let w = (size >> mip).max(1);
        let len = (w * w * 4) as usize;
        level_rgba_offsets.push((off, w, len));
        off += len;
    }
    let layer_rgba_bytes = off;
    let bc_layer_bytes = bc_texture_bytes(size, size, 1, levels, format) as usize;
    let mut out = vec![0u8; bc_layer_bytes * layers.max(1) as usize];

    let encode_layer = |layer_idx: usize, dst_layer: &mut [u8]| {
        let src_start = layer_idx * layer_rgba_bytes;
        if src_start + layer_rgba_bytes > rgba_mips.len() {
            return;
        }
        let src = &rgba_mips[src_start..src_start + layer_rgba_bytes];
        let mut dst_off = 0usize;
        for &(mip_off, w, len) in &level_rgba_offsets {
            let encoded = compress_rgba(&src[mip_off..mip_off + len], w, w, format);
            dst_layer[dst_off..dst_off + encoded.len()].copy_from_slice(&encoded);
            dst_off += encoded.len();
        }
    };

    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(layers.max(1) as usize)
        .max(1);
    if threads > 1 && layers >= 8 && bc_layer_bytes > 0 {
        let per_thread = (layers as usize).div_ceil(threads);
        std::thread::scope(|s| {
            for (chunk_idx, dst) in out.chunks_mut(per_thread * bc_layer_bytes).enumerate() {
                let first_layer = chunk_idx * per_thread;
                s.spawn(move || {
                    for (local, dst_layer) in dst.chunks_mut(bc_layer_bytes).enumerate() {
                        encode_layer(first_layer + local, dst_layer);
                    }
                });
            }
        });
    } else if bc_layer_bytes > 0 {
        for (layer_idx, dst_layer) in out.chunks_mut(bc_layer_bytes).enumerate() {
            encode_layer(layer_idx, dst_layer);
        }
    }
    out
}

fn pack_rgb565(rgb: [u8; 3]) -> u16 {
    let r = u16::from(rgb[0] >> 3);
    let g = u16::from(rgb[1] >> 2);
    let b = u16::from(rgb[2] >> 3);
    (r << 11) | (g << 5) | b
}

fn unpack_rgb565(c: u16) -> [i32; 3] {
    let r5 = i32::from((c >> 11) & 31);
    let g6 = i32::from((c >> 5) & 63);
    let b5 = i32::from(c & 31);
    [
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    ]
}

fn luma(rgb: [u8; 4]) -> i32 {
    i32::from(rgb[0]) * 54 + i32::from(rgb[1]) * 183 + i32::from(rgb[2]) * 19
}

fn encode_bc1_block(block: &[[u8; 4]; 16], dst: &mut [u8]) {
    let has_alpha = block.iter().any(|p| p[3] < 128);
    let mut min_px = [255u8; 3];
    let mut max_px = [0u8; 3];
    let mut any_opaque = false;
    for p in block {
        if has_alpha && p[3] < 128 {
            continue;
        }
        any_opaque = true;
        for c in 0..3 {
            min_px[c] = min_px[c].min(p[c]);
            max_px[c] = max_px[c].max(p[c]);
        }
    }
    if !any_opaque {
        // Transparent black in 3-color + alpha mode (`c0 <= c1`, index 3 = transparent).
        dst[0..4].fill(0);
        dst[4..8].fill(0xFF);
        return;
    }

    let mut c0 = pack_rgb565(max_px);
    let mut c1 = pack_rgb565(min_px);
    if has_alpha {
        if c0 > c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
    } else if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }

    let p0 = unpack_rgb565(c0);
    let p1 = unpack_rgb565(c1);
    let palette: [[i32; 3]; 4] = if has_alpha {
        [
            p0,
            p1,
            [
                (p0[0] + p1[0]) / 2,
                (p0[1] + p1[1]) / 2,
                (p0[2] + p1[2]) / 2,
            ],
            [0, 0, 0],
        ]
    } else {
        [
            p0,
            p1,
            [
                (2 * p0[0] + p1[0] + 1) / 3,
                (2 * p0[1] + p1[1] + 1) / 3,
                (2 * p0[2] + p1[2] + 1) / 3,
            ],
            [
                (p0[0] + 2 * p1[0] + 1) / 3,
                (p0[1] + 2 * p1[1] + 1) / 3,
                (p0[2] + 2 * p1[2] + 1) / 3,
            ],
        ]
    };

    let mut indices = 0u32;
    let count = if has_alpha { 3 } else { 4 };
    for (i, p) in block.iter().enumerate() {
        let code = if has_alpha && p[3] < 128 {
            3u32
        } else {
            let mut best_code = 0u32;
            let mut best_err = i32::MAX;
            for (k, pal) in palette[..count].iter().enumerate() {
                let dr = i32::from(p[0]) - pal[0];
                let dg = i32::from(p[1]) - pal[1];
                let db = i32::from(p[2]) - pal[2];
                let err = dr * dr + dg * dg + db * db;
                if err < best_err {
                    best_err = err;
                    best_code = k as u32;
                }
            }
            best_code
        };
        indices |= code << (i * 2);
    }

    dst[0..2].copy_from_slice(&c0.to_le_bytes());
    dst[2..4].copy_from_slice(&c1.to_le_bytes());
    dst[4..8].copy_from_slice(&indices.to_le_bytes());
}

const BC7_WEIGHTS4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Writes `bits` LSB-first starting at `bit_pos` into a 128-bit BC7 block.
struct BitWriter {
    bits: u128,
    pos: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { bits: 0, pos: 0 }
    }

    fn write(&mut self, value: u32, count: u32) {
        let mask = (1u128 << count) - 1;
        self.bits |= (u128::from(value) & mask) << self.pos;
        self.pos += count;
    }

    fn finish(self) -> [u8; 16] {
        debug_assert_eq!(self.pos, 128);
        self.bits.to_le_bytes()
    }
}

/// Encodes a 4x4 RGBA block using BC7 Mode 6 (1 subset, 7777 + P-bit endpoints, 4-bit indices).
fn encode_bc7_mode6_block(block: &[[u8; 4]; 16], dst: &mut [u8]) {
    let mut e0 = block[0];
    let mut e1 = block[0];
    let mut min_proj = i32::MAX;
    let mut max_proj = i32::MIN;
    for &p in block {
        let proj = luma(p) + i32::from(p[3]) * 128;
        if proj < min_proj {
            min_proj = proj;
            e0 = p;
        }
        if proj > max_proj {
            max_proj = proj;
            e1 = p;
        }
    }
    // Refine endpoints to cover component min/max direction reliably.
    let dir: [i32; 4] = std::array::from_fn(|c| i32::from(e1[c]) - i32::from(e0[c]));
    let dot_dir = |p: [u8; 4]| -> i32 { (0..4).map(|c| i32::from(p[c]) * dir[c]).sum() };
    let mut lo = dot_dir(block[0]);
    let mut hi = lo;
    e0 = block[0];
    e1 = block[0];
    for &p in block {
        let d = dot_dir(p);
        if d < lo {
            lo = d;
            e0 = p;
        }
        if d > hi {
            hi = d;
            e1 = p;
        }
    }

    let quant = |ep: [u8; 4]| -> ([u8; 4], u8) {
        let pbit =
            ((u16::from(ep[0]) + u16::from(ep[1]) + u16::from(ep[2]) + u16::from(ep[3])) & 1) as u8;
        let q = std::array::from_fn(|c| (ep[c] >> 1).min(127));
        (q, pbit)
    };
    let (mut q0, mut p0) = quant(e0);
    let (mut q1, mut p1) = quant(e1);

    let unquant =
        |q: [u8; 4], p: u8| -> [i32; 4] { std::array::from_fn(|c| i32::from((q[c] << 1) | p)) };
    let u0 = unquant(q0, p0);
    let u1 = unquant(q1, p1);

    let mut idxs = [0u8; 16];
    for (i, &px) in block.iter().enumerate() {
        let mut best_k = 0u8;
        let mut best_err = i32::MAX;
        for (k, &w) in BC7_WEIGHTS4.iter().enumerate() {
            let err: i32 = (0..4)
                .map(|c| {
                    let interp = ((64 - w) * u0[c] + w * u1[c] + 32) >> 6;
                    let d = i32::from(px[c]) - interp;
                    d * d
                })
                .sum();
            if err < best_err {
                best_err = err;
                best_k = k as u8;
            }
        }
        idxs[i] = best_k;
    }

    // Anchor index 0 omits its MSB (must be 0..=7): swap endpoints and flip indices if >= 8.
    if idxs[0] >= 8 {
        std::mem::swap(&mut q0, &mut q1);
        std::mem::swap(&mut p0, &mut p1);
        for idx in &mut idxs {
            *idx = 15 - *idx;
        }
    }

    let mut bw = BitWriter::new();
    bw.write(1 << 6, 7); // Mode 6
    for c in 0..4 {
        bw.write(u32::from(q0[c]), 7);
        bw.write(u32::from(q1[c]), 7);
    }
    bw.write(u32::from(p0), 1);
    bw.write(u32::from(p1), 1);
    bw.write(u32::from(idxs[0]), 3);
    for &idx in &idxs[1..] {
        bw.write(u32::from(idx), 4);
    }
    dst.copy_from_slice(&bw.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_bc7_mode6(block: &[u8; 16]) -> [[u8; 4]; 16] {
        let bits = u128::from_le_bytes(*block);
        let mut pos = 0u32;
        let mut read = |n: u32| -> u32 {
            let v = ((bits >> pos) & ((1u128 << n) - 1)) as u32;
            pos += n;
            v
        };
        assert_eq!(read(7), 1 << 6);
        let mut q0 = [0i32; 4];
        let mut q1 = [0i32; 4];
        for c in 0..4 {
            q0[c] = read(7) as i32;
            q1[c] = read(7) as i32;
        }
        let p0 = read(1) as i32;
        let p1 = read(1) as i32;
        let e0: [i32; 4] = std::array::from_fn(|c| (q0[c] << 1) | p0);
        let e1: [i32; 4] = std::array::from_fn(|c| (q1[c] << 1) | p1);
        std::array::from_fn(|i| {
            let idx = read(if i == 0 { 3 } else { 4 }) as usize;
            let w = BC7_WEIGHTS4[idx];
            std::array::from_fn(|c| (((64 - w) * e0[c] + w * e1[c] + 32) >> 6).clamp(0, 255) as u8)
        })
    }

    #[test]
    fn bc1_and_bc7_preserve_colors_transparency_and_sizes() {
        let mut opaque = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4u8 {
            for x in 0..4u8 {
                let t = (x + y * 4) * 16;
                opaque.extend_from_slice(&[t, 255 - t, t / 2, 255]);
            }
        }
        assert_eq!(pick_format(&opaque), BcFormat::Bc1);
        let bc1 = compress_rgba(&opaque, 4, 4, BcFormat::Bc1);
        assert_eq!(bc1.len(), 8);

        let mut cutout = opaque.clone();
        cutout[3] = 0;
        assert_eq!(pick_format(&cutout), BcFormat::Bc1);
        let bc1_cutout = compress_rgba(&cutout, 4, 4, BcFormat::Bc1);
        assert_eq!(bc1_cutout.len(), 8);
        let c0 = u16::from_le_bytes(bc1_cutout[0..2].try_into().unwrap());
        let c1 = u16::from_le_bytes(bc1_cutout[2..4].try_into().unwrap());
        assert!(c0 <= c1, "punch-through alpha mode must set c0 <= c1");
        let idx = u32::from_le_bytes(bc1_cutout[4..8].try_into().unwrap());
        assert_eq!(idx & 3, 3, "transparent texel must map to index 3");

        let mut translucent = opaque;
        translucent[3] = 128;
        assert_eq!(pick_format(&translucent), BcFormat::Bc7);
        let bc7 = compress_rgba(&translucent, 4, 4, BcFormat::Bc7);
        assert_eq!(bc7.len(), 16);
        let decoded = decode_bc7_mode6(bc7.as_slice().try_into().unwrap());
        assert!((i32::from(decoded[0][3]) - 128).abs() <= 4);
        assert!((i32::from(decoded[15][3]) - 255).abs() <= 2);
    }

    #[test]
    fn layer_major_mip_compression_matches_wgpu_layout_bytes() {
        let size = 16u32;
        let levels = 5u32;
        let layers = 3u32;
        let chain_bytes: usize = (0..levels)
            .map(|l| ((size >> l) * (size >> l) * 4) as usize)
            .sum();
        let src = vec![200u8; chain_bytes * layers as usize];
        let compressed = compress_layer_major_mips(&src, size, levels, layers, BcFormat::Bc1);
        assert_eq!(
            compressed.len() as u64,
            bc_texture_bytes(size, size, layers, levels, BcFormat::Bc1)
        );
    }

    #[test]
    fn vram_ledger_summarizes_by_kind() {
        let ledger = VramLedger::default();
        ledger.set(VramKind::SpriteArrays, 64 << 20);
        ledger.set(VramKind::HudAtlas, 80 << 20);
        ledger.add(VramKind::TerrainArena, 128 << 20);
        let snap = ledger.snapshot();
        assert_eq!(snap.total(), (64 + 80 + 128) << 20);
        let summary = ledger.format_summary(Some(4096));
        assert!(summary.contains("sprites=64.0 MiB"));
        assert!(summary.contains("atlas=80.0 MiB"));
        assert!(summary.contains("arena=128.0 MiB"));
        assert!(summary.contains("budget ~3072 MiB"));
    }
}
