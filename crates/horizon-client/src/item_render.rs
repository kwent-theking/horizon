//! Item rendering through the resolved item model pipeline: the
//! `ItemModelResolver` of `horizon-assets` fills an [`ItemRenderState`]
//! (`ItemStackRenderState`: layers of baked quads or special renderers, each
//! with its display transform, local transform, tints and foil), and
//! [`ItemRenderState::submit`] turns it into the quads `ItemFeatureRenderer`
//! and the special model renderers submit, in pose space. A sink then draws
//! them: [`gui`] into the HUD batch with the GUI item lighting, [`WorldSink`]
//! into entity vertices with the level's lighting.
pub mod gui;
mod special;

use crate::{
    block_entity_render::{ModelPart, diffuse},
    hud::Rect,
    mesher::{CULL_BACK, Vertex},
};
use glam::{Mat3, Mat4, Vec3};
use horizon_assets::{
    animation::AnimationMetadata,
    item_bake::{BakedItemModel, SpriteLookup, SpriteMask, bake_item_model},
    item_model::{
        ClientItem, Components, ConditionProperty, Context, DisplayContext, FoilType, ItemEnv,
        ItemModelNode, ItemModels, LayerContent, LevelState, OwnerState, SelectProperty,
        SpecialModel, StackView,
    },
    model::Direction,
    palette::PalettedPermutations,
    resolve::{MISSING_MODEL, ModelLibrary},
};
use horizon_world::nbt::Tag;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// One quad as submitted for drawing: pose-space corners, atlas UVs, the
/// transformed unit normal, ARGB colour (tint, `-1` white) and its glint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemQuad {
    pub pos: [Vec3; 4],
    pub uv: [[f32; 2]; 4],
    /// The model UVs `GLINT_TEXTURING` scrolls for the glint (vanilla's UV0,
    /// or UV3 for a special model).
    pub foil_uv: [[f32; 2]; 4],
    pub normal: Vec3,
    pub color: u32,
    pub foil: Foil,
    /// `MaterialInfo.lightEmission` (block light floor).
    pub light_emission: u8,
    /// The render type culls back faces (`entitySolid`): world drawing discards them.
    pub cull_back: bool,
    /// The quad's sprite has translucent pixels (`MaterialInfo.itemRenderType`
    /// is a translucent sheet): drawn with the translucent entities.
    pub translucent: bool,
}
/// `TextureTransform.setupGlintTexturing`'s inputs.
#[derive(Clone, Copy, Debug)]
pub struct Glint {
    /// `options.glintSpeed` (vanilla default 0.5).
    pub speed: f32,
    /// `options.glintStrength` / shader `GlintAlpha` (vanilla default 0.75).
    pub alpha: f32,
    /// `Util.getMillis()`.
    pub millis: f32,
    /// The transform's scale: `GLINT_TEXTURING`'s 8.0 for GUI items,
    /// `ENTITY_GLINT_TEXTURING`'s 0.5 for items in the world and
    /// `ARMOR_ENTITY_GLINT_TEXTURING`'s 0.16 for armour (entity.wgsl).
    pub scale: f32,
}
impl Glint {
    /// `Util.getMillis() * glintSpeed * 8.0` as vanilla's long.
    fn ticks(millis: f32, speed: f32) -> i64 {
        (f64::from(millis) * f64::from(speed) * 8.0) as i64
    }
    /// The scroll clock modulo the common period of both offsets (110000 and
    /// 30000 divide 330000), for `globals.time.w`: the entity shader derives
    /// `ARMOR_ENTITY_GLINT_TEXTURING` from it.
    pub fn scroll_ticks(millis: f32, speed: f32) -> f32 {
        (Self::ticks(millis, speed) % 330_000) as f32
    }
    /// `TextureTransform.setupGlintTexturing(scale)` applied to the model UV:
    /// translation(-o0, o1) * rotateZ(PI/18) * scale. The result repeats:
    /// the shader samples the dedicated glint texture with vanilla's
    /// repeating `GlintSampler`, so the UVs stay unwrapped.
    pub fn uv(&self, model: [f32; 2]) -> [f32; 2] {
        let ticks = Self::ticks(self.millis, self.speed);
        let offset0 = (ticks % 110_000) as f32 / 110_000.0;
        let offset1 = (ticks % 30_000) as f32 / 30_000.0;
        let angle = std::f32::consts::PI / 18.0;
        let (sin, cos) = angle.sin_cos();
        [
            model[0] * self.scale * cos - model[1] * self.scale * sin - offset0,
            model[0] * self.scale * sin + model[1] * self.scale * cos + offset1,
        ]
    }
}
/// Which glint render type a quad uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foil {
    None,
    /// `GLINT_TEXTURING` over the sprite's UVs (`itemGlintRenderType`,
    /// `entitySolidGlint`).
    Standard,
    /// `itemGlintSpecialRenderType`: compasses and clocks, a sheeted decal.
    Special,
}
/// Where submitted quads go.
pub trait ItemSink {
    fn quad(&mut self, quad: ItemQuad);
}
impl ItemSink for Vec<ItemQuad> {
    fn quad(&mut self, quad: ItemQuad) {
        self.push(quad);
    }
}

/// Data special renderers read from the game (registries, skins).
pub trait SpecialHost {
    /// The banner atlas texture of a banner pattern (`entity/banner/<asset>`).
    fn banner_texture(&self, pattern: &str) -> Option<String>;
    /// The shield atlas texture of a banner pattern (`entity/shield/<asset>`).
    fn shield_texture(&self, pattern: &str) -> Option<String>;
    /// The skin texture of a `minecraft:profile` component, if loaded.
    fn player_skin(&self, _profile: &Value) -> Option<String> {
        None
    }
}
/// A host with no registries: patterns and skins fall back to defaults.
pub struct NoHost;
impl SpecialHost for NoHost {
    fn banner_texture(&self, _: &str) -> Option<String> {
        None
    }
    fn shield_texture(&self, _: &str) -> Option<String> {
        None
    }
}

/// The geometry of one layer.
#[derive(Clone, Debug)]
pub enum LayerGeometry {
    Quads(Arc<BakedItemModel>),
    Special {
        model: SpecialModel,
        argument: Value,
    },
}
/// `ItemStackRenderState.LayerRenderState`.
#[derive(Clone, Debug)]
pub struct LayerRender {
    pub geometry: LayerGeometry,
    pub tints: Vec<i32>,
    pub foil: FoilType,
    /// `applyTransform`: the display transform, then the local transform.
    pub matrix: Mat4,
    /// `particleMaterial` (atlas sprite name).
    pub particle: String,
    pub uses_block_light: bool,
    /// Untransformed extents (quad corners or the special model's parts).
    pub extents: Arc<Vec<Vec3>>,
}
/// `ItemStackRenderState`.
#[derive(Clone, Debug)]
pub struct ItemRenderState {
    pub display: DisplayContext,
    pub layers: Vec<LayerRender>,
    pub oversized_in_gui: bool,
    pub animated: bool,
}
impl ItemRenderState {
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
    /// `usesBlockLight` of the first layer (GUI lighting choice).
    pub fn uses_block_light(&self) -> bool {
        self.layers.first().is_some_and(|l| l.uses_block_light)
    }
    /// `getModelBoundingBox`: the transformed extents of every layer, as
    /// (min, max); a zero box at the origin when there are none.
    pub fn bounding_box(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for layer in &self.layers {
            for p in layer.extents.iter() {
                let q = layer.matrix.transform_point3(*p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
        if lo.x > hi.x {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (lo, hi)
        }
    }
    /// `ItemStackRenderState.submit` under `pose`.
    pub fn submit(
        &self,
        renderer: &ItemRenderer,
        sprites: &HashMap<String, Rect>,
        host: &dyn SpecialHost,
        pose: Mat4,
        sink: &mut dyn ItemSink,
    ) {
        for layer in &self.layers {
            let m = pose * layer.matrix;
            match &layer.geometry {
                LayerGeometry::Quads(baked) => {
                    submit_quads(renderer, baked, layer, self.display, m, sprites, sink);
                }
                LayerGeometry::Special { model, argument } => special::submit(
                    renderer,
                    model,
                    argument,
                    layer.foil != FoilType::None,
                    m,
                    sprites,
                    host,
                    sink,
                ),
            }
        }
    }
}

/// The atlas name of a sprite identifier (`minecraft:item/apple` -> `item/apple`).
pub fn sprite_name(sprite: &str) -> &str {
    sprite.strip_prefix("minecraft:").unwrap_or(sprite)
}
/// `Direction.getUnitVec3f`.
pub fn direction_vector(d: Direction) -> Vec3 {
    match d {
        Direction::Down => Vec3::NEG_Y,
        Direction::Up => Vec3::Y,
        Direction::North => Vec3::NEG_Z,
        Direction::South => Vec3::Z,
        Direction::West => Vec3::NEG_X,
        Direction::East => Vec3::X,
    }
}
/// `PoseStack.Pose.transformNormal`: the normal matrix, renormalized.
pub fn transform_normal(m: Mat4, n: Vec3) -> Vec3 {
    let normal = Mat3::from_mat4(m).inverse().transpose();
    (normal * n).normalize_or_zero()
}
/// An `ARGB` int as the `u32` colour of an [`ItemQuad`].
pub fn argb(color: i32) -> u32 {
    color as u32
}

/// `ItemFeatureRenderer.prepareMainSubmit` for one cuboid layer.
fn submit_quads(
    renderer: &ItemRenderer,
    baked: &BakedItemModel,
    layer: &LayerRender,
    display: DisplayContext,
    m: Mat4,
    sprites: &HashMap<String, Rect>,
    sink: &mut dyn ItemSink,
) {
    let foil = match layer.foil {
        FoilType::None => Foil::None,
        FoilType::Standard => Foil::Standard,
        FoilType::Special => Foil::Special,
    };
    let _ = display;
    // Quads of one layer mostly share a sprite: look its translucency up once.
    let mut last: Option<(&str, bool)> = None;
    for q in &baked.quads {
        let Some(r) = sprites
            .get(sprite_name(&q.material.sprite))
            .or_else(|| sprites.get("misc/missingno"))
            .or_else(|| sprites.get("horizon/missingno"))
            .copied()
        else {
            continue;
        };
        // `getLayerColorSafe`: an untinted quad, or a tint index past the
        // tints, draws white.
        let color = if q.tint_index >= 0 {
            layer
                .tints
                .get(q.tint_index as usize)
                .copied()
                .unwrap_or(-1)
        } else {
            -1
        };
        let pos = q.positions.map(|p| m.transform_point3(Vec3::from(p)));
        let uv =
            q.uv.map(|[u, v]| [r.u0 + (r.u1 - r.u0) * u, r.v0 + (r.v1 - r.v0) * v]);
        let translucent = match last {
            Some((sprite, translucent)) if sprite == q.material.sprite => translucent,
            _ => {
                let translucent = renderer
                    .sprites
                    .translucent(sprite_name(&q.material.sprite));
                last = Some((&q.material.sprite, translucent));
                translucent
            }
        };
        sink.quad(ItemQuad {
            pos,
            uv,
            foil_uv: q.uv,
            normal: transform_normal(m, direction_vector(q.facing)),
            color: argb(color),
            foil,
            light_emission: q.light_emission,
            cull_back: false,
            translucent: translucent || q.material.force_translucent,
        });
    }
}

// ---------------------------------------------------------------------------
// Sprites for baking
// ---------------------------------------------------------------------------

/// The alpha masks of item and block sprites, read from the texture files
/// (every unique animation frame) and the items atlas's palette sprites.
struct FileSprites {
    root: PathBuf,
    permutations: Vec<PalettedPermutations>,
    cache: Mutex<HashMap<String, Option<SpriteMask>>>,
    /// Sprites with a pixel that is neither fully transparent nor opaque
    /// (`Transparency.hasTranslucent`), filled in as sprites load.
    translucent: Mutex<HashMap<String, bool>>,
}
impl SpriteLookup for FileSprites {
    fn mask(&self, sprite: &str) -> Option<SpriteMask> {
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(sprite)
        {
            return hit.clone();
        }
        let mask = self.load(sprite);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(sprite.to_owned(), mask.clone());
        }
        mask
    }
}
fn argb_pixels(rgba: &[u8]) -> Vec<u32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| u32::from_be_bytes([p[3], p[0], p[1], p[2]]))
        .collect()
}
impl FileSprites {
    fn translucent(&self, sprite: &str) -> bool {
        let known = |this: &Self| this.translucent.lock().ok()?.get(sprite).copied();
        known(self).unwrap_or_else(|| {
            self.mask(sprite);
            known(self).unwrap_or(false)
        })
    }
    fn png(&self, sprite: &str, directory: &str) -> PathBuf {
        crate::atlas_sources::texture_path(&self.root, sprite, directory)
    }
    fn image(&self, sprite: &str, directory: &str) -> Option<(usize, usize, Vec<u32>)> {
        let (w, h, rgba) = crate::hud::decode(&self.png(sprite, directory)).ok()?;
        Some((w as usize, h as usize, argb_pixels(&rgba)))
    }
    fn load(&self, sprite: &str) -> Option<SpriteMask> {
        let png = self.png(sprite, "textures");
        let (iw, ih, argb) = match self.image(sprite, "textures") {
            Some(image) => image,
            None => {
                let p = self.permutations.iter().find_map(|s| s.find(sprite))?;
                let (w, h, mut pixels) = self.image(p.base, "textures")?;
                let (_, _, key) = self.image(p.palette_key, "textures/palettes")?;
                let (_, _, target) = self.image(p.palette, "textures/palettes")?;
                horizon_assets::palette::apply_palette(&mut pixels, &key, &target)?;
                (w, h, pixels)
            }
        };
        let translucent = argb.iter().any(|p| matches!(p >> 24, 1..=254));
        if let Ok(mut cache) = self.translucent.lock() {
            cache.insert(sprite.to_owned(), translucent);
        }
        let alpha: Vec<u8> = argb.iter().map(|p| (p >> 24) as u8).collect();
        let animation = fs::read_to_string(png.with_extension("png.mcmeta"))
            .ok()
            .and_then(|m| serde_json::from_str::<Value>(&m).ok())
            .and_then(|m| m.get("animation").map(Value::to_string))
            .and_then(|a| AnimationMetadata::from_json(&a).ok());
        let (w, h) = match &animation {
            Some(a) => {
                let [w, h] = a.frame_size(iw as i32, ih as i32);
                (w as usize, h as usize)
            }
            None => (iw, ih),
        };
        if w == 0 || h == 0 {
            return None;
        }
        let columns = iw / w;
        let total = columns * (ih / h);
        let clip = animation.and_then(|a| a.clip(total as u32).ok().flatten());
        let frames: Vec<usize> = match clip {
            Some(clip) => {
                let mut unique = Vec::new();
                for (index, _) in clip.sequence() {
                    if !unique.contains(&(*index as usize)) {
                        unique.push(*index as usize);
                    }
                }
                unique
            }
            None => vec![0],
        };
        let frames = frames
            .into_iter()
            .map(|f| {
                let (fx, fy) = (f % columns * w, f / columns * h);
                (0..h)
                    .flat_map(|y| (0..w).map(move |x| (x, y)))
                    .map(|(x, y)| alpha[(fy + y) * iw + fx + x] == 0)
                    .collect()
            })
            .collect();
        Some(SpriteMask {
            width: w,
            height: h,
            frames,
        })
    }
}

// ---------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------

/// The baked item models and everything resolution needs.
pub struct ItemRenderer {
    pub models: ItemModels,
    library: Arc<ModelLibrary>,
    sprites: FileSprites,
    baked: Mutex<HashMap<String, Arc<BakedItemModel>>>,
    extents: Mutex<HashMap<String, Arc<Vec<Vec3>>>>,
    /// GUI items already drawn: the HUD vertices of a static item at a GUI position
    /// (`gui::draw`), which are the same every frame it stays there.
    pub(crate) gui_drawn: Mutex<HashMap<gui::DrawnKey, Arc<[crate::hud::HudVertex]>>>,
    /// Entity model layers of the special renderers.
    parts: HashMap<String, ModelPart>,
    /// `colormap/grass.png` as ARGB.
    grass: Vec<i32>,
    /// `minecraft:item` registry ids (`Item.getId`).
    item_ids: HashMap<String, i32>,
}

/// Every `items/**/*.json` client item definition, by identifier.
fn load_definitions(root: &Path) -> BTreeMap<String, ClientItem> {
    fn collect_paths(
        dir: &Path,
        rel: &str,
        namespace: &str,
        out: &mut Vec<(String, std::path::PathBuf)>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if path.is_dir() {
                collect_paths(&path, &rel, namespace, out);
            } else if let Some(id) = rel.strip_suffix(".json") {
                out.push((format!("{namespace}:{id}"), path));
            }
        }
    }
    let cache = horizon_world::resource_cache::Cache::for_tree(root, "client-item-defs-v1", &[]);
    let threads = horizon_core::par::threads();
    if let Some(raw) = cache
        .as_ref()
        .and_then(|c| c.read_value::<Vec<(String, String)>>())
        .filter(|raw| raw.iter().all(|(id, _)| !id.is_empty()))
    {
        let mut parsed = BTreeMap::new();
        let mut all_valid = true;
        horizon_core::par::map_batches_with(
            threads,
            raw,
            256,
            |(id, text)| {
                ClientItem::from_json(text)
                    .ok()
                    .map(|item| (id.clone(), item))
            },
            |entry| {
                if let Some((id, item)) = entry {
                    parsed.insert(id, item);
                } else {
                    all_valid = false;
                }
            },
        );
        if all_valid {
            return parsed;
        }
    }
    let mut paths = Vec::new();
    let Ok(namespaces) = fs::read_dir(root.join("assets")) else {
        return BTreeMap::new();
    };
    for ns in namespaces.flatten() {
        let namespace = ns.file_name().to_string_lossy().into_owned();
        collect_paths(&ns.path().join("items"), "", &namespace, &mut paths);
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    let mut raw = Vec::new();
    let mut out = BTreeMap::new();
    horizon_core::par::map_batches_with(
        threads,
        paths,
        256,
        |(id, path)| {
            let text = fs::read_to_string(path).ok()?;
            let item = ClientItem::from_json(&text).ok()?;
            Some((id.clone(), text, item))
        },
        |entry| {
            if let Some((id, text, item)) = entry {
                raw.push((id.clone(), text));
                out.insert(id, item);
            }
        },
    );
    if let Some(cache) = cache {
        let _ = cache.write_value(&raw);
    }
    out
}

fn load_item_ids(root: &Path) -> HashMap<String, i32> {
    let Some(reg_path) = horizon_protocol::registry::Builtin::find(root) else {
        return HashMap::new();
    };
    let cache = horizon_world::resource_cache::Cache::new(
        root,
        "item-registry-ids-v1",
        &[reg_path.as_path()],
        &[],
    );
    if let Some(map) = cache
        .as_ref()
        .and_then(|c| c.read_value::<HashMap<String, i32>>())
        .filter(|map| map.iter().all(|(k, &v)| !k.is_empty() && v >= 0))
    {
        return map;
    }
    let map = horizon_protocol::registry::Builtin::load(&reg_path)
        .ok()
        .and_then(|b| {
            let ids = b.get("minecraft:item")?;
            let mut out = HashMap::new();
            for i in 0.. {
                let Some(name) = ids.name(i) else { break };
                out.insert(name.to_owned(), i);
            }
            Some(out)
        })
        .unwrap_or_default();
    if let Some(cache) = cache
        && !map.is_empty()
    {
        let _ = cache.write_value(&map);
    }
    map
}

/// Components kept whole: the item resolver reads some, and the game reads the rest from an
/// item's defaults (`game/hand.rs`, `game/item_draw.rs`, `game/totem.rs`, `game.rs`).
const FIXED_KEPT_COMPONENTS: &[&str] = &[
    "minecraft:attack_range",
    "minecraft:item_model",
    "minecraft:enchantment_glint_override",
    "minecraft:enchantments",
    "minecraft:lodestone_tracker",
    "minecraft:max_damage",
    "minecraft:unbreakable",
    "minecraft:damage",
    "minecraft:max_stack_size",
    "minecraft:bundle_contents",
    "minecraft:custom_model_data",
    "minecraft:charged_projectiles",
    "minecraft:trim",
    "minecraft:block_state",
    "minecraft:dyed_color",
    "minecraft:firework_explosion",
    "minecraft:potion_contents",
    "minecraft:banner_patterns",
    "minecraft:pot_decorations",
    "minecraft:profile",
    "minecraft:base_color",
    "minecraft:consumable",
    "minecraft:blocks_attacks",
    "minecraft:kinetic_weapon",
    "minecraft:death_protection",
];

fn collect_referenced_components(node: &ItemModelNode, out: &mut HashSet<String>) {
    match node {
        ItemModelNode::Empty | ItemModelNode::Model(_) | ItemModelNode::BundleSelectedItem => {}
        ItemModelNode::Composite { models, .. } => {
            for model in models {
                collect_referenced_components(model, out);
            }
        }
        ItemModelNode::Condition {
            property,
            on_true,
            on_false,
            ..
        } => {
            if let ConditionProperty::Component(predicate) = property
                && let Some(map) = predicate.as_object()
            {
                for key in map.keys() {
                    out.insert(key.clone());
                }
            }
            collect_referenced_components(on_true, out);
            collect_referenced_components(on_false, out);
        }
        ItemModelNode::Select {
            property,
            cases,
            fallback,
            ..
        } => {
            if let SelectProperty::Component(component) = property {
                out.insert(component.clone());
            }
            for case in cases {
                collect_referenced_components(&case.model, out);
            }
            if let Some(fallback) = fallback {
                collect_referenced_components(fallback, out);
            }
        }
        ItemModelNode::RangeDispatch {
            entries, fallback, ..
        } => {
            for (_, model) in entries {
                collect_referenced_components(model, out);
            }
            if let Some(fallback) = fallback {
                collect_referenced_components(fallback, out);
            }
        }
        ItemModelNode::Special { .. } => {}
    }
}

/// Deserialize `items.json` while discarding heavy component JSON payloads
/// that neither item model resolution nor item draw code inspects. Pruned
/// keys remain in each item's `Components` map as `Value::Null` so presence
/// queries such as `minecraft:has_component` keep vanilla behaviour.
fn load_prototypes_pruned(
    bytes: &[u8],
    definitions: &BTreeMap<String, ClientItem>,
) -> std::result::Result<BTreeMap<String, Components>, serde_json::Error> {
    use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

    let mut kept: HashSet<String> = FIXED_KEPT_COMPONENTS
        .iter()
        .map(|&s| s.to_owned())
        .collect();
    for def in definitions.values() {
        collect_referenced_components(&def.model, &mut kept);
    }

    struct ComponentMapSeed<'a>(&'a HashSet<String>);
    impl<'de> serde::de::DeserializeSeed<'de> for ComponentMapSeed<'_> {
        type Value = Components;
        fn deserialize<D: Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> std::result::Result<Self::Value, D::Error> {
            struct ComponentVisitor<'a>(&'a HashSet<String>);
            impl<'de> Visitor<'de> for ComponentVisitor<'_> {
                type Value = Components;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("component map")
                }
                fn visit_map<A: MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> std::result::Result<Self::Value, A::Error> {
                    let mut out = Components::new();
                    while let Some(key) = map.next_key::<String>()? {
                        let value = if self.0.contains(&key) {
                            map.next_value::<Value>()?
                        } else {
                            map.next_value::<IgnoredAny>()?;
                            Value::Null
                        };
                        out.insert(key, value);
                    }
                    Ok(out)
                }
            }
            deserializer.deserialize_map(ComponentVisitor(self.0))
        }
    }

    struct PrototypesVisitor<'a>(&'a HashSet<String>);
    impl<'de> Visitor<'de> for PrototypesVisitor<'_> {
        type Value = BTreeMap<String, Components>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("item prototypes map")
        }
        fn visit_map<A: MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some(item) = map.next_key::<String>()? {
                let components = map.next_value_seed(ComponentMapSeed(self.0))?;
                out.insert(item, components);
            }
            Ok(out)
        }
    }

    let mut de = serde_json::Deserializer::from_slice(bytes);
    let out = de.deserialize_map(PrototypesVisitor(&kept))?;
    de.end()?;
    Ok(out)
}

impl ItemRenderer {
    /// Load item definitions, item prototypes (`items.json`), every model,
    /// the special renderers' entity models and the grass colormap.
    pub fn load(root: &Path) -> crate::Result<Self> {
        Self::load_with_library(
            root,
            crate::model_library::ParsedModels::load(root).into_item_library(),
        )
    }
    /// Reuse the immutable models prepared with the block resources at `root`.
    /// Defer item-only validation failures until after item definitions are loaded.
    pub fn load_with_library(
        root: &Path,
        library: std::result::Result<Arc<ModelLibrary>, String>,
    ) -> crate::Result<Self> {
        let definitions = load_definitions(root);
        let prototypes = {
            let bytes = fs::read(root.join("items.json"))?;
            load_prototypes_pruned(&bytes, &definitions)?
        };
        let models = ItemModels {
            definitions,
            prototypes,
        };
        let library = library?;
        let permutations = PalettedPermutations::from_atlas(
            &fs::read_to_string(root.join("assets/minecraft/atlases/items.json"))
                .unwrap_or_default(),
        );
        let parts = special::load_parts(root);
        let grass = crate::hud::decode(&root.join("assets/minecraft/textures/colormap/grass.png"))
            .map(|(_, _, rgba)| argb_pixels(&rgba).into_iter().map(|p| p as i32).collect())
            .unwrap_or_default();
        let item_ids = load_item_ids(root);
        Ok(Self {
            item_ids,
            models,
            library,
            sprites: FileSprites {
                root: root.to_path_buf(),
                permutations,
                cache: Mutex::default(),
                translucent: Mutex::default(),
            },
            baked: Mutex::default(),
            extents: Mutex::default(),
            gui_drawn: Mutex::default(),
            parts,
            grass,
        })
    }
    /// The parsed model library loaded with this renderer.
    pub fn library(&self) -> &Arc<ModelLibrary> {
        &self.library
    }
    /// `ModelBakery` item baking, cached per model.
    pub fn bake(&self, model: &str) -> Arc<BakedItemModel> {
        if let Ok(cache) = self.baked.lock()
            && let Some(hit) = cache.get(model)
        {
            return hit.clone();
        }
        let baked = bake_item_model(&self.library, &self.sprites, model)
            .or_else(|_| bake_item_model(&self.library, &self.sprites, MISSING_MODEL))
            .map(Arc::new)
            .unwrap_or_else(|_| {
                Arc::new(BakedItemModel {
                    quads: Vec::new(),
                    uses_block_light: false,
                    particle: horizon_assets::model::Material {
                        sprite: horizon_assets::item_bake::MISSING_SPRITE.to_owned(),
                        force_translucent: false,
                    },
                    transforms: Default::default(),
                })
            });
        if let Ok(mut cache) = self.baked.lock() {
            cache.insert(model.to_owned(), baked.clone());
        }
        baked
    }
    /// `CuboidItemModelWrapper.computeExtents`: the distinct quad corners.
    fn quad_extents(&self, model: &str, baked: &BakedItemModel) -> Arc<Vec<Vec3>> {
        if let Ok(cache) = self.extents.lock()
            && let Some(hit) = cache.get(model)
        {
            return hit.clone();
        }
        let mut out: Vec<Vec3> = Vec::new();
        for q in &baked.quads {
            for p in q.positions {
                let p = Vec3::from(p);
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        let out = Arc::new(out);
        if let Ok(mut cache) = self.extents.lock() {
            cache.insert(model.to_owned(), out.clone());
        }
        out
    }
    fn special_extents(&self, model: &SpecialModel) -> Arc<Vec<Vec3>> {
        let key = model.to_json().to_string();
        if let Ok(cache) = self.extents.lock()
            && let Some(hit) = cache.get(&key)
        {
            return hit.clone();
        }
        let out = Arc::new(special::extents(self, model));
        if let Ok(mut cache) = self.extents.lock() {
            cache.insert(key, out.clone());
        }
        out
    }
    /// `ItemModelResolver.updateForTopItem`.
    pub fn resolve(
        &self,
        stack: &StackView,
        display: DisplayContext,
        owner: Option<&OwnerState>,
        level: Option<&LevelState>,
        seed: i32,
        env: &dyn ItemEnv,
    ) -> ItemRenderState {
        let ctx = Context {
            display,
            owner,
            level,
            seed,
        };
        let resolved = self.models.resolve(stack, &ctx, env);
        let mut animated = false;
        let layers = resolved
            .layers
            .into_iter()
            .map(|layer| {
                let (geometry, baked, extents) = match layer.content {
                    LayerContent::Model { model } => {
                        let baked = self.bake(&model);
                        let extents = self.quad_extents(&model, &baked);
                        (LayerGeometry::Quads(baked.clone()), baked, extents)
                    }
                    LayerContent::Missing => {
                        let baked = self.bake(MISSING_MODEL);
                        let extents = self.quad_extents(MISSING_MODEL, &baked);
                        (LayerGeometry::Quads(baked.clone()), baked, extents)
                    }
                    LayerContent::Special {
                        base,
                        model,
                        argument,
                    } => {
                        let baked = self.bake(&base);
                        let extents = self.special_extents(&model);
                        (LayerGeometry::Special { model, argument }, baked, extents)
                    }
                };
                if layer.foil != FoilType::None {
                    animated = true;
                }
                LayerRender {
                    geometry,
                    tints: layer.tints,
                    foil: layer.foil,
                    matrix: glam_matrix(&baked.layer_matrix(display, &layer.transform)),
                    uses_block_light: baked.uses_block_light,
                    particle: sprite_name(&baked.particle.sprite).to_owned(),
                    extents,
                }
            })
            .collect();
        ItemRenderState {
            display,
            layers,
            oversized_in_gui: resolved.oversized_in_gui,
            animated,
        }
    }
    /// `GrassColor.get` over the loaded colormap.
    pub fn grass_color(&self, temperature: f64, downfall: f64) -> i32 {
        let rain = downfall * temperature;
        let x = ((1.0 - temperature) * 255.0) as i64;
        let y = ((1.0 - rain) * 255.0) as i64;
        let index = (y << 8 | x) as usize;
        self.grass.get(index).copied().unwrap_or(-65281)
    }
    /// Submit a special model renderer directly (worn skulls of
    /// `CustomHeadLayer`, which draw the skull model, not the item).
    #[allow(clippy::too_many_arguments)]
    pub fn submit_special(
        &self,
        model: &SpecialModel,
        argument: &Value,
        has_foil: bool,
        pose: Mat4,
        sprites: &HashMap<String, Rect>,
        host: &dyn SpecialHost,
        sink: &mut dyn ItemSink,
    ) {
        special::submit(self, model, argument, has_foil, pose, sprites, host, sink);
    }
    /// `Item.getId` (0 when the registry report is missing).
    pub fn item_id(&self, item: &str) -> i32 {
        self.item_ids.get(item).copied().unwrap_or(0)
    }
    pub(crate) fn part(&self, key: &str) -> Option<&ModelPart> {
        self.parts.get(key)
    }
}
/// A `horizon-assets` (row-major, JOML-exact) matrix as glam's.
pub fn glam_matrix(m: &horizon_assets::math::Mat4) -> Mat4 {
    Mat4::from_cols_array_2d(&m.rows()).transpose()
}

// ---------------------------------------------------------------------------
// Stacks
// ---------------------------------------------------------------------------

/// An NBT value as the JSON its component codec reads.
pub fn tag_json(tag: &Tag) -> Value {
    match tag {
        Tag::End => Value::Null,
        Tag::Byte(v) => Value::from(*v),
        Tag::Short(v) => Value::from(*v),
        Tag::Int(v) => Value::from(*v),
        Tag::Long(v) => Value::from(*v),
        Tag::Float(bits) => serde_json::Number::from_f64(f64::from(f32::from_bits(*bits)))
            .map_or(Value::Null, Value::Number),
        Tag::Double(bits) => {
            serde_json::Number::from_f64(f64::from_bits(*bits)).map_or(Value::Null, Value::Number)
        }
        Tag::ByteArray(a) => Value::Array(a.iter().map(|b| Value::from(*b as i8)).collect()),
        Tag::String(s) => Value::String(s.to_string_lossy()),
        Tag::List(l) => Value::Array(l.iter().map(tag_json).collect()),
        Tag::Compound(c) => Value::Object(
            c.iter()
                .map(|(k, v)| (k.to_string_lossy(), tag_json(v)))
                .collect(),
        ),
        Tag::IntArray(a) => Value::Array(a.iter().map(|v| Value::from(*v)).collect()),
        Tag::LongArray(a) => Value::Array(a.iter().map(|v| Value::from(*v)).collect()),
    }
}
/// The resolver's view of a client stack: its item, count and component patch.
pub fn stack_view(stack: &crate::items::ItemStack) -> StackView {
    let mut view = StackView::new(&stack.item, stack.count as i32);
    view.bundle_selected = stack.data().map_or(-1, |d| d.bundle_selected.0);
    if let Some(Tag::Compound(c)) = crate::item_data::components_tag(stack.damage, stack.data()) {
        for (k, v) in &c {
            let key = k.to_string_lossy();
            match key.strip_prefix('!') {
                Some(removed) => view.patch.insert(removed.to_owned(), None),
                None => view.patch.insert(key, Some(tag_json(v))),
            };
        }
    }
    view
}

// ---------------------------------------------------------------------------
// World drawing
// ---------------------------------------------------------------------------

/// Entity vertices for submitted quads: the `item`/`entity` shader's
/// `minecraft_mix_light` with the level's diffuse lights on the vertex
/// colour, the packed lightmap coordinates (raised by light emission) and
/// overlay 7 for the glint.
pub struct WorldSink<'a> {
    pub out: &'a mut Vec<Vertex>,
    /// Packed light (`block_entity_render::vertex_light`).
    pub light: u32,
    /// The level's diffuse lights; `None` leaves the colour unshaded.
    pub lights: Option<(Vec3, Vec3)>,
    /// The `GLINT_TEXTURING` sample for foil quads (`None` leaves them unglinted).
    pub glint: Option<Glint>,
}
impl ItemSink for WorldSink<'_> {
    fn quad(&mut self, q: ItemQuad) {
        let brightness = self.lights.map_or(1.0, |l| diffuse(q.normal, l));
        let [a, r, g, b] = q.color.to_be_bytes();
        let c = |v: u8| (f32::from(v) * brightness) as u32;
        let color = u32::from(a) << 24 | c(b) << 16 | c(g) << 8 | c(r);
        // `LightCoordsUtil.lightCoordsWithEmission`: block light at least the emission.
        let emission = u32::from(q.light_emission) * 16;
        let light = if (self.light >> 8 & 0xFF) < emission {
            (self.light & 0xFF) | emission << 8
        } else {
            self.light
        };
        let glint = if q.foil == Foil::None {
            None
        } else {
            self.glint
        };
        let overlay = match (glint.is_some(), q.translucent) {
            (false, true) => crate::gfx::TRANSLUCENT_ITEM_OVERLAY,
            (false, false) => 0,
            (true, true) => crate::gfx::TRANSLUCENT_GLINT_ITEM_OVERLAY,
            (true, false) => 7,
        };
        for (i, (p, uv)) in q.pos.iter().zip(q.uv).enumerate() {
            let glint_uv = glint.map_or([0.0, 0.0], |glint| glint.uv(q.foil_uv[i]));
            self.out.push(Vertex {
                normal: if q.cull_back { CULL_BACK } else { [0; 4] },
                pos: p.to_array(),
                uv,
                layer: overlay,
                color,
                light,
                glint_uv,
            });
        }
    }
}

/// `ItemEntity.getSpin`.
pub fn item_spin(age_in_ticks: f32, bob_offset: f32) -> f32 {
    age_in_ticks / 20.0 + bob_offset
}
/// `ItemClusterRenderState.getRenderedAmount`.
pub fn rendered_amount(count: u32) -> u32 {
    match count {
        0..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    }
}
/// `ItemEntityRenderer.submitMultipleFromCount`: the poses of each copy of
/// a stack (`seed` is `ItemClusterRenderState.getSeedForItemStack`).
pub fn cluster_poses(state: &ItemRenderState, pose: Mat4, amount: u32, seed: i64) -> Vec<Mat4> {
    use horizon_core::random::{LegacyRandom, RandomSource};
    let mut out = Vec::new();
    if amount == 0 {
        return out;
    }
    let mut random = LegacyRandom::new(seed);
    let (lo, hi) = state.bounding_box();
    let depth = hi.z - lo.z;
    let mut next = || random.next_float() * 2.0 - 1.0;
    if depth > 0.0625 {
        out.push(pose);
        for _ in 1..amount {
            let (x, y, z) = (next() * 0.15, next() * 0.15, next() * 0.15);
            out.push(pose * Mat4::from_translation(Vec3::new(x, y, z)));
        }
    } else {
        let offset = depth * 1.5;
        let mut p = pose
            * Mat4::from_translation(Vec3::new(0.0, 0.0, -(offset * (amount - 1) as f32 / 2.0)));
        out.push(p);
        p *= Mat4::from_translation(Vec3::new(0.0, 0.0, offset));
        for _ in 1..amount {
            let (x, y) = (next() * 0.15 * 0.5, next() * 0.15 * 0.5);
            out.push(p * Mat4::from_translation(Vec3::new(x, y, 0.0)));
            p *= Mat4::from_translation(Vec3::new(0.0, 0.0, offset));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bad item model surfaces only once items.json has loaded, so a broken
    /// items.json still reports itself first.
    #[test]
    fn item_model_errors_wait_for_the_item_definitions() -> crate::Result<()> {
        let root =
            std::env::temp_dir().join(format!("horizon-item-model-errors-{}", std::process::id()));
        let model = root.join("assets/horizon/models/INVALID.json");
        fs::create_dir_all(model.parent().unwrap())?;
        fs::write(model, "{}")?;
        let library = crate::model_library::ParsedModels::load(&root).into_item_library();
        assert!(library.is_err());
        fs::write(root.join("items.json"), "{")?;
        let load = || {
            ItemRenderer::load_with_library(&root, library.clone())
                .err()
                .unwrap()
                .to_string()
        };
        let broken_items = load();
        fs::write(root.join("items.json"), "{}")?;
        let bad_model = load();
        let _ = fs::remove_dir_all(&root);
        assert!(!broken_items.starts_with("item models:"));
        assert!(bad_model.starts_with("item models:"));
        Ok(())
    }

    /// Definitions live at any depth (`items/<dir>/<name>.json`), as
    /// ModelEngine's `modelengine:postman/head` does.
    #[test]
    fn item_definitions_load_from_nested_folders() -> crate::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "horizon-item-definitions-nested-{}",
            std::process::id()
        ));
        let definitions = root.join("assets/horizon/items");
        fs::create_dir_all(definitions.join("postman"))?;
        let empty = r#"{"model":{"type":"minecraft:empty"}}"#;
        fs::write(definitions.join("top.json"), empty)?;
        fs::write(definitions.join("postman/head.json"), empty)?;
        let loaded = load_definitions(&root);
        let _ = fs::remove_dir_all(&root);
        assert!(loaded.contains_key("horizon:top"));
        assert!(loaded.contains_key("horizon:postman/head"));
        Ok(())
    }

    #[test]
    fn rendered_amounts_follow_the_count_bands() {
        let bands: Vec<u32> = [0, 1, 2, 16, 17, 32, 33, 48, 49, 64]
            .iter()
            .map(|c| rendered_amount(*c))
            .collect();
        assert_eq!(bands, [1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
    }
    #[test]
    fn nbt_components_read_as_json() {
        let tag = Tag::compound([
            ("a", Tag::Int(3)),
            ("b", Tag::List(vec![Tag::String("x".into())])),
            ("c", Tag::Float(1.5f32.to_bits())),
        ]);
        assert_eq!(
            tag_json(&tag),
            serde_json::json!({"a": 3, "b": ["x"], "c": 1.5})
        );
    }
    #[test]
    fn normals_follow_the_inverse_transpose() {
        let m = Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0));
        assert_eq!(transform_normal(m, Vec3::X), Vec3::X);
        let r = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert!((transform_normal(r, Vec3::Z) - Vec3::X).length() < 1e-6);
    }

    #[test]
    fn definitions_and_item_ids_snapshots_hit_recover_and_invalidate() {
        let temp =
            std::env::temp_dir().join(format!("horizon-item-defs-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let items_dir = temp.join("assets/minecraft/items");
        fs::create_dir_all(&items_dir).unwrap();
        fs::write(temp.join("horizon-mod-content.stamp"), "item-defs-test-v1").unwrap();
        fs::write(
            items_dir.join("stick.json"),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:item/stick"}}"#,
        )
        .unwrap();
        let reg_path = temp.join("registries.json");
        fs::write(
            &reg_path,
            r#"{"minecraft:item":{"default":"minecraft:air","entries":{"minecraft:air":{"protocol_id":0},"minecraft:stone":{"protocol_id":1}},"protocol_id":5}}"#,
        )
        .unwrap();

        let defs = load_definitions(&temp);
        assert!(defs.contains_key("minecraft:stick"));
        let ids = load_item_ids(&temp);
        assert_eq!(ids.get("minecraft:stone"), Some(&1));

        // Prove cache hit path runs without scanning files again.
        let defs_cache =
            horizon_world::resource_cache::Cache::for_tree(&temp, "client-item-defs-v1", &[])
                .unwrap();
        let mut seeded_defs = defs_cache.read_value::<Vec<(String, String)>>().unwrap();
        seeded_defs.push((
            "minecraft:cached_marker".into(),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:item/stick"}}"#.into(),
        ));
        defs_cache.write_value(&seeded_defs).unwrap();
        assert!(load_definitions(&temp).contains_key("minecraft:cached_marker"));

        let ids_cache = horizon_world::resource_cache::Cache::new(
            &temp,
            "item-registry-ids-v1",
            &[reg_path.as_path()],
            &[],
        )
        .unwrap();
        let mut seeded_ids = ids_cache.read_value::<HashMap<String, i32>>().unwrap();
        seeded_ids.insert("minecraft:cached_marker".into(), 99);
        ids_cache.write_value(&seeded_ids).unwrap();
        assert_eq!(
            load_item_ids(&temp).get("minecraft:cached_marker"),
            Some(&99)
        );

        // Corrupt snapshots and confirm fallback rebuilds cleanly.
        for entry in fs::read_dir(temp.join(".horizon-derived"))
            .unwrap()
            .flatten()
        {
            if entry.path().is_file() {
                fs::write(entry.path(), b"corrupted").unwrap();
            }
        }
        let rebuilt_defs = load_definitions(&temp);
        assert!(rebuilt_defs.contains_key("minecraft:stick"));
        assert!(!rebuilt_defs.contains_key("minecraft:cached_marker"));
        let rebuilt_ids = load_item_ids(&temp);
        assert_eq!(rebuilt_ids.get("minecraft:stone"), Some(&1));
        assert!(!rebuilt_ids.contains_key("minecraft:cached_marker"));

        // Modifying source files invalidates snapshots.
        fs::write(
            items_dir.join("blaze_rod.json"),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:item/blaze_rod"}}"#,
        )
        .unwrap();
        fs::write(
            &reg_path,
            r#"{"minecraft:item":{"default":"minecraft:air","entries":{"minecraft:air":{"protocol_id":0},"minecraft:stone":{"protocol_id":1},"minecraft:dirt":{"protocol_id":2}},"protocol_id":5}}"#,
        )
        .unwrap();
        let updated_defs = load_definitions(&temp);
        assert!(updated_defs.contains_key("minecraft:blaze_rod"));
        assert_eq!(load_item_ids(&temp).get("minecraft:dirt"), Some(&2));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn pruned_prototypes_keep_components_the_game_reads() {
        let read_by_game = [
            "minecraft:attack_range",
            "minecraft:consumable",
            "minecraft:blocks_attacks",
            "minecraft:kinetic_weapon",
            "minecraft:death_protection",
            "minecraft:item_model",
            "minecraft:max_stack_size",
            "minecraft:bundle_contents",
        ];
        let body = read_by_game
            .iter()
            .map(|id| format!("\"{id}\": {{\"kept\": true}}"))
            .collect::<Vec<_>>()
            .join(",");
        let raw = format!("{{\"minecraft:trident\": {{{body}, \"minecraft:tool\": {{\"x\": 1}}}}}}");
        let prototypes = load_prototypes_pruned(raw.as_bytes(), &BTreeMap::new()).unwrap();
        let components = &prototypes["minecraft:trident"];
        for id in read_by_game {
            assert_eq!(
                components.get(id),
                Some(&serde_json::json!({"kept": true})),
                "{id} was pruned"
            );
        }
        assert_eq!(components.get("minecraft:tool"), Some(&Value::Null));
    }

    #[test]
    fn pruned_prototypes_keep_has_component_and_select_component_values() {
        let def = ClientItem::from_json(
            r#"{
                "model": {
                    "type": "minecraft:condition",
                    "property": "minecraft:has_component",
                    "component": "minecraft:tool",
                    "on_true": {
                        "type": "minecraft:select",
                        "property": "minecraft:component",
                        "component": "minecraft:custom_name",
                        "cases": [{"when": "pick", "model": {"type": "minecraft:model", "model": "minecraft:item/diamond_pickaxe"}}],
                        "fallback": {"type": "minecraft:model", "model": "minecraft:item/stone_pickaxe"}
                    },
                    "on_false": {"type": "minecraft:empty"}
                }
            }"#,
        )
        .unwrap();
        let definitions = BTreeMap::from([("minecraft:diamond_pickaxe".to_owned(), def)]);
        let raw_items = br##"{
            "minecraft:diamond_pickaxe": {
                "minecraft:item_model": "minecraft:diamond_pickaxe",
                "minecraft:max_stack_size": 1,
                "minecraft:tool": {"rules": [{"blocks": "#minecraft:mineable/pickaxe", "speed": 8.0}]},
                "minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage", "amount": 5.0}],
                "minecraft:item_name": {"text": "Diamond Pickaxe"},
                "minecraft:custom_name": "pick"
            }
        }"##;

        let prototypes = load_prototypes_pruned(raw_items, &definitions).unwrap();
        let components = &prototypes["minecraft:diamond_pickaxe"];
        assert_eq!(
            components.get("minecraft:item_model"),
            Some(&Value::String("minecraft:diamond_pickaxe".to_owned()))
        );
        assert_eq!(
            components.get("minecraft:max_stack_size"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            components.get("minecraft:custom_name"),
            Some(&Value::String("pick".to_owned()))
        );
        // Heavy components keep key presence for `has_component` without retaining large trees.
        assert_eq!(components.get("minecraft:tool"), Some(&Value::Null));
        assert_eq!(
            components.get("minecraft:attribute_modifiers"),
            Some(&Value::Null)
        );
        assert_eq!(components.get("minecraft:item_name"), Some(&Value::Null));

        let models = ItemModels {
            definitions,
            prototypes,
        };
        let view = StackView::new("minecraft:diamond_pickaxe", 1);
        let ctx = Context {
            display: DisplayContext::Gui,
            owner: None,
            level: None,
            seed: 0,
        };
        struct DummyEnv;
        impl ItemEnv for DummyEnv {}
        let resolved = models.resolve(&view, &ctx, &DummyEnv);
        assert_eq!(resolved.layers.len(), 1);
        assert_eq!(
            resolved.layers[0].content,
            LayerContent::Model {
                model: "minecraft:item/diamond_pickaxe".to_owned()
            }
        );
    }
}
