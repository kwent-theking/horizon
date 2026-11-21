//! Stage snapshots keep registry and renderer ownership separate. Payloads are
//! read sequentially; runtime handles are reconstructed only after validation.
use super::*;
use horizon_world::resource_cache::Cache;
use std::collections::BTreeMap;

type Core = (BlockRegistry, Vec<StateFacts>, [u32; 6], BTreeSet<String>);

pub(super) fn load_core(root: &Path) -> Result<Assets> {
    let started = std::time::Instant::now();
    let cache = Cache::new(
        root,
        "registry-facts-v1",
        &[
            &root.join("horizon-resources.json"),
            &root.join("facts.json"),
            &root.join("block-tags.json"),
        ],
        &[],
    );
    if let Some((registry, properties, materials, flow)) = cache
        .as_ref()
        .and_then(|cache| cache.read_value::<Core>())
        .filter(|(registry, properties, materials, _)| {
            registry.len() == properties.len()
                && materials.iter().zip(MATERIALS).all(|(&id, name)| {
                    registry.default_state(&format!("minecraft:{name}")) == Some(BlockStateId(id))
                })
        })
    {
        crate::chunk::set_block_states(registry.len());
        let loaded = Assets {
            registry: Box::leak(Box::new(registry)),
            properties: Properties(Arc::new(properties)),
            material_states: materials.map(BlockStateId),
            blocks_fluid_flow: flow,
        };
        log::info!(
            "Horizon: stage=registry_facts cache=hit elapsed_ms={}",
            started.elapsed().as_millis()
        );
        return Ok(loaded);
    }
    let loaded = Assets::load_uncached(root)?;
    if let Some(cache) = cache {
        let _ = cache.write_value(&(
            loaded.registry,
            loaded.properties.0.as_ref(),
            loaded.material_states.map(|id| id.0),
            &loaded.blocks_fluid_flow,
        ));
    }
    log::info!(
        "Horizon: stage=registry_facts cache=miss elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(loaded)
}

type Render = (
    u32,
    u32,
    crate::compact_bytes::CompactBytes,
    u32,
    crate::models::StateModels,
    Vec<crate::sprites::PackedSpriteAnimation>,
    BTreeMap<String, u32>,
    Vec<bool>,
    Vec<crate::mipmap::Settings>,
);

fn valid_render(data: &Render, states: usize) -> bool {
    let (size, minimum, pixels, layers, catalog, animations, names, translucent, mips) = data;
    let expected = (*size as usize)
        .checked_mul(*size as usize)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_mul(*layers as usize));
    *size > 0
        && size.is_power_of_two()
        && *minimum > 0
        && *minimum <= *size
        && *layers > 0
        && expected == Some(pixels.len())
        && translucent.len() == *layers as usize
        && mips.len() == *layers as usize
        && names.values().all(|id| *id < *layers)
        && mips.iter().all(|m| m.alpha_cutoff_bias.is_finite())
        && catalog.valid(states, *layers)
        && animations
            .iter()
            .all(|animation| animation.valid(*layers, *size))
}

pub(super) fn load_render(
    root: &Path,
    assets: &Assets,
    max_sprite_side: u32,
) -> Result<RenderAssets> {
    let started = std::time::Instant::now();
    let kind = if max_sprite_side > 0 {
        format!(
            "baked-render-v2-cap{}",
            max_sprite_side
                .max(crate::sprites::SPRITE_SIZE)
                .next_power_of_two()
        )
    } else {
        "baked-render-v2".to_owned()
    };
    let cache = Cache::for_tree(root, &kind, assets.registry.cache_identity());
    if let Some(data) = cache
        .as_ref()
        .and_then(|c| c.read_value::<Render>())
        .filter(|data| valid_render(data, assets.registry.len()))
    {
        let (size, minimum, pixels, layers, catalog, animations, names, translucent, mips) = data;
        if let Ok((placements, gallery)) = crate::models::gallery(assets.registry, &catalog) {
            let loaded = RenderAssets {
                sprite_size: size,
                sprite_minimum_size: minimum,
                pixels: std::sync::RwLock::new(pixels),
                texture_layers: layers,
                model_catalog: catalog,
                model_library: std::sync::Mutex::new(None),
                model_placements: placements,
                model_gallery: gallery,
                animations,
                sprite_layers: names,
                translucent_layers: translucent,
                sprite_mips: mips,
            };
            log::info!(
                "Horizon: stage=baked_render cache=hit elapsed_ms={}",
                started.elapsed().as_millis()
            );
            crate::logging::process_memory("baked render snapshot restored");
            return Ok(loaded);
        }
    }
    let loaded = RenderAssets::load_uncached(root, assets, max_sprite_side)?;
    if let Some(cache) = cache {
        loaded.with_pixels(|pixels| {
            let _ = cache.write_value(&(
                loaded.sprite_size,
                loaded.sprite_minimum_size,
                pixels,
                loaded.texture_layers,
                &loaded.model_catalog,
                &loaded.animations,
                &loaded.sprite_layers,
                &loaded.translucent_layers,
                &loaded.sprite_mips,
            ));
        });
    }
    log::info!(
        "Horizon: stage=baked_render cache=miss elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(loaded)
}

#[cfg(test)]
mod render_snapshot_tests {
    use super::*;
    use horizon_world::{chunk::BlockState, nbt::Tag, registry::BlockRegistry};
    use std::{collections::BTreeMap, fs};

    fn fixture(root: &Path, animation_size: u32) -> Render {
        use horizon_world::nbt::JavaString;
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        let registry = BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
            ("states", Tag::List(vec![air.to_nbt()])),
            (
                "defaults",
                Tag::compound([(JavaString::from("minecraft:air"), Tag::Int(0))]),
            ),
        ]))
        .unwrap();
        let mut sprites = crate::sprites::Sprites::default();
        let catalog = crate::models::StateModels::from_facts_with_models(
            root,
            &registry,
            &[],
            &mut sprites,
            &BTreeMap::<String, serde_json::Value>::new(),
            &crate::model_library::ParsedModels::default(),
        )
        .unwrap();
        let animation = crate::sprites::PackedSpriteAnimation::new(crate::sprites::test_animation(
            animation_size,
            0,
        ));
        (
            4,
            4,
            crate::compact_bytes::CompactBytes::new(vec![17; 4 * 4 * 4]),
            1,
            catalog,
            vec![animation],
            BTreeMap::from([("test:sprite".to_owned(), 0)]),
            vec![false],
            vec![crate::mipmap::Settings::default()],
        )
    }

    #[test]
    fn render_snapshot_round_trip_validates_selection_atlas_and_animation_pixels() {
        let root = std::env::temp_dir().join(format!(
            "horizon-render-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("horizon-mod-content.stamp"), "render-a").unwrap();
        let cache = Cache::new(&root, "render-payload-test-v1", &[], &[]).unwrap();
        let original = fixture(&root, 4);
        assert!(valid_render(&original, 1));
        cache.write_value(&original).unwrap();
        let restored: Render = cache.read_value().unwrap();
        assert!(valid_render(&restored, 1));
        assert_eq!(restored.0, original.0);
        assert_eq!(restored.1, original.1);
        assert_eq!(restored.2.decode().as_ref(), original.2.decode().as_ref());
        assert_eq!(restored.3, original.3);
        assert_eq!(
            restored.5[0].decode().pixels(1),
            original.5[0].decode().pixels(1)
        );

        assert!(!valid_render(&restored, 2));
        let mut bad_atlas = restored;
        bad_atlas.6.insert("test:bad".into(), 1);
        assert!(!valid_render(&bad_atlas, 1));
        let bad_animation = fixture(&root, 2);
        assert!(!valid_render(&bad_animation, 1));

        cache.write(&[0xff, 0x00]).unwrap();
        assert!(cache.read_value::<Render>().is_none());
        assert!(valid_render(&original, 1));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
