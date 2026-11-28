//! Shared nearest-sampled sprite array for block and item textures.
use crate::Result;
use horizon_assets::animation::{AnimationClip, AnimationMetadata};
use horizon_world::content_view;
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Clone)]
pub struct SpriteAnimation {
    pub layer: u32,
    frames: Arc<Vec<Vec<u8>>>,
    clip: Arc<AnimationClip>,
}
/// Pixel-free playback state, after frame mip chains have been uploaded.
pub(crate) struct SpriteTiming {
    pub layer: u32,
    clip: Arc<AnimationClip>,
}
/// All original frames retained losslessly for renderer recreation.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PackedSpriteAnimation {
    layer: u32,
    clip: Arc<AnimationClip>,
    frames: Vec<crate::compact_bytes::CompactBytes>,
}
impl PackedSpriteAnimation {
    pub fn layer(&self) -> u32 {
        self.layer
    }

    pub(crate) fn valid(&self, layers: u32, size: u32) -> bool {
        let expected = (size as usize)
            .checked_mul(size as usize)
            .and_then(|n| n.checked_mul(4));
        self.layer < layers
            && self.clip.valid(self.frames.len())
            // CompactBytes validates compressed backing during deserialization.
            // Length checks do not expand the animation a second time.
            && expected.is_some_and(|expected| expected > 0 && self.frames.iter().all(|bytes| bytes.len() == expected))
    }

    pub(crate) fn valid_any_size(&self) -> bool {
        let Some(first) = self.frames.first() else {
            return false;
        };
        let frame_len = first.len();
        frame_len > 0
            && frame_len % 4 == 0
            && self.clip.valid(self.frames.len())
            && self.frames.iter().all(|bytes| bytes.len() == frame_len)
    }

    pub fn new(animation: SpriteAnimation) -> Self {
        Self {
            layer: animation.layer,
            clip: animation.clip,
            frames: animation
                .frames
                .iter()
                .map(|frame| crate::compact_bytes::CompactBytes::new(frame.clone()))
                .collect(),
        }
    }
    pub fn decode(&self) -> SpriteAnimation {
        SpriteAnimation {
            layer: self.layer,
            clip: self.clip.clone(),
            frames: Arc::new(
                self.frames
                    .iter()
                    .map(|frame| frame.decode().into_owned())
                    .collect(),
            ),
        }
    }
}
/// What an animated sprite shows: `frame`, mixed `millis` thousandths of the
/// way towards `next` (0 for a single frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameBlend {
    pub frame: usize,
    pub next: usize,
    pub millis: u32,
}
impl SpriteAnimation {
    pub fn frame(&self, tick: u64) -> usize {
        self.clip.sample(tick).frame as usize
    }
    /// `SpriteContents.AnimationState.drawToAtlas`: an animation with
    /// `interpolate` mixes the frame with the next one by
    /// `(int)(subFrame / time * 1000)` thousandths; others show the frame.
    pub fn blend(&self, tick: u64) -> FrameBlend {
        blend(&self.clip, tick)
    }
    pub(crate) fn timing(&self) -> SpriteTiming {
        SpriteTiming {
            layer: self.layer,
            clip: self.clip.clone(),
        }
    }
    pub fn resized_pixels(&self, frame: usize, width: u32, height: u32) -> Vec<u8> {
        let source = self.pixels(frame);
        let side = (source.len() / 4).isqrt() as u32;
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * side / height * side + x * side / width) * 4) as usize;
                out.extend_from_slice(&source[offset..offset + 4]);
            }
        }
        out
    }
    pub fn pixels(&self, frame: usize) -> &[u8] {
        &self.frames[frame]
    }
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}
impl SpriteTiming {
    pub fn blend(&self, tick: u64) -> FrameBlend {
        blend(&self.clip, tick)
    }
}
fn blend(clip: &AnimationClip, tick: u64) -> FrameBlend {
    let sample = clip.sample(tick);
    let frame = sample.frame as usize;
    let next = sample.next_frame as usize;
    if clip.interpolates() && sample.interpolation_millis > 0 && next != frame {
        FrameBlend {
            frame,
            next,
            millis: sample.interpolation_millis,
        }
    } else {
        FrameBlend {
            frame,
            next: frame,
            millis: 0,
        }
    }
}

#[derive(Default)]
pub struct Sprites {
    /// Edge length of every array layer and the layers' current frames, set
    /// by [`Sprites::finish`].
    pub size: u32,
    /// Smallest native layer before array-size resampling.
    pub minimum_size: u32,
    pub pixels: Vec<u8>,
    pub layers: BTreeMap<String, u32>,
    pub animations: Vec<SpriteAnimation>,
    pub opaque: Vec<bool>,
    pub translucent: Vec<bool>,
    /// Each layer's `MipmapGenerator` settings.
    pub mips: Vec<crate::mipmap::Settings>,
    /// Each layer's frames at the texture's own resolution (square), and the
    /// frame shown first.
    native: Vec<(u32, Arc<Vec<Vec<u8>>>, usize)>,
    /// Still textures by their pixels and mip settings: mods copy vanilla's
    /// textures under their own names, and one layer serves them all (the
    /// GPU's array may hold as few as 2048).
    still: std::collections::HashMap<(Arc<Vec<Vec<u8>>>, u32, [u32; 2], [bool; 4]), u32>,
    /// Decoded textures kept on disk between launches ([`Sprites::with_disk_cache`]).
    disk: Option<crate::sprite_cache::SpriteCache>,
}
/// Edge length of vanilla's block textures: the smallest array layer.
pub const SPRITE_SIZE: u32 = 16;
/// Largest sprite array (all layers of the top mip level): resource packs
/// with bigger textures are sampled down to fit.
const MAX_ARRAY_BYTES: u64 = 256 << 20;
/// Nearest-resample a square RGBA frame to another square size.
pub(crate) fn resample(frame: &[u8], from: u32, to: u32) -> Vec<u8> {
    if from == to {
        return frame.to_vec();
    }
    let mut out = Vec::with_capacity((to * to * 4) as usize);
    for row in 0..to {
        let source_row = row * from / to;
        for column in 0..to {
            let offset = ((source_row * from + column * from / to) * 4) as usize;
            out.extend_from_slice(&frame[offset..offset + 4]);
        }
    }
    out
}
impl Sprites {
    /// Sprites that reuse textures decoded by an earlier launch, kept in a
    /// cache file in `root`.
    pub fn with_disk_cache(root: &Path) -> Self {
        Self {
            disk: crate::sprite_cache::SpriteCache::open(root),
            ..Self::default()
        }
    }
    /// Write what this run decoded to the cache file, then release its memory backing.
    pub fn save_disk_cache(&mut self) {
        if let Some(mut disk) = self.disk.take() {
            disk.save();
        }
    }
    /// How many array layers the textures take.
    pub fn count(&self) -> usize {
        self.native.len()
    }
    /// `SpriteContents.computeTransparency(u0, v0, u1, v1)` is NONE: every texel under
    /// the quad's UV box is fully opaque in every frame, so the quad needs no alpha test.
    pub fn region_opaque(&self, layer: u32, uv: &[[f32; 2]; 4]) -> bool {
        if self.opaque[layer as usize] {
            return true;
        }
        let (side, frames, _) = &self.native[layer as usize];
        let side = *side as usize;
        let range = |axis: usize| {
            let lo = uv.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min);
            let hi = uv.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max);
            let clamp = |v: f32| (v.max(0.0) as usize).min(side);
            clamp((lo * side as f32).floor())..clamp((hi * side as f32).ceil())
        };
        let (columns, rows) = (range(0), range(1));
        frames.iter().all(|frame| {
            rows.clone().all(|y| {
                columns
                    .clone()
                    .all(|x| frame[(y * side + x) * 4 + 3] == 255)
            })
        })
    }
    /// Lay every layer out at one size: the largest texture's (resource packs
    /// may ship 32x, 64x... textures, like vanilla's stitched atlas), rounded
    /// up to a power of two and limited to [`MAX_ARRAY_BYTES`]. Smaller
    /// textures are scaled up by whole pixels, so UVs stay per layer.
    pub fn finish(&mut self) {
        self.finish_with_cap(0);
    }

    /// Like [`Sprites::finish`], optionally clamping any sprite larger than
    /// `max_sprite_side` pixels (rounded to a power of two) before picking the
    /// uniform layer edge.
    pub fn finish_with_cap(&mut self, max_sprite_side: u32) {
        let _ = self.finish_layers(max_sprite_side, false);
    }

    /// Pack every layer into `self.pixels` and progressively drop staging buffers
    /// (`disk`, `still`, `opaque`, `native`) that are no longer needed once packed.
    pub fn finish_and_release(&mut self, max_sprite_side: u32) -> Result<u32> {
        self.finish_layers(max_sprite_side, true)
    }

    fn finish_layers(&mut self, max_sprite_side: u32, release: bool) -> Result<u32> {
        if release {
            self.disk = None;
            self.still.clear();
            self.still.shrink_to_fit();
            self.opaque = Vec::new();
        }
        let layer_count = self.native.len();
        let layers = layer_count.max(1) as u64;
        let cap = if max_sprite_side > 0 {
            Some(max_sprite_side.max(SPRITE_SIZE).next_power_of_two())
        } else {
            None
        };
        let largest = self
            .native
            .iter()
            .map(|n| cap.map_or(n.0, |c| n.0.min(c)))
            .max()
            .unwrap_or(SPRITE_SIZE);
        self.minimum_size = self
            .native
            .iter()
            .map(|n| cap.map_or(n.0, |c| n.0.min(c)))
            .min()
            .unwrap_or(SPRITE_SIZE);
        let mut size = largest.max(SPRITE_SIZE).next_power_of_two();
        if let Some(c) = cap {
            size = size.min(c);
        }
        while size > SPRITE_SIZE && layers * u64::from(size * size) * 4 > MAX_ARRAY_BYTES {
            size /= 2;
        }
        self.pixels.clear();
        self.pixels
            .reserve(layer_count * (size * size * 4) as usize);
        let animations: BTreeMap<u32, usize> = self
            .animations
            .iter()
            .enumerate()
            .map(|(i, animation)| (animation.layer, i))
            .collect();
        if release {
            for (layer, (from, frames, initial)) in
                std::mem::take(&mut self.native).into_iter().enumerate()
            {
                Self::pack_layer(
                    layer,
                    from,
                    frames,
                    initial,
                    size,
                    &animations,
                    &mut self.pixels,
                    &mut self.animations,
                );
            }
        } else {
            for (layer, (from, frames, initial)) in self.native.iter().enumerate() {
                Self::pack_layer(
                    layer,
                    *from,
                    Arc::clone(frames),
                    *initial,
                    size,
                    &animations,
                    &mut self.pixels,
                    &mut self.animations,
                );
            }
        }
        self.size = size;
        Ok(u32::try_from(layer_count)?)
    }

    #[allow(clippy::too_many_arguments)]
    fn pack_layer(
        layer: usize,
        from: u32,
        frames: Arc<Vec<Vec<u8>>>,
        initial: usize,
        size: u32,
        animations: &BTreeMap<u32, usize>,
        pixels: &mut Vec<u8>,
        out_animations: &mut [SpriteAnimation],
    ) {
        if let Some(&i) = animations.get(&(layer as u32)) {
            let packed = if from == size {
                frames
            } else {
                Arc::new(frames.iter().map(|f| resample(f, from, size)).collect())
            };
            pixels.extend_from_slice(&packed[initial]);
            out_animations[i].frames = packed;
        } else if from == size {
            pixels.extend_from_slice(&frames[initial]);
        } else {
            // Only the initial frame of a nonanimated strip is ever drawn.
            pixels.extend(resample(&frames[initial], from, size));
        }
    }
    /// The layer of a texture, added on first use. A texture that is missing
    /// or does not load (a resource pack's filter or a broken file) becomes
    /// vanilla's missing texture, as in a stitched atlas.
    pub fn insert(&mut self, root: &Path, name: &str) -> Result<u32> {
        if let Some(layer) = self.layers.get(name) {
            return Ok(*layer);
        }
        match self.load(root, name) {
            Ok(layer) => Ok(layer),
            Err(e) => self.insert_missing(name, &e),
        }
    }
    /// Decode one texture and its `.mcmeta` without touching `self`, so callers
    /// can decode many sprites in parallel before calling [`Sprites::insert_decoded`]
    /// in deterministic order.
    pub(crate) fn decode_sprite(
        root: &Path,
        name: &str,
    ) -> std::result::Result<crate::sprite_cache::Decoded, String> {
        Self::decode(root, name).map_err(|e| e.to_string())
    }
    /// Register a sprite pre-decoded by [`Sprites::decode_sprite`], matching
    /// [`Sprites::insert`] (including layer assignment, still-sprite dedup and
    /// the missing-texture fallback).
    pub(crate) fn insert_decoded(
        &mut self,
        name: &str,
        decoded: std::result::Result<crate::sprite_cache::Decoded, String>,
    ) -> Result<u32> {
        if let Some(layer) = self.layers.get(name) {
            return Ok(*layer);
        }
        match decoded.and_then(|decoded| {
            if let Some(disk) = &mut self.disk {
                disk.insert(name, decoded.clone());
            }
            self.register(name, decoded).map_err(|e| e.to_string())
        }) {
            Ok(layer) => Ok(layer),
            Err(e) => self.insert_missing(name, &e),
        }
    }
    fn insert_missing(&mut self, name: &str, e: &dyn std::fmt::Display) -> Result<u32> {
        // `MissingTextureAtlasSprite.getLocation()`: vanilla models name the
        // missing texture on purpose (block/air.json's particle), #853.
        if name != "minecraft:missingno" {
            log::warn!("Horizon: using the missing texture for {name}: {e}");
        }
        // MissingTextureAtlasSprite.generateMissingImage: 16x16,
        // magenta and black quarters.
        let frame: Vec<u8> = (0..SPRITE_SIZE * SPRITE_SIZE)
            .flat_map(|i| {
                let (x, y) = (i % SPRITE_SIZE, i / SPRITE_SIZE);
                if (y < SPRITE_SIZE / 2) ^ (x < SPRITE_SIZE / 2) {
                    [248, 0, 248, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
            .collect();
        let layer = u32::try_from(self.native.len())?;
        self.opaque.push(true);
        self.translucent.push(false);
        self.mips.push(crate::mipmap::Settings::default());
        self.layers.insert(name.to_owned(), layer);
        self.native.push((SPRITE_SIZE, Arc::new(vec![frame]), 0));
        Ok(layer)
    }
    /// Decode the textures `names` that the disk cache lacks, in parallel,
    /// into the cache. Failures are left for [`Sprites::insert`] to report.
    pub fn prefetch<'a>(&mut self, root: &Path, names: impl IntoIterator<Item = &'a str>) {
        const CHUNK: usize = 64;
        let Some(disk) = &mut self.disk else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let missing: Vec<&str> = names
            .into_iter()
            .filter(|name| disk.get(name).is_none() && seen.insert(*name))
            .collect();
        for chunk in missing.chunks(CHUNK) {
            let decoded = crate::par::par_map(chunk, |name| Self::decode(root, name).ok());
            for (name, decoded) in chunk.iter().zip(decoded) {
                if let Some(decoded) = decoded {
                    disk.insert(name, decoded);
                }
            }
        }
    }
    fn load(&mut self, root: &Path, name: &str) -> Result<u32> {
        let cached = self.disk.as_ref().and_then(|d| d.get(name)).cloned();
        let decoded = match cached {
            Some(decoded) => decoded,
            None => {
                let decoded = Self::decode(root, name)?;
                if let Some(disk) = &mut self.disk {
                    disk.insert(name, decoded.clone());
                }
                decoded
            }
        };
        self.register(name, decoded)
    }
    /// Read and decode one texture and its `.mcmeta`.
    fn decode(root: &Path, name: &str) -> Result<crate::sprite_cache::Decoded> {
        let (namespace, path) = name.split_once(':').ok_or("invalid sprite identifier")?;
        let path = root.join(format!("assets/{namespace}/textures/{path}.png"));
        let metadata = match content_view::read_to_string(path.with_extension("png.mcmeta")) {
            Ok(text) => Some(serde_json::from_str::<
                BTreeMap<String, Box<serde_json::value::RawValue>>,
            >(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let animation_text = metadata
            .as_ref()
            .and_then(|v| v.get("animation"))
            .map(|v| v.get().to_owned());
        let animation = animation_text
            .as_deref()
            .map(AnimationMetadata::from_json)
            .transpose()?;
        let mut decoder = png::Decoder::new(content_view::open(&path)?);
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info()?;
        let mut bytes = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or("sprite exceeds decoder limits")?
        ];
        let info = reader.next_frame(&mut bytes)?;
        let (width, height) = if let Some(animation) = &animation {
            let [width, height] =
                animation.frame_size(i32::try_from(info.width)?, i32::try_from(info.height)?);
            (u32::try_from(width)?, u32::try_from(height)?)
        } else {
            (info.width, info.height)
        };
        if width == 0 || height == 0 || info.width % width != 0 || info.height % height != 0 {
            return Err(format!("sprite {name} has invalid frame dimensions").into());
        }
        let channels = info.color_type.samples();
        let mut pixels = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        for pixel in bytes[..info.buffer_size()].chunks_exact(channels) {
            let rgba = match info.color_type {
                png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                png::ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 255],
                png::ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
                _ => return Err("unsupported expanded sprite format".into()),
            };
            pixels.extend(rgba);
        }
        let columns = info.width / width;
        let count = columns * (info.height / height);
        // Layers are square; a frame of another shape is nearest-resampled to
        // its longer side so UVs (normalized per layer) are unchanged.
        let side = width.max(height);
        let mut frames = Vec::new();
        for index in 0..count {
            let x = index % columns * width;
            let y = index / columns * height;
            let mut frame = Vec::with_capacity((side * side * 4) as usize);
            for row in 0..side {
                let source_row = y + row * height / side;
                for column in 0..side {
                    let offset =
                        ((source_row * info.width + x + column * width / side) * 4) as usize;
                    frame.extend_from_slice(&pixels[offset..offset + 4]);
                }
            }
            frames.push(frame);
        }
        let texture = texture_section(metadata.as_ref());
        Ok(crate::sprite_cache::Decoded {
            side,
            frames: Arc::new(frames),
            animation: animation_text,
            strategy: texture
                .as_ref()
                .and_then(|t| t["mipmap_strategy"].as_str())
                .and_then(crate::mipmap::Strategy::parse)
                .unwrap_or_default(),
            alpha_cutoff_bias: texture
                .as_ref()
                .and_then(|t| t["alpha_cutoff_bias"].as_f64())
                .unwrap_or(0.0) as f32,
            has_transparent: pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 0),
            opaque: pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            translucent: pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] != 0 && p[3] != 255),
        })
    }
    /// Give a decoded texture its layer (or the one an identical still has).
    fn register(&mut self, name: &str, decoded: crate::sprite_cache::Decoded) -> Result<u32> {
        let crate::sprite_cache::Decoded {
            side,
            frames,
            animation: animation_text,
            strategy,
            alpha_cutoff_bias,
            has_transparent,
            opaque,
            translucent,
        } = decoded;
        let animation = animation_text
            .as_deref()
            .map(AnimationMetadata::from_json)
            .transpose()?;
        let count = frames.len() as u32;
        let layer = u32::try_from(self.native.len())?;
        let settings = crate::mipmap::Settings {
            native_size: side,
            strategy,
            alpha_cutoff_bias,
            has_transparent,
            item: name
                .split_once(':')
                .is_some_and(|(_, p)| p.starts_with("item/")),
        };
        let still = (animation.is_none() && frames.len() == 1).then(|| {
            (
                frames.clone(),
                side,
                [
                    settings.strategy as u32,
                    settings.alpha_cutoff_bias.to_bits(),
                ],
                [settings.has_transparent, settings.item, opaque, translucent],
            )
        });
        if let Some(&same) = still.as_ref().and_then(|k| self.still.get(k)) {
            self.layers.insert(name.to_owned(), same);
            return Ok(same);
        }
        if let Some(key) = still {
            self.still.insert(key, layer);
        }
        let mut initial = 0;
        if let Some(clip) = animation
            .as_ref()
            .map(|m| m.clip(count))
            .transpose()?
            .flatten()
        {
            initial = clip.sample(0).frame as usize;
            self.animations.push(SpriteAnimation {
                layer,
                frames: frames.clone(),
                clip: Arc::new(clip),
            });
        }
        self.mips.push(settings);
        self.opaque.push(opaque);
        self.translucent.push(translucent);
        self.layers.insert(name.to_owned(), layer);
        self.native.push((side, frames, initial));
        Ok(layer)
    }
}

/// SpriteContents: the `.mcmeta` `texture` section (TextureMetadataSection's
/// mipmap strategy and alpha cutoff bias).
fn texture_section(
    metadata: Option<&BTreeMap<String, Box<serde_json::value::RawValue>>>,
) -> Option<serde_json::Value> {
    metadata
        .and_then(|v| v.get("texture"))
        .and_then(|v| serde_json::from_str(v.get()).ok())
}

#[cfg(test)]
pub(crate) fn test_animation(size: u32, layer: u32) -> SpriteAnimation {
    test_clip(size, layer, r#"{"frametime":1,"frames":[0,1]}"#)
}
#[cfg(test)]
pub(crate) fn test_clip(size: u32, layer: u32, metadata: &str) -> SpriteAnimation {
    let frame = |blue: u8| {
        (0..size * size)
            .flat_map(|i| [(i % size) as u8, (i / size) as u8, blue, 255])
            .collect()
    };
    test_frames(layer, vec![frame(0), frame(99)], metadata)
}
#[cfg(test)]
pub(crate) fn test_frames(layer: u32, frames: Vec<Vec<u8>>, metadata: &str) -> SpriteAnimation {
    let metadata = AnimationMetadata::from_json(metadata).unwrap();
    SpriteAnimation {
        layer,
        clip: Arc::new(metadata.clip(frames.len() as u32).unwrap().unwrap()),
        frames: Arc::new(frames),
    }
}

/// How many reduced copies a sprite atlas keeps: the Mipmap Levels option plus the
/// resampling-only levels an upscaled pack needs, within what the atlas holds.
pub(crate) fn sprite_mipmap_limit(requested: u32, size: u32, minimum: u32, available: u32) -> u32 {
    // Only the atlas native minimum determines how many resampling-only mips to skip.
    let upscale = (size / minimum.max(1)).max(1).ilog2();
    requested.saturating_add(upscale).min(available - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn compact_animation_restores_all_frames_and_timings_without_retaining_raw_pixels() {
        for interpolate in [false, true] {
            let metadata =
                format!(r#"{{"interpolate":{interpolate},"frames":[{{"index":1,"time":3}},0,1]}}"#);
            let original = test_frames(
                7,
                vec![vec![0; 16 * 16 * 4], vec![255; 16 * 16 * 4]],
                &metadata,
            );
            let expected_blends: Vec<_> = (0..30).map(|tick| original.blend(tick)).collect();
            let weak = Arc::downgrade(&original.frames);
            let timing = original.timing();
            let packed = PackedSpriteAnimation::new(original);
            assert!(weak.upgrade().is_none());
            assert!(
                packed
                    .frames
                    .iter()
                    .all(|frame| frame.stored_len() < frame.len())
            );
            for _ in 0..2 {
                let decoded = packed.decode();
                assert_eq!(decoded.layer, 7);
                assert_eq!(decoded.frame_count(), 2);
                assert_eq!(decoded.pixels(0), vec![0; 16 * 16 * 4]);
                assert_eq!(decoded.pixels(1), vec![255; 16 * 16 * 4]);
                for (tick, expected) in expected_blends.iter().enumerate() {
                    assert_eq!(decoded.blend(tick as u64), *expected);
                    assert_eq!(timing.blend(tick as u64), *expected);
                }
            }
        }
    }

    fn decoded(frames: Arc<Vec<Vec<u8>>>, animation: Option<&str>) -> crate::sprite_cache::Decoded {
        crate::sprite_cache::Decoded {
            side: 16,
            frames,
            animation: animation.map(str::to_owned),
            strategy: crate::mipmap::Strategy::default(),
            alpha_cutoff_bias: 0.0,
            has_transparent: false,
            opaque: true,
            translucent: false,
        }
    }

    #[test]
    fn cache_registration_and_still_keys_share_pixels() {
        let frames = Arc::new(vec![vec![255; 16 * 16 * 4]]);
        let cache_entry = decoded(frames.clone(), None);
        let mut sprites = Sprites::default();
        sprites
            .register("test:block/a", cache_entry.clone())
            .unwrap();
        assert!(Arc::ptr_eq(&frames, &sprites.native[0].1));
        assert!(Arc::ptr_eq(
            &frames,
            &sprites.still.keys().next().unwrap().0
        ));
        // Equality is by texels, not Arc identity, even for separately decoded copies.
        let copy = decoded(Arc::new((*frames).clone()), None);
        assert_eq!(sprites.register("test:block/b", copy).unwrap(), 0);
        sprites.finish();
        assert_eq!(sprites.pixels, frames[0]);
        assert_eq!(cache_entry.frames[0], frames[0]);
    }

    #[test]
    fn finish_shares_native_animations_and_resamples_without_changing_frames() {
        let frames = Arc::new(vec![vec![10; 16 * 16 * 4], vec![20; 16 * 16 * 4]]);
        let mut sprites = Sprites::default();
        sprites
            .register(
                "test:block/animated",
                decoded(
                    frames.clone(),
                    Some(r#"{"frames":[1,0],"frametime":3,"interpolate":true}"#),
                ),
            )
            .unwrap();
        sprites.finish();
        assert!(Arc::ptr_eq(&frames, &sprites.animations[0].frames));
        assert_eq!(sprites.pixels, frames[1]);
        // Adding a larger layer changes only resolution; frame order, timings,
        // and every nearest-sampled texel remain those of the original frames.
        sprites
            .native
            .push((32, Arc::new(vec![vec![30; 32 * 32 * 4]]), 0));
        sprites.finish();
        let animation = &sprites.animations[0];
        for frame in 0..2 {
            assert_eq!(animation.pixels(frame), resample(&frames[frame], 16, 32));
        }
        assert_eq!(animation.frame(0), 1);
        assert_eq!(animation.frame(3), 0);
        assert_eq!(animation.blend(1).millis, 333);
        assert_eq!(&sprites.pixels[..32 * 32 * 4], animation.pixels(1));
    }

    #[test]
    fn mipmap_option_keeps_real_16px_reductions() {
        for levels in 0..=4 {
            assert_eq!(sprite_mipmap_limit(levels, 32, 16, 6), levels + 1);
            assert_eq!(sprite_mipmap_limit(levels, 16, 16, 5), levels);
        }
    }
    #[test]
    fn archive_atlas_texture_matches_disk_sprite() {
        use ::zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};
        use std::io::Write;

        const PNG: &str = "assets/testmod/textures/block/tile.png";
        const META: &str = "assets/testmod/textures/block/tile.png.mcmeta";
        const ID: &str = "testmod:block/tile";
        let temp =
            std::env::temp_dir().join(format!("horizon-archive-sprite-{}", std::process::id()));
        let disk = temp.join("disk");
        let mounted = temp.join("mounted");
        fs::create_dir_all(disk.join(PNG).parent().unwrap()).unwrap();
        fs::create_dir_all(&mounted).unwrap();

        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 2, 4);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut pixels = [255, 0, 0, 255].repeat(4);
        pixels.extend([0, 0, 255, 255].repeat(4));
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
        let metadata = br#"{"animation":{"frames":[1,0],"frametime":3}}"#;

        let hmod = temp.join("textures.hmod");
        let mut zip = ZipWriter::new(File::create(&hmod).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in [(PNG, png.as_slice()), (META, metadata.as_slice())] {
            fs::write(disk.join(name), bytes).unwrap();
            zip.start_file(name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        drop(zip.finish().unwrap());
        let mut zip = ZipArchive::new(File::open(&hmod).unwrap()).unwrap();
        let mut index = content_view::Index::default();
        let archive = index.add_archive(&hmod).unwrap();
        for name in [PNG, META] {
            let entry = zip.by_name(name).unwrap();
            index.insert(
                name,
                content_view::Stored {
                    archive,
                    offset: entry.data_start(),
                    compressed: entry.compressed_size(),
                    size: entry.size(),
                    crc32: entry.crc32(),
                    deflated: true,
                },
            );
        }
        drop(zip);
        index.mount(&mounted, &[hmod]).unwrap();

        assert!(!mounted.join(PNG).exists());
        assert!(!mounted.join(META).exists());
        assert_eq!(crate::atlas_sources::list_textures(&mounted), [ID]);
        assert_eq!(
            crate::hud::decode(&mounted.join(PNG)).unwrap(),
            crate::hud::decode(&disk.join(PNG)).unwrap()
        );
        let mut loose = Sprites::default();
        let mut served = Sprites::with_disk_cache(&mounted);
        assert_eq!(
            served.insert(&mounted, ID).unwrap(),
            loose.insert(&disk, ID).unwrap()
        );
        loose.finish();
        served.finish();
        assert_eq!(served.pixels, loose.pixels);
        assert_eq!(served.animations[0].frame(0), 1);
        assert_eq!(served.animations[0].frame(3), 0);
        served.save_disk_cache();
        assert!(
            crate::sprite_cache::SpriteCache::open(&mounted)
                .unwrap()
                .get(ID)
                .is_some()
        );

        content_view::unmount(&mounted);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn identical_still_textures_share_a_layer() {
        let root =
            std::env::temp_dir().join(format!("horizon-sprite-share-{}", std::process::id()));
        let directory = root.join("assets/a/textures/block");
        fs::create_dir_all(&directory).unwrap();
        let write = |name: &str, red: u8| {
            let mut encoder =
                png::Encoder::new(File::create(directory.join(name)).unwrap(), 16, 16);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let pixels: Vec<u8> = (0..256).flat_map(|i| [red, i as u8, 0, 255]).collect();
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        };
        write("one.png", 10);
        write("copy.png", 10);
        write("other.png", 20);
        let mut sprites = Sprites::default();
        let one = sprites.insert(&root, "a:block/one").unwrap();
        let copy = sprites.insert(&root, "a:block/copy").unwrap();
        let other = sprites.insert(&root, "a:block/other").unwrap();
        // The same pixels as an item sprite mip differently: a layer of its own.
        fs::create_dir_all(root.join("assets/a/textures/item")).unwrap();
        fs::copy(
            directory.join("one.png"),
            root.join("assets/a/textures/item/one.png"),
        )
        .unwrap();
        let item = sprites.insert(&root, "a:item/one").unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!((one, copy, other, item), (0, 0, 1, 2));
        assert_eq!((sprites.count(), sprites.layers.len()), (3, 4));
        assert_eq!((sprites.opaque.len(), sprites.mips.len()), (3, 3));
        sprites.finish();
        assert_eq!(sprites.pixels.len(), 3 * 16 * 16 * 4);
    }
    #[test]
    fn a_32px_only_atlas_does_not_add_an_extra_mipmap() {
        let mut sprites = Sprites {
            native: vec![
                (32, Arc::new(vec![vec![255; 32 * 32 * 4]]), 0),
                (32, Arc::new(vec![vec![128; 32 * 32 * 4]]), 0),
            ],
            ..Default::default()
        };
        sprites.finish();
        assert_eq!((sprites.size, sprites.minimum_size), (32, 32));
        for level in 0..=4 {
            assert_eq!(
                sprite_mipmap_limit(level, sprites.size, sprites.minimum_size, 6),
                level
            );
        }
        sprites
            .native
            .push((16, Arc::new(vec![vec![255; 16 * 16 * 4]]), 0));
        sprites.finish();
        assert_eq!((sprites.size, sprites.minimum_size), (32, 16));
        assert_eq!(
            sprite_mipmap_limit(0, sprites.size, sprites.minimum_size, 6),
            1
        );
    }

    #[test]
    fn hud_animation_resampling_uses_the_actual_frame_size() {
        for size in [16, 32, 64] {
            let animation = test_animation(size, 0);
            let resized = animation.resized_pixels(1, 16, 16);
            let last = (15 * size / 16) as u8;
            assert_eq!(&resized[15 * 4..16 * 4], &[last, 0, 99, 255]);
            assert_eq!(&resized[255 * 4..256 * 4], &[last, last, 99, 255]);
        }
    }

    #[test]
    fn native_fire_sprites_register_full_animation_sequences() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/horizon-client")
            .join(horizon_core::VANILLA_VERSION);
        if !root
            .join("assets/minecraft/textures/block/fire_0.png")
            .exists()
        {
            return;
        }
        let mut sprites = Sprites::default();
        for name in ["fire_0", "fire_1", "soul_fire_0", "soul_fire_1"] {
            let layer = sprites
                .insert(&root, &format!("minecraft:block/{name}"))
                .unwrap();
            let animation = sprites
                .animations
                .iter()
                .find(|a| a.layer == layer)
                .expect("fire must be animated");
            assert_ne!(
                animation.pixels(animation.frame(0)),
                animation.pixels(animation.frame(1))
            );
            assert_ne!(
                animation.resized_pixels(animation.frame(0), 16, 16),
                animation.resized_pixels(animation.frame(1), 16, 16)
            );
            assert_eq!(
                animation.pixels(animation.frame(0)).len(),
                (SPRITE_SIZE * SPRITE_SIZE * 4) as usize
            );
        }
    }

    #[test]
    fn native_32_pixel_animation_frames_keep_every_texel() {
        let root = std::env::temp_dir().join(format!(
            "horizon-sprite-detail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = root.join("assets/test/textures");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("flow.png");
        let mut pixels = Vec::new();
        for y in 0..64u8 {
            for x in 0..32u8 {
                pixels.extend([x, y, 0, 255]);
            }
        }
        let mut encoder = png::Encoder::new(File::create(&path).unwrap(), 32, 64);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
        fs::write(path.with_extension("png.mcmeta"), r#"{"animation":{}}"#).unwrap();
        let mut sprites = Sprites::default();
        sprites.insert(&root, "test:flow").unwrap();
        sprites.finish();
        let size = sprites.size;
        let initial = sprites.pixels.clone();
        let animation = sprites.animations[0].clone();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            size, 32,
            "half a flowing-water frame must retain 16 texels, not 8"
        );
        assert_eq!(initial, pixels[..32 * 32 * 4]);
        assert_eq!(animation.pixels(animation.frame(1)), &pixels[32 * 32 * 4..]);
    }

    #[test]
    fn animation_boundaries_and_repeated_frames_do_not_depend_on_render_rate() {
        let animation = SpriteAnimation {
            layer: 0,
            frames: Arc::new(vec![vec![], vec![], vec![]]),
            clip: Arc::new(AnimationMetadata::from_json(r#"{"frames":[{"index":2,"time":8},{"index":1,"time":3},{"index":1,"time":2},0]}"#).unwrap().clip(3).unwrap().unwrap()),
        };
        for (tick, frame) in [
            (0, 2),
            (7, 2),
            (8, 1),
            (10, 1),
            (11, 1),
            (12, 1),
            (13, 0),
            (14, 2),
            (29, 2),
            (u64::MAX, 2),
        ] {
            assert_eq!(animation.frame(tick), frame);
        }
    }

    #[test]
    fn interpolated_animations_blend_towards_the_next_frame() {
        let animation = test_clip(16, 0, r#"{"frametime":4,"interpolate":true}"#);
        let blend = |frame, next, millis| FrameBlend {
            frame,
            next,
            millis,
        };
        for (tick, expected) in [
            (0, blend(0, 0, 0)),
            (1, blend(0, 1, 250)),
            (3, blend(0, 1, 750)),
            (4, blend(1, 1, 0)),
            (5, blend(1, 0, 250)),
            (8, blend(0, 0, 0)),
        ] {
            assert_eq!(animation.blend(tick), expected, "tick {tick}");
        }
        let stepped = test_clip(16, 0, r#"{"frametime":4}"#);
        assert_eq!(stepped.blend(1), blend(0, 0, 0));
        assert_eq!(stepped.blend(5), blend(1, 1, 0));
    }

    #[test]
    fn finish_with_cap_downscales_oversized_sprites_and_keeps_vanilla_crisp() {
        let mut s = Sprites::default();
        let small = Arc::new(vec![vec![11u8; 16 * 16 * 4]]);
        s.register("minecraft:small", decoded(small, None)).unwrap();
        let big = Arc::new(vec![vec![99u8; 256 * 256 * 4]]);
        s.register(
            "modid:big",
            crate::sprite_cache::Decoded {
                side: 256,
                frames: big,
                ..decoded(Arc::new(Vec::new()), None)
            },
        )
        .unwrap();
        s.finish_with_cap(64);
        assert_eq!(s.size, 64);
        assert_eq!(s.minimum_size, 16);
        assert_eq!(s.pixels.len(), 2 * 64 * 64 * 4);
    }

    #[test]
    fn finish_and_release_matches_finish_and_frees_staging_buffers() {
        let small = Arc::new(vec![vec![17u8; 16 * 16 * 4]]);
        let animated = Arc::new(vec![vec![33u8; 16 * 16 * 4], vec![44u8; 16 * 16 * 4]]);

        let build = || {
            let mut s = Sprites::default();
            s.register("minecraft:small", decoded(Arc::clone(&small), None))
                .unwrap();
            s.register(
                "test:anim",
                decoded(
                    Arc::clone(&animated),
                    Some(r#"{"frames":[1,0],"frametime":2}"#),
                ),
            )
            .unwrap();
            s
        };

        let mut retained = build();
        retained.finish_with_cap(0);

        let mut released = build();
        let layers = released.finish_and_release(0).unwrap();

        assert_eq!(layers, 2);
        assert_eq!(released.size, retained.size);
        assert_eq!(released.minimum_size, retained.minimum_size);
        assert_eq!(released.pixels, retained.pixels);
        assert_eq!(released.animations.len(), 1);
        assert_eq!(released.animations[0].pixels(0), animated[0].as_slice());
        assert_eq!(released.animations[0].pixels(1), animated[1].as_slice());
        assert!(released.native.is_empty());
        assert!(released.still.is_empty());
        assert!(released.opaque.is_empty());
        assert!(released.disk.is_none());
    }

    #[test]
    fn parallel_decode_and_insert_decoded_matches_serial_insert() {
        let root = std::env::temp_dir().join(format!(
            "horizon-sprite-par-decode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let block = root.join("assets/minecraft/textures/block");
        fs::create_dir_all(&block).unwrap();
        let write_png = |file: &str, h: u32, frames: &[[u8; 4]], mcmeta: Option<&str>| {
            let path = block.join(file);
            let mut encoder = png::Encoder::new(File::create(&path).unwrap(), 16, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut pixels = Vec::with_capacity((16 * h * 4) as usize);
            for color in frames {
                for _ in 0..16 * 16 {
                    pixels.extend_from_slice(color);
                }
            }
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
            if let Some(meta) = mcmeta {
                fs::write(path.with_extension("png.mcmeta"), meta).unwrap();
            }
        };
        write_png(
            "anim_a.png",
            32,
            &[[10, 20, 30, 255], [40, 50, 60, 255]],
            Some(r#"{"animation":{"frametime":3,"frames":[1,0]}}"#),
        );
        write_png(
            "anim_b.png",
            48,
            &[[70, 80, 90, 200], [100, 110, 120, 255], [130, 140, 150, 0]],
            Some(r#"{"animation":{"frametime":2,"interpolate":true}}"#),
        );
        write_png("still_one.png", 16, &[[9, 9, 9, 255]], None);
        write_png("still_dup.png", 16, &[[9, 9, 9, 255]], None);
        fs::write(block.join("broken.png"), b"not-a-png").unwrap();

        let names = [
            "minecraft:block/anim_a",
            "minecraft:block/anim_b",
            "minecraft:block/broken",
            "minecraft:block/missing",
            "minecraft:block/still_dup",
            "minecraft:block/still_one",
            "minecraft:block/anim_a",
        ];

        let mut serial = Sprites::default();
        let mut serial_layers = Vec::new();
        for name in names {
            serial_layers.push(serial.insert(&root, name).unwrap());
        }
        serial.finish();

        let decoded = crate::par::par_map(&names, |name| Sprites::decode_sprite(&root, name));
        let mut parallel = Sprites::default();
        let mut parallel_layers = Vec::new();
        for (name, item) in names.into_iter().zip(decoded) {
            parallel_layers.push(parallel.insert_decoded(name, item).unwrap());
        }
        parallel.finish();

        let _ = fs::remove_dir_all(&root);

        assert_eq!(parallel_layers, serial_layers);
        assert_eq!(parallel.layers, serial.layers);
        assert_eq!(parallel.size, serial.size);
        assert_eq!(parallel.minimum_size, serial.minimum_size);
        assert_eq!(parallel.pixels, serial.pixels);
        assert_eq!(parallel.opaque, serial.opaque);
        assert_eq!(parallel.translucent, serial.translucent);
        assert_eq!(parallel.animations.len(), serial.animations.len());
        for (p, s) in parallel.animations.iter().zip(&serial.animations) {
            assert_eq!(p.layer, s.layer);
            assert_eq!(p.frame_count(), s.frame_count());
            for frame in 0..s.frame_count() {
                assert_eq!(p.pixels(frame), s.pixels(frame));
            }
            for tick in 0..16 {
                assert_eq!(p.blend(tick), s.blend(tick));
            }
        }
    }
}
