//! The 26.3 cuboid model format, before parent resolution and quad baking.
//!
//! Absent geometry differs from an explicit empty element list. Explicit null
//! elements, faces and display transforms are retained as the native decoder
//! does; render preparation must diagnose these before baking.
use crate::json::*;
use crate::obj::{ObjModel, ObjSource};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}
impl Direction {
    pub const ALL: [Self; 6] = [
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
        Self::West,
        Self::East,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|direction| direction.name() == name)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiLight {
    Front,
    Side,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    pub sprite: String,
    pub force_translucent: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureSlot {
    Reference(String),
    Material(Material),
}
#[derive(Clone, Debug, PartialEq)]
pub struct CuboidModel {
    pub parent: Option<String>,
    pub ambient_occlusion: Option<bool>,
    pub gui_light: Option<GuiLight>,
    pub textures: BTreeMap<String, TextureSlot>,
    pub elements: Option<Vec<Option<Element>>>,
    pub display: Option<ItemTransforms>,
    /// `neoforge:composite` children: their geometry is drawn together.
    pub children: Vec<CuboidModel>,
    /// A custom loader's model with no geometry of its own: the best texture
    /// found in its JSON, drawn as a cube (blocks) or a flat item.
    pub fallback_texture: Option<String>,
    /// `neoforge:obj`: the OBJ file the model draws, once read.
    pub obj: Option<crate::obj::ObjModel>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub faces: BTreeMap<Direction, Option<Face>>,
    pub rotation: Option<ElementRotation>,
    pub shade_direction_override: Option<Direction>,
    pub light_emission: u8,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub cull: Option<Direction>,
    pub tint_index: i32,
    pub texture: String,
    pub uv: Option<[f32; 4]>,
    /// Normalized counterclockwise quadrants, in the range 0..4.
    pub rotation: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}
#[derive(Clone, Debug, PartialEq)]
pub enum RotationValue {
    SingleAxis { axis: Axis, angle: f32 },
    EulerXyz([f32; 3]),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ElementRotation {
    /// Block units, converted from model coordinates during decoding.
    pub origin: [f32; 3],
    pub value: RotationValue,
    pub rescale: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ItemTransform {
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}
/// Missing transforms inherit later. Explicit identity and null values do not.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayTransform {
    Inherit,
    Explicit(Option<ItemTransform>),
}
pub const DISPLAY_CONTEXTS: [&str; 9] = [
    "thirdperson_lefthand",
    "thirdperson_righthand",
    "firstperson_lefthand",
    "firstperson_righthand",
    "head",
    "gui",
    "ground",
    "fixed",
    "on_shelf",
];
#[derive(Clone, Debug, PartialEq)]
pub struct ItemTransforms {
    pub contexts: [DisplayTransform; 9],
}

impl CuboidModel {
    /// Decode strict JSON, preserving the native parser's primitive coercions,
    /// optional inheritance fields, nullable children and f32 conversion.
    /// Like the native stream entry point, this reads one value and leaves
    /// trailing input unread. The pinned native entry point uses strict JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        let raw = serde_json::Deserializer::from_str(json.strip_prefix('\u{feff}').unwrap_or(json))
            .into_iter::<&RawValue>()
            .next()
            .ok_or_else(|| error("empty model input"))??;
        validate_number_tokens(raw)?;
        Self::decode(raw)
    }
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let Some(loader) = obj.get("loader").and_then(|raw| string(raw).ok()) else {
            return Self::decode_plain(&obj);
        };
        let known = loader
            .strip_prefix("neoforge:")
            .or_else(|| loader.strip_prefix("forge:"));
        Self::decode_with_loader(&obj, known)
    }
    /// Library-level model loaders: every mod using one works the same way.
    /// A loader this cannot draw degrades to its parent, its referenced model
    /// or its best texture; it never fails the model.
    fn decode_with_loader(obj: &Object<'_>, known: Option<&str>) -> Result<Self> {
        let mut model = Self::decode_plain(obj)?;
        match known {
            Some("separate_transforms") => {
                if let Some(base) = obj.get("base").and_then(|raw| Self::decode_nested(raw)) {
                    model = model.with_base(base);
                }
            }
            Some("composite") => {
                if let Some(children) = obj.get("children").and_then(|raw| object(raw).ok()) {
                    model.children = children
                        .values()
                        .filter_map(|raw| Self::decode_nested(raw))
                        .collect();
                }
            }
            Some("item_layers") => {
                if model.parent.is_none() && model.elements.is_none() {
                    model.parent = Some(crate::resolve::GENERATED_MODEL.to_owned());
                }
            }
            Some("fluid_container") => {
                let loader_parent = model
                    .parent
                    .as_deref()
                    .is_none_or(|p| p.starts_with("neoforge:") || p.starts_with("forge:"));
                if model.elements.is_none() && loader_parent {
                    let base = model.textures.get("base").cloned().unwrap_or_else(|| {
                        TextureSlot::Material(Material {
                            sprite: "minecraft:item/bucket".to_owned(),
                            force_translucent: false,
                        })
                    });
                    model.parent = Some(crate::resolve::GENERATED_MODEL.to_owned());
                    model.textures.insert("layer0".to_owned(), base);
                }
            }
            // The geometry is read from the OBJ file by `attach_obj`; until
            // then, and if that fails, the model draws its best texture.
            Some("obj") => {
                model.obj = ObjSource::decode(obj).map(|source| ObjModel {
                    source,
                    quads: None,
                });
                model.fallback_texture = fallback_texture_of(obj);
            }
            _ => {
                if model.parent.is_some() || model.elements.is_some() {
                    return Ok(model);
                }
                let referenced = obj
                    .get("model")
                    .and_then(|raw| string(raw).ok())
                    .filter(|name| !name.ends_with(".obj"))
                    .and_then(|name| identifier(&name).ok());
                if let Some(referenced) = referenced {
                    model.parent = Some(referenced);
                    return Ok(model);
                }
                let nested = obj
                    .values()
                    .filter(|raw| raw.get().starts_with('{'))
                    .filter(|raw| {
                        object(raw)
                            .is_ok_and(|o| o.contains_key("elements") || o.contains_key("parent"))
                    })
                    .find_map(|raw| Self::decode_nested(raw));
                if let Some(nested) = nested {
                    return Ok(model.with_base(nested));
                }
                model.fallback_texture = fallback_texture_of(obj);
            }
        }
        Ok(model)
    }
    /// A child or base: an inline model, or a model reference.
    fn decode_nested(raw: &RawValue) -> Option<Self> {
        if raw.get().starts_with('"') {
            let parent = identifier(&string(raw).ok()?).ok()?;
            return Some(Self {
                parent: Some(parent),
                ambient_occlusion: None,
                gui_light: None,
                textures: BTreeMap::new(),
                elements: None,
                display: None,
                children: Vec::new(),
                fallback_texture: None,
                obj: None,
            });
        }
        Self::decode(raw).ok()
    }
    /// Read and parse the OBJ of a `neoforge:obj` model through `read`, which
    /// returns the text of `assets/<namespace>/<path>`. On error the model
    /// keeps drawing its fallback texture.
    pub fn attach_obj(
        &mut self,
        read: &dyn Fn(&str, &str) -> Option<String>,
    ) -> std::result::Result<(), String> {
        let Some(obj) = self.obj.as_mut() else {
            return Ok(());
        };
        let (namespace, path) = obj
            .source
            .model
            .split_once(':')
            .unwrap_or(("minecraft", obj.source.model.as_str()));
        let text = read(namespace, path).ok_or_else(|| format!("cannot read {namespace}:{path}"))?;
        let directory = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let mut materials = BTreeMap::new();
        for library in crate::obj::material_libraries(&text) {
            let file = if directory.is_empty() {
                library
            } else {
                format!("{directory}/{library}")
            };
            if let Some(mtl) = read(namespace, &file) {
                materials.extend(crate::obj::parse_materials(&mtl));
            }
        }
        let quads = crate::obj::parse_obj(&text, &materials, &obj.source)?;
        obj.quads = Some(quads);
        Ok(())
    }
    /// `self` is the outer model, `base` the one that actually describes the
    /// geometry: the base wins, the outer model fills what it leaves out.
    fn with_base(self, base: Self) -> Self {
        let has_geometry = base.parent.is_some() || base.elements.is_some();
        let mut textures = self.textures;
        textures.extend(base.textures);
        Self {
            parent: if has_geometry { base.parent } else { self.parent },
            ambient_occlusion: base.ambient_occlusion.or(self.ambient_occlusion),
            gui_light: base.gui_light.or(self.gui_light),
            textures,
            elements: base.elements.or(self.elements),
            display: base.display.or(self.display),
            children: if base.children.is_empty() {
                self.children
            } else {
                base.children
            },
            fallback_texture: base.fallback_texture.or(self.fallback_texture),
            obj: base.obj.or(self.obj),
        }
    }
    fn decode_plain(obj: &Object<'_>) -> Result<Self> {
        let parent = obj
            .get("parent")
            .map(|r| primitive_string(r))
            .transpose()?
            .unwrap_or_default();
        let parent = if parent.is_empty() {
            None
        } else {
            Some(identifier(&parent)?)
        };
        let ambient_occlusion = obj
            .get("ambientocclusion")
            .map(|r| boolean(r))
            .transpose()?;
        let gui_light = obj
            .get("gui_light")
            .map(|raw| match primitive_string(raw)?.as_str() {
                "front" => Ok(GuiLight::Front),
                "side" => Ok(GuiLight::Side),
                _ => Err(error("invalid gui light")),
            })
            .transpose()?;
        let mut textures = BTreeMap::new();
        if let Some(raw) = obj.get("textures") {
            for (key, raw) in object(raw)? {
                textures.insert(key, texture(raw)?);
            }
        }
        let elements = obj
            .get("elements")
            .map(|raw| {
                array(raw)?
                    .into_iter()
                    .map(|raw| nullable(raw, Element::decode))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let display = obj
            .get("display")
            .map(|r| ItemTransforms::decode(r))
            .transpose()?;
        Ok(Self {
            parent,
            ambient_occlusion,
            gui_light,
            textures,
            elements,
            display,
            children: Vec::new(),
            fallback_texture: None,
            obj: None,
        })
    }
}
/// The best texture of a model-like JSON object (`textures`, `ctm_textures`,
/// a top-level `particle`): particle, then all, center, empty, then any.
fn fallback_texture_of(obj: &Object<'_>) -> Option<String> {
    const PREFERRED: [&str; 4] = ["particle", "all", "center", "empty"];
    let name = |raw: &RawValue| -> Option<String> {
        let sprite = if raw.get().starts_with('{') {
            string(object(raw).ok()?.get("sprite")?).ok()?
        } else {
            string(raw).ok()?
        };
        if sprite.starts_with('#') {
            return None;
        }
        identifier(&sprite).ok()
    };
    for key in ["textures", "ctm_textures"] {
        let Some(map) = obj.get(key).and_then(|raw| object(raw).ok()) else {
            continue;
        };
        let picked = PREFERRED
            .iter()
            .filter_map(|k| map.get(*k))
            .chain(map.values())
            .find_map(|raw| name(raw));
        if picked.is_some() {
            return picked;
        }
    }
    ["particle", "texture"]
        .iter()
        .filter_map(|k| obj.get(*k))
        .find_map(|raw| name(raw))
}
/// [`fallback_texture_of`] for a whole JSON document (a blockstate with
/// `ctm_textures`, for example). `None` when it names no texture.
pub fn loader_fallback_texture(json: &str) -> Option<String> {
    let raw: &RawValue =
        serde_json::from_str(json.strip_prefix('\u{feff}').unwrap_or(json)).ok()?;
    fallback_texture_of(&object(raw).ok()?)
}
fn nullable<T>(raw: &RawValue, decode: impl FnOnce(&RawValue) -> Result<T>) -> Result<Option<T>> {
    if raw.get() == "null" {
        Ok(None)
    } else {
        decode(raw).map(Some)
    }
}
fn texture(raw: &RawValue) -> Result<TextureSlot> {
    if raw.get().starts_with('"') {
        let name = string(raw)?;
        if name.is_empty() {
            return Err(error("empty texture slot value"));
        }
        if let Some(target) = name.strip_prefix('#') {
            return Ok(TextureSlot::Reference(target.to_owned()));
        }
        return Ok(TextureSlot::Material(Material {
            sprite: identifier(&name)?,
            force_translucent: false,
        }));
    }
    let obj = object(raw)?;
    let sprite = identifier(&string(required(&obj, "sprite")?)?)?;
    let force_translucent = match obj.get("force_translucent") {
        None => false,
        Some(raw) if raw.get() == "null" => false,
        Some(raw) if raw.get() == "true" => true,
        Some(raw) if raw.get() == "false" => false,
        Some(_) => return Err(error("invalid material boolean")),
    };
    Ok(TextureSlot::Material(Material {
        sprite,
        force_translucent,
    }))
}
impl Element {
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let from = vector(required(&obj, "from")?)?;
        let to = vector(required(&obj, "to")?)?;
        if from.iter().chain(&to).any(|v| !(-16.0..=32.0).contains(v)) {
            return Err(error("element outside model bounds"));
        }
        let rotation = obj
            .get("rotation")
            .map(|r| ElementRotation::decode(r))
            .transpose()?;
        let mut faces = BTreeMap::new();
        for (name, raw) in object(required(&obj, "faces")?)? {
            let direction =
                Direction::by_name(&name).ok_or_else(|| error("invalid face direction"))?;
            faces.insert(direction, nullable(raw, Face::decode)?);
        }
        if faces.is_empty() {
            return Err(error("element requires a face"));
        }
        let shade_direction_override = obj
            .get("shade_direction_override")
            .map(|raw| {
                Direction::by_name(&primitive_string(raw)?)
                    .ok_or_else(|| error("invalid shade direction"))
            })
            .transpose()?;
        let light_emission = obj
            .get("light_emission")
            .map(|r| integer(r))
            .transpose()?
            .unwrap_or(0);
        if !(0..=15).contains(&light_emission) {
            return Err(error("invalid light emission"));
        }
        Ok(Self {
            from,
            to,
            faces,
            rotation,
            shade_direction_override,
            light_emission: light_emission as u8,
        })
    }
}
impl Face {
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let cull = obj
            .get("cullface")
            .map(|r| primitive_string(r))
            .transpose()?
            .and_then(|s| Direction::by_name(&s));
        let texture = primitive_string(required(&obj, "texture")?)?;
        let tint_index = obj
            .get("tintindex")
            .map(|r| integer(r))
            .transpose()?
            .unwrap_or(-1);
        let rotation = obj
            .get("rotation")
            .map(|r| integer(r))
            .transpose()?
            .unwrap_or(0)
            .rem_euclid(360);
        if rotation % 90 != 0 {
            return Err(error("face rotation must be a multiple of 90"));
        }
        let uv = obj.get("uv").map(|r| vector(r)).transpose()?;
        Ok(Self {
            cull,
            tint_index,
            texture,
            uv,
            rotation: (rotation / 90) as u8,
        })
    }
}
impl ElementRotation {
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let origin = vector::<3>(required(&obj, "origin")?)?.map(|v| v * 0.0625);
        let value = if obj.contains_key("axis") || obj.contains_key("angle") {
            let axis = match primitive_string(required(&obj, "axis")?)?
                .to_lowercase()
                .as_str()
            {
                "x" => Axis::X,
                "y" => Axis::Y,
                "z" => Axis::Z,
                _ => return Err(error("invalid rotation axis")),
            };
            RotationValue::SingleAxis {
                axis,
                angle: float(required(&obj, "angle")?)?,
            }
        } else if ["x", "y", "z"].iter().any(|key| obj.contains_key(*key)) {
            let mut angles = [0.0; 3];
            for (angle, key) in angles.iter_mut().zip(["x", "y", "z"]) {
                *angle = obj.get(key).map(|r| float(r)).transpose()?.unwrap_or(0.0);
            }
            RotationValue::EulerXyz(angles)
        } else {
            return Err(error("missing rotation value"));
        };
        let rescale = obj
            .get("rescale")
            .map(|r| boolean(r))
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            origin,
            value,
            rescale,
        })
    }
}
impl ItemTransform {
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let rotation = obj
            .get("rotation")
            .map(|r| vector(r))
            .transpose()?
            .unwrap_or([0.0; 3]);
        let translation = obj
            .get("translation")
            .map(|r| vector(r))
            .transpose()?
            .unwrap_or([0.0; 3])
            .map(|v| (v * 0.0625).clamp(-5.0, 5.0));
        let scale = obj
            .get("scale")
            .map(|r| vector(r))
            .transpose()?
            .unwrap_or([1.0; 3])
            .map(|v| v.clamp(-4.0, 4.0));
        Ok(Self {
            rotation,
            translation,
            scale,
        })
    }
}
impl ItemTransforms {
    fn decode(raw: &RawValue) -> Result<Self> {
        let obj = object(raw)?;
        let mut contexts = std::array::from_fn(|_| DisplayTransform::Inherit);
        for (transform, key) in contexts.iter_mut().zip(DISPLAY_CONTEXTS) {
            if let Some(raw) = obj.get(key) {
                *transform = DisplayTransform::Explicit(nullable(raw, ItemTransform::decode)?);
            }
        }
        for (left, right) in [(0, 1), (2, 3)] {
            if contexts[left] == DisplayTransform::Inherit {
                contexts[left] = contexts[right].clone();
            }
        }
        Ok(Self { contexts })
    }
}
