//! Cuboid parent inheritance and texture-slot resolution.
use crate::{json::*, model::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const MISSING_MODEL: &str = "minecraft:builtin/missing";
/// `ItemModelGenerator.GENERATED_ITEM_MODEL_ID`.
pub const GENERATED_MODEL: &str = "minecraft:builtin/generated";
/// Parent of items drawn by an entity renderer; a mod's item model that has
/// it is drawn flat from its textures, as no renderer exists here for it.
const ENTITY_MODEL: &str = "minecraft:builtin/entity";
const MAX_COMPOSITE_DEPTH: u8 = 8;

/// Decoded models are immutable and their inherited geometry is shared.
pub struct ModelLibrary {
    models: BTreeMap<String, ModelResource>,
    missing: Arc<CuboidModel>,
}
/// An immutable, already validated snapshot. Implementations may keep compact
/// backing and expand one model for a resolution, without consulting pack files.
pub trait ModelSource: Send + Sync {
    fn decode(&self) -> Arc<CuboidModel>;
}
enum ModelResource {
    Decoded(Arc<CuboidModel>),
    Deferred(Arc<dyn ModelSource>),
}
impl ModelResource {
    fn decode(&self) -> Arc<CuboidModel> {
        match self {
            Self::Decoded(model) => Arc::clone(model),
            Self::Deferred(source) => source.decode(),
        }
    }
}
/// An OBJ face with its texture slot already resolved.
#[derive(Clone, Debug)]
pub struct ObjDraw {
    pub quad: crate::obj::ObjQuad,
    pub material: Material,
    pub automatic_culling: bool,
}
#[derive(Clone, Debug)]
pub struct ResolvedCuboidModel {
    pub ambient_occlusion: bool,
    pub gui_light: GuiLight,
    pub display: ItemTransforms,
    pub textures: BTreeMap<String, Material>,
    /// Cyclic and dangling aliases are absent from `textures`.
    pub unresolved_textures: BTreeSet<String>,
    geometry: Option<Arc<CuboidModel>>,
    generated: bool,
    obj: Vec<ObjDraw>,
}
impl ResolvedCuboidModel {
    /// The faces of a `neoforge:obj` model (and of composite children that are).
    pub fn obj_quads(&self) -> &[ObjDraw] {
        &self.obj
    }
    /// Whether the top geometry is `builtin/generated`'s layer extrusion
    /// (no model closer to the leaf has elements of its own).
    pub fn uses_item_generator(&self) -> bool {
        self.generated
    }
    pub fn elements(&self) -> &[Option<Element>] {
        self.geometry
            .as_ref()
            .and_then(|m| m.elements.as_deref())
            .unwrap_or(&[])
    }
    /// Native slot lookup strips a single leading '#'. Non-reference names
    /// are still slot names, not direct sprite identifiers.
    pub fn material(&self, slot: &str) -> Result<Option<&Material>> {
        if slot.is_empty() {
            return Err(error("empty texture lookup"));
        }
        Ok(self.textures.get(slot.strip_prefix('#').unwrap_or(slot)))
    }
}
impl ModelLibrary {
    /// A library that also knows `builtin/generated`, the parent of every
    /// flat item model; the native client registers it as a special model.
    pub fn with_item_generator(mut models: BTreeMap<String, CuboidModel>) -> Result<Self> {
        models.insert(GENERATED_MODEL.to_owned(), Self::item_generator_model());
        Self::new(models)
    }
    /// Register the item built-in while reusing already decoded resources.
    pub fn with_shared_item_generator(
        mut models: BTreeMap<String, Arc<CuboidModel>>,
    ) -> Result<Self> {
        models.insert(
            GENERATED_MODEL.to_owned(),
            Arc::new(Self::item_generator_model()),
        );
        Self::from_shared(models)
    }
    /// Retain all runtime identifiers while releasing expanded source trees.
    pub fn with_deferred_item_generator(
        models: BTreeMap<String, Arc<dyn ModelSource>>,
    ) -> Result<Self> {
        let mut models: BTreeMap<_, _> = models
            .into_iter()
            .map(|(name, source)| (name, ModelResource::Deferred(source)))
            .collect();
        models.insert(
            GENERATED_MODEL.to_owned(),
            ModelResource::Decoded(Arc::new(Self::item_generator_model())),
        );
        Self::from_resources(models)
    }
    fn item_generator_model() -> CuboidModel {
        CuboidModel {
            parent: None,
            ambient_occlusion: None,
            gui_light: Some(GuiLight::Front),
            textures: BTreeMap::from([(
                "particle".to_owned(),
                TextureSlot::Reference("layer0".to_owned()),
            )]),
            elements: None,
            display: None,
            children: Vec::new(),
            fallback_texture: None,
            obj: None,
        }
    }
    pub fn new(models: BTreeMap<String, CuboidModel>) -> Result<Self> {
        Self::from_models(
            models
                .into_iter()
                .map(|(name, model)| (name, Arc::new(model))),
        )
    }
    /// Build an immutable resolution view without copying model geometry.
    pub fn from_shared(models: BTreeMap<String, Arc<CuboidModel>>) -> Result<Self> {
        Self::from_models(models)
    }
    fn from_models(models: impl IntoIterator<Item = (String, Arc<CuboidModel>)>) -> Result<Self> {
        Self::from_resources(
            models
                .into_iter()
                .map(|(name, model)| (name, ModelResource::Decoded(model))),
        )
    }
    fn from_resources(models: impl IntoIterator<Item = (String, ModelResource)>) -> Result<Self> {
        let mut decoded = BTreeMap::new();
        for (name, model) in models {
            if decoded.insert(identifier(&name)?, model).is_some() {
                return Err(error("duplicate normalized model identifier"));
            }
        }
        let missing = Arc::new(missing_model());
        // The native discovery registers this built-in before consulting resources.
        decoded.insert(
            MISSING_MODEL.to_owned(),
            ModelResource::Decoded(missing.clone()),
        );
        Ok(Self {
            models: decoded,
            missing,
        })
    }
    /// Follow the complete parent chain. A missing resource uses the built-in
    /// model; a cyclic chain is rejected, as native discovery excludes it.
    /// The returned model can outlive this library without copying its elements.
    pub fn resolve(&self, name: &str) -> Result<ResolvedCuboidModel> {
        let name = identifier(name)?;
        let model = self.model(&name);
        self.resolve_from(model, name, 0)
    }
    fn model(&self, name: &str) -> Arc<CuboidModel> {
        self.models
            .get(name)
            .map(ModelResource::decode)
            .unwrap_or_else(|| Arc::clone(&self.missing))
    }
    /// Resolve `first`, whose identifier is `first_name` (a composite child
    /// has a made-up one), through its parent chain.
    fn resolve_from(
        &self,
        first: Arc<CuboidModel>,
        first_name: String,
        depth: u8,
    ) -> Result<ResolvedCuboidModel> {
        let leaf = first_name
            .split('#')
            .next()
            .unwrap_or(&first_name)
            .to_owned();
        let (leaf_namespace, leaf_path) =
            leaf.split_once(':').unwrap_or(("minecraft", leaf.as_str()));
        let is_mod_item = leaf_namespace != "minecraft" && leaf_path.starts_with("item/");
        let mut name = first_name.clone();
        let mut current = first;
        let mut visited = BTreeSet::new();
        let mut chain: Vec<Arc<CuboidModel>> = Vec::new();
        let mut names = Vec::new();
        let mut entity = false;
        loop {
            if !visited.insert(name.clone()) {
                return Err(error("cyclic model parent"));
            }
            if is_mod_item && name == ENTITY_MODEL {
                entity = true;
                chain.push(Arc::new(Self::item_generator_model()));
                names.push(GENERATED_MODEL.to_owned());
                break;
            }
            chain.push(Arc::clone(&current));
            names.push(name.clone());
            let Some(parent) = current.parent.clone() else {
                break;
            };
            current = self.model(&parent);
            name = parent;
        }
        let ambient_occlusion = chain
            .iter()
            .find_map(|m| m.ambient_occlusion)
            .unwrap_or(true);
        let gui_light = chain
            .iter()
            .find_map(|m| m.gui_light)
            .unwrap_or(GuiLight::Side);
        let mut geometry = chain.iter().find(|m| m.elements.is_some()).map(Arc::clone);
        let mut generated = chain
            .iter()
            .zip(&names)
            .find(|(m, name)| m.elements.is_some() || *name == GENERATED_MODEL)
            .is_some_and(|(_, name)| name == GENERATED_MODEL);
        let display = ItemTransforms {
            contexts: std::array::from_fn(|index| {
                chain
                    .iter()
                    .filter_map(|m| m.display.as_ref())
                    .map(|d| &d.contexts[index])
                    .find(|t| **t != DisplayTransform::Inherit)
                    .cloned()
                    .unwrap_or(DisplayTransform::Inherit)
            }),
        };
        // Slots a loader fallback adds below every explicit definition.
        let mut extra_slots: Vec<(&'static str, TextureSlot)> = Vec::new();
        if entity && !chain.iter().any(|m| m.textures.contains_key("layer0")) {
            let own_particle = chain
                .iter()
                .find_map(|m| m.textures.get("particle"))
                .filter(|slot| matches!(slot, TextureSlot::Material(_)))
                .cloned();
            let guess = || {
                TextureSlot::Material(Material {
                    sprite: format!("{leaf_namespace}:{leaf_path}"),
                    force_translucent: false,
                })
            };
            extra_slots.push(("layer0", own_particle.unwrap_or_else(guess)));
        }
        let obj_quads = chain.iter().filter_map(|m| m.obj.as_ref()).find_map(|o| {
            o.quads
                .as_ref()
                .map(|quads| (quads, o.source.automatic_culling))
        });
        if !generated && geometry.is_none() && obj_quads.is_none() {
            let fallback = chain.iter().find_map(|m| m.fallback_texture.clone());
            if let Some(sprite) = fallback {
                let slot = TextureSlot::Material(Material {
                    sprite,
                    force_translucent: false,
                });
                if leaf_path.starts_with("item/") {
                    generated = true;
                    extra_slots.push(("layer0", slot.clone()));
                } else {
                    geometry = Some(Arc::new(cube_all_geometry()));
                    extra_slots.push(("all", slot.clone()));
                }
                extra_slots.push(("particle", slot));
            }
        }
        // Resolve aliases only after applying every child override. A reference
        // inherited from a parent binds to the child's final slot definition.
        let mut slots = BTreeMap::new();
        for (key, slot) in &extra_slots {
            slots.insert((*key).to_owned(), slot);
        }
        for model in chain.iter().rev() {
            slots.extend(model.textures.iter().map(|(k, v)| (k.clone(), v)));
        }
        let mut textures = BTreeMap::<String, Material>::new();
        let mut unresolved_textures = BTreeSet::<String>::new();
        for start in slots.keys() {
            let mut current = start.as_str();
            let mut seen = BTreeSet::new();
            let mut path = Vec::new();
            let material = loop {
                if let Some(material) = textures.get(current) {
                    break Some(material.clone());
                }
                if unresolved_textures.contains(current) || !seen.insert(current) {
                    break None;
                }
                let Some(slot) = slots.get(current) else {
                    break None;
                };
                path.push(current);
                match slot {
                    TextureSlot::Reference(target) => current = target,
                    TextureSlot::Material(material) => break Some(material.clone()),
                }
            };
            // Cache the entire walk, including failures, so long alias chains
            // and cycles are traversed once rather than once per referencing slot.
            match material {
                Some(material) => {
                    for name in path {
                        textures.insert(name.to_owned(), material.clone());
                    }
                }
                None => {
                    unresolved_textures.extend(path.into_iter().map(str::to_owned));
                }
            }
        }
        let mut obj: Vec<ObjDraw> = obj_quads
            .into_iter()
            .flat_map(|(quads, automatic_culling)| {
                let textures = &textures;
                quads.iter().map(move |quad| ObjDraw {
                    material: obj_material(textures, &quad.texture),
                    quad: quad.clone(),
                    automatic_culling,
                })
            })
            .collect();
        // Like NeoForge, inherit the nearest supported custom geometry. A
        // composite supplies its own geometry in place of ordinary elements.
        if let Some(composite) = chain
            .iter()
            .find(|m| !m.children.is_empty() || m.obj.is_some())
            .filter(|m| !m.children.is_empty())
            && depth < MAX_COMPOSITE_DEPTH
        {
            geometry = None;
            generated = false;
            obj.clear();
            let mut elements = Vec::new();
            let mut layers = Vec::new();
            for (index, child) in composite.children.iter().enumerate() {
                let child_name = format!("{first_name}#c{index}");
                let Ok(child) = self.resolve_from(Arc::new(child.clone()), child_name, depth + 1)
                else {
                    continue;
                };
                if !textures.contains_key("particle")
                    && let Some(particle) = child.textures.get("particle")
                {
                    textures.insert("particle".to_owned(), particle.clone());
                }
                if child.generated {
                    for slot in ["layer0", "layer1", "layer2", "layer3", "layer4"] {
                        match child.textures.get(slot) {
                            Some(material) => layers.push(material.clone()),
                            None => break,
                        }
                    }
                    continue;
                }
                obj.extend(child.obj.iter().cloned());
                for element in child.elements() {
                    elements.push(element.as_ref().map(|e| prefix_slots(e, index)));
                }
                for (slot, material) in &child.textures {
                    textures.insert(format!("c{index}:{slot}"), material.clone());
                }
            }
            if !elements.is_empty() {
                geometry = Some(Arc::new(CuboidModel {
                    elements: Some(elements),
                    ..empty_model()
                }));
            } else if !layers.is_empty() {
                for (index, material) in layers.into_iter().enumerate() {
                    textures.insert(format!("layer{index}"), material);
                }
                generated = true;
            }
        }
        Ok(ResolvedCuboidModel {
            ambient_occlusion,
            gui_light,
            display,
            textures,
            unresolved_textures,
            geometry,
            generated,
            obj,
        })
    }
}

/// An OBJ face's texture token (`#slot`, a slot name or a sprite id) as a material.
fn obj_material(textures: &BTreeMap<String, Material>, token: &str) -> Material {
    let slot = token.strip_prefix('#').unwrap_or(token);
    if let Some(material) = textures.get(slot).filter(|_| !slot.is_empty()) {
        return material.clone();
    }
    let direct = if token.is_empty() || token.starts_with('#') {
        None
    } else {
        identifier(token).ok()
    };
    Material {
        sprite: direct.unwrap_or_else(|| "minecraft:missingno".to_owned()),
        force_translucent: false,
    }
}
fn empty_model() -> CuboidModel {
    CuboidModel {
        parent: None,
        ambient_occlusion: None,
        gui_light: None,
        textures: BTreeMap::new(),
        elements: None,
        display: None,
        children: Vec::new(),
        fallback_texture: None,
        obj: None,
    }
}
/// A composite child's slots live next to its siblings' under `c<index>:`.
fn prefix_slots(element: &Element, index: usize) -> Element {
    let mut element = element.clone();
    for face in element.faces.values_mut().flatten() {
        let slot = face.texture.strip_prefix('#').unwrap_or(&face.texture);
        face.texture = format!("#c{index}:{slot}");
    }
    element
}
/// A cube drawn with one sprite, as `block/cube_all` would.
pub fn cube_all_model(sprite: &str) -> CuboidModel {
    let slot = TextureSlot::Material(Material {
        sprite: sprite.to_owned(),
        force_translucent: false,
    });
    CuboidModel {
        textures: BTreeMap::from([
            ("all".to_owned(), slot.clone()),
            ("particle".to_owned(), slot),
        ]),
        ..cube_all_geometry()
    }
}
/// A full cube with every face textured by the `all` slot.
fn cube_all_geometry() -> CuboidModel {
    let faces = Direction::ALL
        .into_iter()
        .map(|direction| {
            (
                direction,
                Some(Face {
                    cull: Some(direction),
                    tint_index: -1,
                    texture: "#all".to_owned(),
                    uv: Some([0.0, 0.0, 16.0, 16.0]),
                    rotation: 0,
                }),
            )
        })
        .collect();
    CuboidModel {
        elements: Some(vec![Some(Element {
            from: [0.0; 3],
            to: [16.0; 3],
            faces,
            rotation: None,
            shade_direction_override: None,
            light_emission: 0,
        })]),
        ..empty_model()
    }
}
/// The native missing-model cube uses slot names without a leading '#'.
pub fn missing_model() -> CuboidModel {
    let faces = Direction::ALL
        .into_iter()
        .map(|direction| {
            (
                direction,
                Some(Face {
                    cull: Some(direction),
                    tint_index: -1,
                    texture: "missingno".to_owned(),
                    uv: Some([0.0, 0.0, 16.0, 16.0]),
                    rotation: 0,
                }),
            )
        })
        .collect();
    CuboidModel {
        parent: None,
        ambient_occlusion: None,
        gui_light: None,
        elements: Some(vec![Some(Element {
            from: [0.0; 3],
            to: [16.0; 3],
            faces,
            rotation: None,
            shade_direction_override: None,
            light_emission: 0,
        })]),
        display: Some(ItemTransforms {
            contexts: std::array::from_fn(|_| DisplayTransform::Inherit),
        }),
        children: Vec::new(),
        fallback_texture: None,
        obj: None,
        textures: BTreeMap::from([
            (
                "particle".to_owned(),
                TextureSlot::Reference("missingno".to_owned()),
            ),
            (
                "missingno".to_owned(),
                TextureSlot::Material(Material {
                    sprite: "minecraft:missingno".to_owned(),
                    force_translucent: false,
                }),
            ),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_alias_uses_child_material_and_geometry_outlives_library() {
        let parent = CuboidModel::from_json(r##"{"textures":{"side":"#all","all":"block/stone"},"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"up":{"texture":"#side"}}}]}"##).unwrap();
        let child = CuboidModel::from_json(r#"{"parent":"horizon:parent","textures":{"all":{"sprite":"block/glass","force_translucent":true}}}"#).unwrap();
        let library = ModelLibrary::new(BTreeMap::from([
            ("horizon:parent".into(), parent),
            ("horizon:child".into(), child),
        ]))
        .unwrap();
        let model = library.resolve("horizon:child").unwrap();
        drop(library);
        assert_eq!(model.elements()[0].as_ref().unwrap().to, [16.0, 8.0, 16.0]);
        assert_eq!(
            model.material("#side").unwrap().unwrap(),
            &Material {
                sprite: "minecraft:block/glass".into(),
                force_translucent: true
            }
        );
    }
    #[test]
    fn absent_parents_fall_back_but_cycles_invalidate_descendants() {
        let a = CuboidModel::from_json(r#"{"parent":"horizon:b"}"#).unwrap();
        let b = CuboidModel::from_json(r#"{"parent":"horizon:a"}"#).unwrap();
        let child = CuboidModel::from_json(r#"{"parent":"horizon:a","elements":[]}"#).unwrap();
        let library = ModelLibrary::new(BTreeMap::from([
            ("horizon:a".into(), a),
            ("horizon:b".into(), b),
            ("horizon:child".into(), child),
        ]))
        .unwrap();
        assert!(library.resolve("horizon:child").is_err());
        let absent = library.resolve("horizon:absent").unwrap();
        assert_eq!(absent.elements().len(), 1);
        assert_eq!(
            absent.material("particle").unwrap().unwrap().sprite,
            "minecraft:missingno"
        );
    }

    fn library(models: &[(&str, &str)]) -> ModelLibrary {
        let mut map = BTreeMap::new();
        for (name, json) in models {
            map.insert((*name).to_owned(), CuboidModel::from_json(json).unwrap());
        }
        for (name, json) in [
            ("minecraft:block/block", "{}"),
            (
                "minecraft:item/generated",
                r#"{"parent":"minecraft:builtin/generated"}"#,
            ),
        ] {
            map.insert(name.to_owned(), CuboidModel::from_json(json).unwrap());
        }
        ModelLibrary::with_item_generator(map).unwrap()
    }
    const CUBE: &str = r##"{"from":[0,0,0],"to":[16,16,16],"faces":{"up":{"texture":"#t"}}}"##;

    #[test]
    fn separate_transforms_uses_inline_base_over_outer_parent() {
        let lib = library(&[
            (
                "m:item/tpl",
                &format!(r#"{{"textures":{{"t":"m:item/x"}},"elements":[{CUBE}]}}"#),
            ),
            (
                "m:item/sword",
                r#"{"parent":"minecraft:item/handheld","loader":"neoforge:separate_transforms",
                    "base":{"parent":"m:item/tpl","textures":{"t":"m:item/blade"}},
                    "perspectives":{"gui":{"parent":"item/generated"}}}"#,
            ),
        ]);
        let model = lib.resolve("m:item/sword").unwrap();
        assert_eq!(model.elements().len(), 1);
        assert_eq!(model.material("t").unwrap().unwrap().sprite, "m:item/blade");
    }
    #[test]
    fn separate_transforms_string_base_is_a_parent_reference() {
        let lib = library(&[
            (
                "m:item/tpl",
                &format!(r#"{{"textures":{{"t":"m:item/x"}},"elements":[{CUBE}]}}"#),
            ),
            (
                "m:item/umbrella",
                r#"{"loader":"neoforge:separate_transforms","base":"m:item/tpl"}"#,
            ),
        ]);
        assert_eq!(lib.resolve("m:item/umbrella").unwrap().elements().len(), 1);
    }
    #[test]
    fn composite_draws_the_union_of_inline_and_referenced_children() {
        let lib = library(&[
            (
                "m:block/part",
                &format!(r#"{{"textures":{{"t":"m:block/a"}},"elements":[{CUBE}]}}"#),
            ),
            (
                "m:block/both",
                &format!(
                    r#"{{"loader":"neoforge:composite","parent":"block/block","textures":{{"particle":"m:block/p"}},
                    "children":{{"a":{{"parent":"m:block/part","textures":{{"t":"m:block/b"}}}},
                                 "b":{{"textures":{{"t":"m:block/c"}},"elements":[{CUBE}]}},
                                 "c":"m:block/part"}}}}"#
                ),
            ),
        ]);
        let model = lib.resolve("m:block/both").unwrap();
        assert_eq!(model.elements().len(), 3);
        let sprites: Vec<_> = model
            .elements()
            .iter()
            .map(|e| {
                e.as_ref().unwrap().faces[&Direction::Up]
                    .as_ref()
                    .unwrap()
                    .texture
                    .clone()
            })
            .map(|slot| model.material(&slot).unwrap().unwrap().sprite.clone())
            .collect();
        assert_eq!(sprites, ["m:block/b", "m:block/c", "m:block/a"]);
        assert_eq!(
            model.material("particle").unwrap().unwrap().sprite,
            "m:block/p"
        );
    }

    #[test]
    fn composite_geometry_survives_model_aliases() {
        let lib = library(&[
            (
                "m:block/part",
                &format!(r#"{{"textures":{{"t":"m:block/contact"}},"elements":[{CUBE}]}}"#),
            ),
            (
                "m:block/composite",
                &format!(
                    r#"{{"loader":"neoforge:composite","parent":"block/block",
                    "children":{{"a":"m:block/part","b":{{"textures":{{"t":"m:block/glass"}},
                    "elements":[{CUBE}]}}}}}}"#
                ),
            ),
            ("m:block/alias", r#"{"parent":"m:block/composite"}"#),
            (
                "m:block/display",
                r#"{"parent":"m:block/alias","textures":{"particle":"m:block/emitter"}}"#,
            ),
            (
                "m:block/nested",
                r#"{"loader":"neoforge:composite","children":{"display":"m:block/display"}}"#,
            ),
        ]);
        for id in ["m:block/alias", "m:block/display", "m:block/nested"] {
            let model = lib.resolve(id).unwrap();
            let quads = crate::bake::bake_block_model(
                &model,
                crate::bake::BlockModelTransform::new([0; 3], false),
            )
            .unwrap();
            let sprites: Vec<_> = quads.iter().map(|q| q.material.sprite.as_str()).collect();
            assert_eq!(sprites, ["m:block/contact", "m:block/glass"], "{id}");
        }
        let display = lib.resolve("m:block/display").unwrap();
        assert_eq!(
            display.material("particle").unwrap().unwrap().sprite,
            "m:block/emitter"
        );
    }

    #[test]
    fn nearest_composite_loader_replaces_older_geometry() {
        let with_elements =
            |sprite: &str| format!(r#"{{"textures":{{"t":"{sprite}"}},"elements":[{CUBE}]}}"#);
        let lib = library(&[
            ("m:block/old_part", &with_elements("m:block/old")),
            ("m:block/new_part", &with_elements("m:block/new")),
            (
                "m:block/base",
                r#"{"loader":"neoforge:composite","children":{"base":"m:block/old_part"}}"#,
            ),
            (
                "m:block/replacement",
                r#"{"parent":"m:block/base","loader":"neoforge:composite",
                    "children":{"replacement":"m:block/new_part"}}"#,
            ),
            (
                "m:block/alias",
                &format!(r#"{{"parent":"m:block/replacement","elements":[{CUBE}]}}"#),
            ),
        ]);
        let model = lib.resolve("m:block/alias").unwrap();
        let quads = crate::bake::bake_block_model(
            &model,
            crate::bake::BlockModelTransform::new([0; 3], false),
        )
        .unwrap();
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].material.sprite, "m:block/new");
    }

    #[test]
    fn composite_of_flat_children_stacks_their_layers() {
        let lib = library(&[(
            "m:item/stack",
            r#"{"loader":"neoforge:composite","children":{
                "a":{"parent":"item/generated","textures":{"layer0":"m:item/a"}},
                "b":{"parent":"item/generated","textures":{"layer0":"m:item/b"}}}}"#,
        )]);
        let model = lib.resolve("m:item/stack").unwrap();
        assert!(model.uses_item_generator());
        assert_eq!(
            model.material("layer1").unwrap().unwrap().sprite,
            "m:item/b"
        );
    }
    #[test]
    fn item_layers_without_parent_is_a_flat_item() {
        let lib = library(&[(
            "m:item/sigil",
            r#"{"loader":"neoforge:item_layers","textures":{"layer0":"m:item/sigil"}}"#,
        )]);
        assert!(lib.resolve("m:item/sigil").unwrap().uses_item_generator());
    }
    #[test]
    fn fluid_container_draws_its_base_or_the_bucket() {
        let lib = library(&[
            (
                "m:item/a",
                r#"{"loader":"neoforge:fluid_container","parent":"neoforge:item/bucket",
                    "textures":{"base":"m:item/jug","fluid":"neoforge:item/mask"}}"#,
            ),
            (
                "m:item/b",
                r#"{"loader":"neoforge:fluid_container","parent":"neoforge:item/bucket"}"#,
            ),
        ]);
        let a = lib.resolve("m:item/a").unwrap();
        assert!(a.uses_item_generator());
        assert_eq!(a.material("layer0").unwrap().unwrap().sprite, "m:item/jug");
        let b = lib.resolve("m:item/b").unwrap();
        assert_eq!(
            b.material("layer0").unwrap().unwrap().sprite,
            "minecraft:item/bucket"
        );
    }
    #[test]
    fn builtin_entity_mod_items_are_flat_and_vanilla_is_untouched() {
        let lib = library(&[
            (
                "m:item/staff",
                r#"{"parent":"builtin/entity","textures":{"particle":"m:item/staff_icon"}}"#,
            ),
            ("m:item/wand", r#"{"parent":"builtin/entity"}"#),
            ("minecraft:item/chest", r#"{"parent":"builtin/entity"}"#),
        ]);
        let staff = lib.resolve("m:item/staff").unwrap();
        assert!(staff.uses_item_generator());
        assert_eq!(
            staff.material("layer0").unwrap().unwrap().sprite,
            "m:item/staff_icon"
        );
        assert_eq!(
            lib.resolve("m:item/wand")
                .unwrap()
                .material("layer0")
                .unwrap()
                .unwrap()
                .sprite,
            "m:item/wand"
        );
        assert!(
            !lib.resolve("minecraft:item/chest")
                .unwrap()
                .uses_item_generator()
        );
    }
    #[test]
    fn unknown_loaders_keep_parent_or_elements_and_follow_model_references() {
        let lib = library(&[
            (
                "m:block/real",
                &format!(r#"{{"textures":{{"t":"m:block/r"}},"elements":[{CUBE}]}}"#),
            ),
            (
                "m:block/jar",
                r#"{"loader":"m:jar","textures":{"particle":"m:block/side"},"model":"m:block/real"}"#,
            ),
            (
                "m:block/rot",
                &format!(
                    r#"{{"loader":"m:extra_rotation","textures":{{"t":"m:block/r"}},"elements":[{CUBE}]}}"#
                ),
            ),
            (
                "m:block/knot",
                &format!(
                    r#"{{"loader":"m:rope_knot","knot":{{"textures":{{"t":"m:block/k"}},"elements":[{CUBE}]}}}}"#
                ),
            ),
            (
                "m:block/obj",
                r#"{"loader":"neoforge:obj","model":"m:models/x.obj","textures":{"particle":"m:block/p"}}"#,
            ),
        ]);
        assert_eq!(lib.resolve("m:block/jar").unwrap().elements().len(), 1);
        assert_eq!(lib.resolve("m:block/rot").unwrap().elements().len(), 1);
        let knot = lib.resolve("m:block/knot").unwrap();
        assert_eq!(knot.elements().len(), 1);
        assert_eq!(knot.material("t").unwrap().unwrap().sprite, "m:block/k");
        // OBJ geometry is attached by the model library from the .obj file; without one the
        // model still resolves (its texture fallback), it does not fail.
        assert!(lib.resolve("m:block/obj").is_ok());
    }
    #[test]
    fn geometryless_loader_models_fall_back_to_their_best_texture() {
        let lib = library(&[
            (
                "m:block/ctm",
                r#"{"loader":"m:ctm","ctm_textures":{"center":"m:block/c","empty":"m:block/e"}}"#,
            ),
            (
                "m:block/frame",
                r#"{"loader":"m:frame_block","overlay":"m:block/o","particle":"m:block/pp"}"#,
            ),
            (
                "m:item/book",
                r#"{"loader":"m:book","textures":{"a":"m:item/z","all":"m:item/all"}}"#,
            ),
            ("m:block/none", r#"{"loader":"m:way_sign"}"#),
        ]);
        let cube = lib.resolve("m:block/ctm").unwrap();
        assert_eq!(cube.elements().len(), 1);
        assert_eq!(cube.material("all").unwrap().unwrap().sprite, "m:block/c");
        let frame = lib.resolve("m:block/frame").unwrap();
        assert_eq!(frame.material("all").unwrap().unwrap().sprite, "m:block/pp");
        let item = lib.resolve("m:item/book").unwrap();
        assert!(item.uses_item_generator());
        assert_eq!(
            item.material("layer0").unwrap().unwrap().sprite,
            "m:item/all"
        );
        assert!(lib.resolve("m:block/none").unwrap().elements().is_empty());
    }
    #[test]
    fn blockstate_loader_texture_prefers_particle_then_center() {
        use crate::model::loader_fallback_texture as pick;
        assert_eq!(
            pick(
                r#"{"variants":{"":{"model":"a:b"}},"athena:loader":"athena:ctm","ctm_textures":{"center":"a:c","empty":"a:e","particle":"a:p"}}"#
            )
            .as_deref(),
            Some("a:p")
        );
        assert_eq!(
            pick(r#"{"ctm_textures":{"empty":"a:e","center":"a:c"}}"#).as_deref(),
            Some("a:c")
        );
        assert_eq!(pick(r#"{"variants":{}}"#), None);
    }
    #[test]
    fn obj_models_bake_their_faces_and_fall_back_to_a_cube_when_unreadable() {
        let json = r#"{"parent":"minecraft:block/block","loader":"neoforge:obj",
            "model":"m:models/block/x.obj","flip_v":true,
            "textures":{"0":"m:block/t","particle":"m:block/p"}}"#;
        let obj = "mtllib x.mtl
v 0 0 1
v 1 0 1
v 1 1 1
v 0 1 1
vt 0 0
vt 1 0
vt 1 1
vt 0 1
            usemtl a
f 1/1 2/2 3/3 4/4
f 1/1 2/2 3/3
";
        let read = |_: &str, path: &str| match path {
            "models/block/x.obj" => Some(obj.to_owned()),
            "models/block/x.mtl" => Some(
                "newmtl a
map_Kd #0
"
                .to_owned(),
            ),
            _ => None,
        };
        let mut model = CuboidModel::from_json(json).unwrap();
        model.attach_obj(&read).unwrap();
        let with_block = |model: CuboidModel| {
            let mut map = BTreeMap::new();
            map.insert(
                "minecraft:block/block".to_owned(),
                CuboidModel::from_json("{}").unwrap(),
            );
            map.insert("m:block/obj".to_owned(), model);
            ModelLibrary::new(map)
                .unwrap()
                .resolve("m:block/obj")
                .unwrap()
        };
        let resolved = with_block(model);
        let quads = crate::bake::bake_block_model(
            &resolved,
            crate::bake::BlockModelTransform::new([0; 3], false),
        )
        .unwrap();
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].material.sprite, "m:block/t");
        assert_eq!(quads[0].facing, Direction::South);
        assert_eq!(quads[0].cull, Some(Direction::South));
        assert_eq!(quads[0].uv[0], [0.0, 1.0]);
        assert_eq!(quads[1].positions[2], quads[1].positions[3]);

        let mut broken = CuboidModel::from_json(json).unwrap();
        assert!(broken.attach_obj(&|_: &str, _: &str| None).is_err());
        let resolved = with_block(broken);
        assert_eq!(resolved.elements().len(), 1);
        assert!(resolved.obj_quads().is_empty());
    }
}
