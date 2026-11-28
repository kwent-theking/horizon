use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    registry: BlockRegistry,
    behaviours: Value,
}

impl Fixture {
    fn new(gameplay: &str, behaviours: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "horizon-block-stream-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("gameplay-facts.json"), gameplay).unwrap();
        let registry = BlockRegistry::from_facts_json(
            &br#"{"defaults":{"minecraft:air":0,"minecraft:stone":1,"minecraft:dirt":2},
                "states":[
                  ["minecraft:air",true,false,"Block{minecraft:air}","minecraft:air"],
                  ["minecraft:stone",false,false,"Block{minecraft:stone}","minecraft:stone"],
                  ["minecraft:dirt",false,false,"Block{minecraft:dirt}","minecraft:dirt"]
                ]}"#[..],
        )
        .unwrap();
        Self {
            root,
            registry,
            behaviours,
        }
    }

    fn standard(gameplay: &str) -> Self {
        Self::new(gameplay, standard_behaviours())
    }

    fn load(&self) -> crate::Result<(BlockFacts, Vec<GameplayProjection>)> {
        BlockFacts::load_with_projection(&self.root, &self.registry, &self.behaviours)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn standard_behaviours() -> Value {
    serde_json::json!({
        "states": [[63, 0], [63, 1], [63, 1]],
        "blocks": [
            {"name": "minecraft:air", "class": ["AirBlock"]},
            {"name": "minecraft:stone", "class": ["Block"]},
            {"name": "minecraft:dirt", "class": ["Block"]}
        ]
    })
}

#[test]
fn repeated_block_facts_share_storage_but_keep_state_specific_fields() -> crate::Result<()> {
    let gameplay = gameplay_rows(["1.0", "1.0", "1.0"]);
    let mut fixture = Fixture::standard(&gameplay);
    fixture.registry = BlockRegistry::from_facts_json(
        &br#"{
        "defaults":{"minecraft:air":0,"minecraft:stone":1},
        "states":[["",true,false,"","minecraft:air"],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"a"}}],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"b"}}]]}"#[..],
    )?;
    let (mut facts, _) = fixture.load()?;
    assert!(Arc::ptr_eq(&facts.states[1].block, &facts.states[2].block));
    assert!(!Arc::ptr_eq(&facts.states[0].block, &facts.states[1].block));
    assert_eq!(facts.states[1].emission, 1);
    assert_eq!(facts.states[2].emission, 2);
    facts.states[1].random_ticks = false;
    assert!(!facts.states[1].random_ticks);
    assert!(facts.states[2].random_ticks);
    assert!(!Arc::ptr_eq(&facts.states[1].block, &facts.states[2].block));

    let mut rows: Value = serde_json::from_str(&gameplay)?;
    rows["states"][2]["hardness"] = serde_json::json!(9.0);
    fs::write(
        fixture.root.join("gameplay-facts.json"),
        serde_json::to_vec(&rows)?,
    )?;
    let (facts, _) = fixture.load()?;
    assert!(!Arc::ptr_eq(&facts.states[1].block, &facts.states[2].block));
    assert_eq!(facts.states[1].hardness, 1.5);
    assert_eq!(facts.states[2].hardness, 9.0);
    Ok(())
}

#[test]
fn block_fact_interning_checks_every_field_and_float_bits() -> crate::Result<()> {
    let fixture = Fixture::standard(&gameplay_rows(["1.0", "1.0", "1.0"]));
    let (facts, _) = fixture.load()?;
    let original = &*facts.states[1].block;
    let mut types = BlockTypes::default();
    let first = types.intern(original.clone());
    assert!(Arc::ptr_eq(&first, &types.intern(original.clone())));
    let changes: &[fn(&mut BlockTypeFacts)] = &[
        |f| f.name = Arc::from("other:block"),
        |f| f.hardness = -0.0,
        |f| f.requires_tool = !f.requires_tool,
        |f| f.random_ticks = !f.random_ticks,
        |f| f.block_entity = !f.block_entity,
        |f| f.render_shape = RenderShape::Invisible,
        |f| f.friction = -0.0,
        |f| f.explosion_resistance = -0.0,
        |f| f.item = Arc::from("other:item"),
        |f| {
            let mut sounds = (*f.sounds).clone();
            sounds.pitch = 2.0;
            f.sounds = Arc::new(sounds);
        },
        |f| f.random_offset = !f.random_offset,
        |f| f.outline_offset = !f.outline_offset,
        |f| f.classes = Arc::from([Arc::from("OtherClass")]),
    ];
    for change in changes {
        let mut variant = original.clone();
        change(&mut variant);
        let other = types.intern(variant.clone());
        assert!(!Arc::ptr_eq(&first, &other));
        assert!(Arc::ptr_eq(&other, &types.intern(variant)));
    }
    for field in [0, 1, 2] {
        let mut positive = original.clone();
        let mut negative = original.clone();
        match field {
            0 => {
                positive.hardness = 0.0;
                negative.hardness = -0.0;
            }
            1 => {
                positive.friction = 0.0;
                negative.friction = -0.0;
            }
            _ => {
                positive.explosion_resistance = 0.0;
                negative.explosion_resistance = -0.0;
            }
        }
        assert!(!Arc::ptr_eq(
            &types.intern(positive),
            &types.intern(negative)
        ));
    }
    // The state row keeps small flags and two shape handles, not repeated block metadata.
    assert!(std::mem::size_of::<StateFacts>() < std::mem::size_of::<BlockTypeFacts>());
    Ok(())
}

fn gameplay_rows(volumes: [&str; 3]) -> String {
    let entry = |id: usize, volume: &str| {
        format!(
            r#"{{"id":{id},"emission":{id},"dampening":2,"hardness":1.5,
              "replaceable":false,"random_ticks":true,"solid_render":true,
              "liquid":false,"block_entity":false,"can_occlude":true,
              "full_collision":true,"skylight_down":false,"requires_tool":true,
              "shape_occlusion":true,"ignited_by_lava":false,"push":"NORMAL",
              "volume":91,"pitch":92,"sound":{{"volume":{volume},"pitch":1.25,
                "break":{{"id":"minecraft:block.stone.break"}},
                "step":{{"id":"minecraft:block.stone.step"}},
                "place":{{"id":"minecraft:block.stone.place"}},
                "hit":{{"id":"minecraft:block.stone.hit"}},
                "fall":{{"id":"minecraft:block.stone.fall"}}}},
              "item":"minecraft:stone","render_shape":"MODEL","offset":false,
              "outline_offset":false,"outline":[],"outline_edges":[],"map_color":0}}"#
        )
    };
    format!(
        "{{\"states\":[{},{},{}]}}",
        entry(0, volumes[0]),
        entry(1, volumes[1]),
        entry(2, volumes[2])
    )
}

fn assert_projection_matches_entry(projection: &GameplayProjection, entry: &decode::Entry<'_>) {
    for flag in [
        "replaceable",
        "random_ticks",
        "solid_render",
        "liquid",
        "block_entity",
        "ignited_by_lava",
        "can_occlude",
        "full_collision",
        "skylight_down",
        "requires_tool",
        "shape_occlusion",
    ] {
        assert_eq!(projection.flag(flag), entry.flag(flag), "{flag}");
    }
    for field in ["emission", "dampening"] {
        assert_eq!(projection.unsigned(field), entry.unsigned(field), "{field}");
    }
    assert_eq!(projection.number("hardness"), entry.number("hardness"));
    assert_eq!(projection.text("push"), entry.text("push"));
    for field in ["volume", "pitch"] {
        assert_eq!(
            projection.sound_number(field).to_bits(),
            entry.sound_number(field).to_bits(),
            "{field}"
        );
    }
    for field in ["place", "break", "fall"] {
        assert_eq!(projection.sound_id(field), entry.sound_id(field), "{field}");
    }
}

fn assert_state_facts_equal(left: &StateFacts, right: &StateFacts) {
    assert_eq!(left.name, right.name);
    assert_eq!(left.emission, right.emission);
    assert_eq!(left.dampening, right.dampening);
    assert_eq!(left.skylight_down, right.skylight_down);
    assert_eq!(left.shape_occlusion, right.shape_occlusion);
    assert_eq!(left.can_occlude, right.can_occlude);
    assert_eq!(left.solid_render, right.solid_render);
    assert_eq!(left.hardness.to_bits(), right.hardness.to_bits());
    assert_eq!(left.requires_tool, right.requires_tool);
    assert_eq!(left.replaceable, right.replaceable);
    assert_eq!(left.random_ticks, right.random_ticks);
    assert_eq!(left.block_entity, right.block_entity);
    assert_eq!(left.render_shape, right.render_shape);
    assert_eq!(left.full_collision, right.full_collision);
    assert_eq!(left.friction.to_bits(), right.friction.to_bits());
    assert_eq!(
        left.explosion_resistance.to_bits(),
        right.explosion_resistance.to_bits()
    );
    assert_eq!(left.item, right.item);
    assert_eq!(left.sounds.volume.to_bits(), right.sounds.volume.to_bits());
    assert_eq!(left.sounds.pitch.to_bits(), right.sounds.pitch.to_bits());
    assert_eq!(left.sounds.break_sound, right.sounds.break_sound);
    assert_eq!(left.sounds.step, right.sounds.step);
    assert_eq!(left.sounds.place, right.sounds.place);
    assert_eq!(left.sounds.hit, right.sounds.hit);
    assert_eq!(left.sounds.fall, right.sounds.fall);
    assert_eq!(left.fluid, right.fluid);
    assert_eq!(left.fluid_level, right.fluid_level);
    assert_eq!(left.fluid_falling, right.fluid_falling);
    assert_eq!(left.waterlogged, right.waterlogged);
    assert_eq!(left.random_offset, right.random_offset);
    assert_eq!(left.outline_offset, right.outline_offset);
    assert_eq!(left.legacy_solid, right.legacy_solid);
    assert_eq!(left.sturdy_full, right.sturdy_full);
    assert_eq!(left.classes, right.classes);
    assert_eq!(left.outline, right.outline);
    assert_eq!(left.outline_edges, right.outline_edges);
    assert_eq!(left.map_color, right.map_color);
}

#[test]
fn projected_load_matches_plain_load_and_entry_projection() -> crate::Result<()> {
    let gameplay = gameplay_rows(["1.0", "1.0", "1.0"]);
    let fixture = Fixture::standard(&gameplay);
    let (projected_facts, projection) = fixture.load()?;
    let plain_facts = BlockFacts::load_with_behaviour_json(
        &fixture.root,
        &fixture.registry,
        &fixture.behaviours,
    )?;
    assert_eq!(projected_facts.states.len(), 3);
    assert_eq!(projection.len(), 3);
    assert_eq!(projected_facts.states[0].emission, 0);
    assert_eq!(projected_facts.states[2].emission, 2);
    assert_eq!(projected_facts.states[1].dampening, 2);
    assert_eq!(projected_facts.states[1].hardness, 1.5);
    assert_eq!(projected_facts.states[1].sounds.volume, 1.0);
    assert_eq!(projected_facts.states[1].sounds.pitch, 1.25);
    assert_eq!(projected_facts.states[0].sturdy_full, 63);
    assert_eq!(projected_facts.states[1].sturdy_full, 63);
    assert!(!projected_facts.states[0].legacy_solid);
    assert!(projected_facts.states[1].legacy_solid);
    for (left, right) in projected_facts.states.iter().zip(&plain_facts.states) {
        assert_state_facts_equal(left, right);
    }

    let parsed: decode::Gameplay<'_> = serde_json::from_str(&gameplay)?;
    let entries = parsed.states.0.unwrap();
    for (projected, entry) in projection.iter().zip(&entries) {
        assert_projection_matches_entry(projected, entry);
    }
    Ok(())
}

#[test]
fn interned_facts_match_every_unshared_fixture_row() -> crate::Result<()> {
    let mut rows: Value = serde_json::from_str(&gameplay_rows(["0.0", "0.0", "0.0"]))?;
    rows["states"][2]["emission"] = rows["states"][1]["emission"].clone();
    rows["states"][1]["outline"] = serde_json::json!([[0, 0, 0, 1, 1, 1]]);
    rows["states"][2]["outline"] = serde_json::json!([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
    let mut flags_variant = rows["states"][2].clone();
    flags_variant["id"] = serde_json::json!(3);
    flags_variant["can_occlude"] = serde_json::json!(false);
    let mut sound_variant = rows["states"][2].clone();
    sound_variant["id"] = serde_json::json!(4);
    sound_variant["sound"]["volume"] = serde_json::json!(-0.0);
    rows["states"]
        .as_array_mut()
        .unwrap()
        .extend([flags_variant, sound_variant]);
    let source = rows.to_string();
    let mut behaviours = standard_behaviours();
    behaviours["states"]
        .as_array_mut()
        .unwrap()
        .extend([serde_json::json!([63, 1]), serde_json::json!([63, 1])]);
    let mut fixture = Fixture::new(&source, behaviours);
    fixture.registry = BlockRegistry::from_facts_json(
        &br#"{"defaults":{"minecraft:air":0,"minecraft:stone":1},
          "states":[["",true,false,"","minecraft:air"],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"a"}}],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"b"}}],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"c"}}],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"d"}}]]}"#[..],
    )?;
    let (baseline, _) = BlockFacts::decode_projected(
        &fixture.root,
        &fixture.registry,
        &fixture.behaviours,
        false,
        StateRecords {
            unshared: true,
            ..StateRecords::default()
        },
    )?;
    let (cold, gameplay) = fixture.load()?;
    let (warm, warm_gameplay) = snapshot::Snapshot::capture(&cold, &gameplay)
        .restore()
        .unwrap();

    let entries: decode::Gameplay<'_> = serde_json::from_str(&source)?;
    let entries = entries.states.0.unwrap();
    for (facts, projection) in [(&cold, &gameplay), (&warm, &warm_gameplay)] {
        assert_eq!(facts.air, baseline.air);
        assert_eq!(facts.states.len(), entries.len());
        assert_eq!(projection.len(), entries.len());
        assert!(Arc::ptr_eq(&facts.states[1].0, &facts.states[2].0));
        assert!(Arc::ptr_eq(&projection[1].0, &projection[2].0));
        assert!(!Arc::ptr_eq(&facts.states[2].0, &facts.states[3].0));
        assert!(!Arc::ptr_eq(&projection[2].0, &projection[3].0));
        assert!(!Arc::ptr_eq(&facts.states[2].0, &facts.states[4].0));
        assert!(!Arc::ptr_eq(&projection[2].0, &projection[4].0));
        for (i, entry) in entries.iter().enumerate() {
            let id = BlockStateId(i as u32);
            assert_state_facts_equal(facts.get(id), baseline.get(id));
            let descriptor = fixture.registry.state(id).unwrap();
            assert_eq!(facts.get(id).name.as_ref(), descriptor.name.as_str());
            assert_eq!(
                descriptor.properties.get("test").map(String::as_str),
                [None, Some("a"), Some("b"), Some("c"), Some("d")][i]
            );
            assert_projection_matches_entry(&projection[i], entry);
        }
    }
    assert!(!Arc::ptr_eq(&baseline.states[1].0, &baseline.states[2].0));
    Ok(())
}

#[test]
fn duplicate_states_keys_use_the_last_array_and_discard_prior_row_errors() -> crate::Result<()> {
    let rows = gameplay_rows(["1.0", "1.0", "1.0"]);
    let valid = rows
        .strip_prefix("{\"states\":")
        .unwrap()
        .strip_suffix('}')
        .unwrap();
    let gameplay = format!("{{\"states\":[{{\"id\":99}},{{\"id\":98}}],\"states\":{valid}}}");
    let fixture = Fixture::standard(&gameplay);
    let (facts, projection) = fixture.load()?;
    assert_eq!(facts.states.len(), 3);
    assert_eq!(projection.len(), 3);
    assert_eq!(facts.states[0].name.as_ref(), "minecraft:air");
    Ok(())
}

#[test]
fn final_state_count_error_precedes_row_domain_errors() {
    let gameplay = r#"{"states":[{"id":99}]}"#;
    let fixture = Fixture::standard(gameplay);
    let error = fixture.load().err().unwrap().to_string();
    assert_eq!(error, "gameplay facts do not match the block registry");
}

#[test]
fn missing_behaviour_table_errors_precede_row_domain_errors() {
    let gameplay = gameplay_rows(["1.0", "1.0", "1.0"]).replace("\"id\":0", "\"id\":99");
    let behaviours = serde_json::json!({
        "states": [[63, 0]],
        "blocks": [
            {"name": "minecraft:air", "class": ["AirBlock"]},
            {"name": "minecraft:stone", "class": ["Block"]},
            {"name": "minecraft:dirt", "class": ["Block"]}
        ]
    });
    let fixture = Fixture::new(&gameplay, behaviours);
    let error = fixture.load().err().unwrap().to_string();
    assert_eq!(
        error,
        "block behaviour facts do not match the block registry"
    );
}

#[test]
fn identical_sounds_share_arcs_but_signed_zero_remains_distinct() -> crate::Result<()> {
    let gameplay = gameplay_rows(["0.0", "0.0", "-0.0"]);
    let fixture = Fixture::standard(&gameplay);
    let (facts, projection) = fixture.load()?;
    assert!(Arc::ptr_eq(
        &facts.states[0].sounds,
        &facts.states[1].sounds
    ));
    assert!(!Arc::ptr_eq(
        &facts.states[0].sounds,
        &facts.states[2].sounds
    ));
    assert_eq!(facts.states[0].sounds.volume.to_bits(), 0.0f32.to_bits());
    assert_eq!(facts.states[2].sounds.volume.to_bits(), (-0.0f32).to_bits());
    assert!(Arc::ptr_eq(&projection[0].0.place, &projection[1].0.place));
    assert!(Arc::ptr_eq(&projection[0].0.place, &projection[2].0.place));
    Ok(())
}

#[test]
fn binary_snapshot_round_trip_preserves_float_bits_and_shared_records() -> crate::Result<()> {
    use horizon_world::resource_cache::Cache;
    let mut fixture = Fixture::standard(&gameplay_rows(["1.0", "1.0", "1.0"]));
    fixture.registry = BlockRegistry::from_facts_json(
        &br#"{"defaults":{"minecraft:air":0,"minecraft:stone":1},
        "states":[["",true,false,"","minecraft:air"],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"a"}}],
          ["",false,false,"",{"id":"minecraft:stone","properties":{"test":"b"}}]]}"#[..],
    )?;
    let (mut facts, projection) = fixture.load()?;
    // Signed zero must survive both the shared-record pool and postcard.
    Arc::make_mut(&mut facts.states[0].block).hardness = -0.0;
    fs::write(
        fixture.root.join("horizon-mod-content.stamp"),
        "snapshot-fixture",
    )?;
    let cache = Cache::new(&fixture.root, "round-trip-v1", &[], &[]).unwrap();
    let original = snapshot::Snapshot::capture(&facts, &projection);
    let expected = Cache::value_identity(&original);
    cache.write_value(&original)?;
    let (warm, gameplay) = cache
        .read_value::<snapshot::Snapshot>()
        .unwrap()
        .restore()
        .unwrap();
    assert_eq!(
        expected,
        Cache::value_identity(&snapshot::Snapshot::capture(&warm, &gameplay))
    );
    assert_eq!(warm.states[0].hardness.to_bits(), (-0.0f32).to_bits());
    assert!(Arc::ptr_eq(&warm.states[1].block, &warm.states[2].block));
    assert!(Arc::ptr_eq(
        &warm.states[1].outline,
        &warm.states[2].outline
    ));
    assert!(Arc::ptr_eq(&warm.states[1].sounds, &warm.states[2].sounds));
    assert!(Arc::ptr_eq(&gameplay[1].0.place, &gameplay[2].0.place));
    Ok(())
}

#[test]
fn stamped_facts_fall_back_on_corruption_changed_sources_and_supplied_behaviours()
-> crate::Result<()> {
    let fixture = Fixture::standard(&gameplay_rows(["1.0", "1.0", "1.0"]));
    fs::write(fixture.root.join("horizon-mod-content.stamp"), "facts-a")?;
    fs::write(
        fixture.root.join("block-behaviours.json"),
        serde_json::to_vec(&fixture.behaviours)?,
    )?;
    let (cold, _) = fixture.load()?;
    let path = fixture.root.join(".horizon-derived/block-facts-v3");
    let original = fs::read(&path)?;
    let (warm, _) = fixture.load()?;
    assert_eq!(
        cold.states[1].hardness.to_bits(),
        warm.states[1].hardness.to_bits()
    );
    assert_eq!(fs::read(&path)?, original);

    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(&path, corrupt)?;
    fixture.load()?;
    assert_eq!(fs::read(&path)?, original);
    fs::write(&path, &original[..40])?;
    fixture.load()?;
    assert_eq!(fs::read(&path)?, original);

    let mut changed = fixture.behaviours.clone();
    changed["states"][1][1] = serde_json::json!(0);
    let (facts, _) = BlockFacts::load_with_projection(&fixture.root, &fixture.registry, &changed)?;
    assert!(!facts.states[1].legacy_solid);
    assert!(fixture.load()?.0.states[1].legacy_solid);

    let mut gameplay: Value = serde_json::from_str(&gameplay_rows(["1.0", "1.0", "1.0"]))?;
    gameplay["states"][1]["hardness"] = serde_json::json!(123.5);
    fs::write(
        fixture.root.join("gameplay-facts.json"),
        serde_json::to_vec(&gameplay)?,
    )?;
    assert_eq!(fixture.load()?.0.states[1].hardness, 123.5);
    // Stamp mismatch must take the source loader, including its error.
    fs::write(fixture.root.join("horizon-mod-content.stamp"), "facts-b")?;
    fs::write(fixture.root.join("gameplay-facts.json"), b"invalid source")?;
    assert!(fixture.load().is_err());
    Ok(())
}

#[test]
fn snapshot_write_failure_keeps_authoritative_facts_available() -> crate::Result<()> {
    let fixture = Fixture::standard(&gameplay_rows(["1.0", "1.0", "1.0"]));
    fs::write(fixture.root.join("horizon-mod-content.stamp"), "facts-a")?;
    fs::write(
        fixture.root.join("block-behaviours.json"),
        serde_json::to_vec(&fixture.behaviours)?,
    )?;
    fs::write(fixture.root.join(".horizon-derived"), b"not a directory")?;
    assert_eq!(fixture.load()?.0.states.len(), 3);
    Ok(())
}

#[test]
fn root_facts_snapshot_skips_behaviour_json_parse_on_hit_and_invalidates_on_change()
-> crate::Result<()> {
    let fixture = Fixture::standard(&gameplay_rows(["1.0", "1.0", "1.0"]));
    fs::write(fixture.root.join("horizon-mod-content.stamp"), "root-a")?;
    fs::write(
        fixture.root.join("block-behaviours.json"),
        serde_json::to_vec(&fixture.behaviours)?,
    )?;
    let (cold, proj, parsed) =
        BlockFacts::load_root_with_projection(&fixture.root, &fixture.registry, true)?;
    assert!(parsed.is_some());
    assert_eq!(proj.len(), 3);
    let (warm, warm_proj, warm_parsed) =
        BlockFacts::load_root_with_projection(&fixture.root, &fixture.registry, true)?;
    assert!(warm_parsed.is_none());
    assert_eq!(warm.states.len(), cold.states.len());
    assert_eq!(warm_proj.len(), proj.len());
    assert_eq!(
        warm.states[1].hardness.to_bits(),
        cold.states[1].hardness.to_bits()
    );

    let mut changed = fixture.behaviours.clone();
    changed["states"][1][1] = serde_json::json!(0);
    fs::write(
        fixture.root.join("block-behaviours.json"),
        serde_json::to_vec(&changed)?,
    )?;
    let (reloaded, _, reloaded_parsed) =
        BlockFacts::load_root_with_projection(&fixture.root, &fixture.registry, true)?;
    assert!(reloaded_parsed.is_some());
    assert!(!reloaded.states[1].legacy_solid);
    Ok(())
}
