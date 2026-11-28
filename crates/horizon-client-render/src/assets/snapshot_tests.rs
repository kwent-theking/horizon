use super::*;
use std::fs;

#[test]
fn core_snapshot_round_trip_repair_and_source_invalidation() -> Result<()> {
    let root = std::env::temp_dir().join(format!("horizon-core-snapshot-{}", std::process::id()));
    fs::create_dir_all(&root)?;
    fs::write(root.join("horizon-mod-content.stamp"), "core-a")?;
    fs::write(
        root.join("horizon-resources.json"),
        format!(r#"{{"data_version":{DATA_VERSION}}}"#),
    )?;
    let names: Vec<_> = std::iter::once("air")
        .chain(MATERIALS)
        .map(|name| format!("minecraft:{name}"))
        .collect();
    let defaults: BTreeMap<_, _> = names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();
    let states: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, name)| serde_json::json!([name, i == 0, false, "", name]))
        .collect();
    fs::write(
        root.join("facts.json"),
        serde_json::to_vec(&serde_json::json!({
            "states": states, "defaults": defaults
        }))?,
    )?;
    let motion = serde_json::json!({"values": names[1..]}).to_string();
    fs::write(
        root.join("block-tags.json"),
        serde_json::to_vec(&serde_json::json!({
            "minecraft:blocks_motion_in_heightmap": motion,
            "minecraft:blocks_motion_in_heightmap_no_leaves": motion,
            "minecraft:blocks_fluid_flow": "{\"values\":[]}"
        }))?,
    )?;
    let cold = Assets::load(&root)?;
    let path = root.join(".horizon-derived/registry-facts-v1");
    let bytes = fs::read(&path)?;
    let warm = Assets::load(&root)?;
    assert_eq!(
        cold.registry.cache_identity(),
        warm.registry.cache_identity()
    );
    assert_eq!(cold.material_states, warm.material_states);
    assert_eq!(cold.blocks_fluid_flow, warm.blocks_fluid_flow);
    for i in 0..names.len() {
        let a = cold.properties.0[i];
        let b = warm.properties.0[i];
        assert_eq!(
            (a.air, a.fluid, a.air_block, a.motion, a.no_leaves),
            (b.air, b.fluid, b.air_block, b.motion, b.no_leaves)
        );
    }
    fs::write(&path, &bytes[..40])?;
    Assets::load(&root)?;
    assert_eq!(bytes, fs::read(&path)?);
    let mut broken = bytes.clone();
    *broken.last_mut().unwrap() ^= 1;
    fs::write(&path, broken)?;
    Assets::load(&root)?;
    assert_eq!(bytes, fs::read(&path)?);
    fs::write(root.join("horizon-mod-content.stamp"), "core-b")?;
    fs::write(root.join("horizon-resources.json"), "{}")?;
    assert!(Assets::load(&root).is_err());
    fs::remove_dir_all(root)?;
    Ok(())
}
