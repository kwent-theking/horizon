use super::*;
use crate::{assets::Assets, blocks::BlockFacts, items::Items};

#[test]
fn renames_ids_keys_and_states() {
    let mut r = Rename::new("minecraft:iron_block", "ruby:ruby_block");
    r.states = 10..12;
    r.first = 100;
    assert_eq!(
        r.string("minecraft:iron_block").as_deref(),
        Some("ruby:ruby_block")
    );
    assert_eq!(
        r.string("Block{minecraft:iron_block}").as_deref(),
        Some("Block{ruby:ruby_block}")
    );
    assert_eq!(
        r.string("block.minecraft.iron_block").as_deref(),
        Some("block.ruby.ruby_block")
    );
    assert_eq!(r.string("minecraft:iron_blocks"), None);
    assert_eq!(r.string("block.minecraft.iron_bars"), None);
    assert_eq!(
        (r.id(9), r.id(10), r.id(11), r.id(12)),
        (None, Some(100), Some(101), None)
    );
}

#[test]
fn json_keeps_key_order_and_numbers() {
    let text = r#"{"z":1,"a":[0.6,5.0,-1,{"m":null,"b":true}],"k":"v"}"#;
    let doc: J = serde_json::from_str(text).unwrap();
    assert_eq!(serde_json::to_string(&doc).unwrap(), text);
}

#[test]
fn item_components_patch_the_template_entry() {
    let mut entry: J =
        serde_json::from_str(r#"{"minecraft:max_damage": 250, "minecraft:repairable": {}}"#)
            .unwrap();
    let known: BTreeSet<String> = [
        "minecraft:max_damage",
        "minecraft:repairable",
        "minecraft:food",
    ]
    .map(str::to_owned)
    .into();
    let patch = |text: &str| -> Vec<(String, J)> {
        let J::Obj(o) = serde_json::from_str(text).unwrap() else {
            unreachable!()
        };
        o
    };
    patch_components(
        &mut entry,
        &patch(r#"{"minecraft:max_damage": 900, "!minecraft:repairable": {}, "minecraft:food": {"nutrition": 4}}"#),
        &known,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&entry).unwrap(),
        r#"{"minecraft:max_damage":900,"minecraft:food":{"nutrition":4}}"#
    );
    for typo in [r#"{"minecraft:fod": {}}"#, r#"{"!minecraft:fod": {}}"#] {
        let e = patch_components(&mut entry, &patch(typo), &known).unwrap_err();
        assert_eq!(e, "minecraft:fod is no component a vanilla item has");
    }
}

#[test]
fn tags_merge_without_repeats() {
    let vanilla = r#"{"values": ["minecraft:stone"]}"#;
    let more = [
        J::Str("ruby:ruby_block".into()),
        J::Str("minecraft:stone".into()),
    ];
    let merged: serde_json::Value =
        serde_json::from_str(&merge_tag(Some(vanilla), &more, "t").unwrap()).unwrap();
    assert_eq!(
        merged["values"],
        serde_json::json!(["minecraft:stone", "ruby:ruby_block"])
    );
}

/// Registry IDs and state IDs must not depend on package discovery order.
#[test]
fn allocated_ids_follow_mod_and_entry_order_after_vanilla() {
    let scratch = std::env::temp_dir().join(format!("horizon-mod-order-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let base = scratch.join("base");
    fs::create_dir_all(&base).unwrap();
    fs::write(
        base.join("facts.json"),
        r#"{"states": [["minecraft:iron_block"], ["minecraft:iron_block"], ["minecraft:stone"]],
            "defaults": {"minecraft:iron_block": 0, "minecraft:stone": 2}}"#,
    )
    .unwrap();
    fs::write(
        base.join("gameplay-facts.json"),
        r#"{"states": [{"item":"minecraft:iron_block"}, {"item":"minecraft:iron_block"},
            {"item":"minecraft:stone"}]}"#,
    )
    .unwrap();
    fs::write(
        base.join("items.json"),
        r#"{"minecraft:stone": {}, "minecraft:iron_block": {}}"#,
    )
    .unwrap();
    fs::write(
        base.join("registries.json"),
        r#"{"minecraft:item":{"entries":{"minecraft:stone":{"protocol_id":4},
            "minecraft:iron_block":{"protocol_id":9}}},
            "minecraft:block":{"entries":{"minecraft:stone":{"protocol_id":7},
            "minecraft:iron_block":{"protocol_id":11}}}}"#,
    )
    .unwrap();
    fs::create_dir_all(base.join("assets/minecraft/blockstates")).unwrap();
    fs::write(
        base.join("assets/minecraft/blockstates/iron_block.json"),
        "{}",
    )
    .unwrap();
    fs::write(base.join("assets/minecraft/blockstates/stone.json"), "{}").unwrap();
    let ruby = folder_mod(
        &scratch.join("ruby"),
        &[
            ("horizon-mod.toml", &manifest("ruby", "")),
            (
                "data/ruby/horizon/block/zblock.json",
                r#"{"template":"minecraft:stone"}"#,
            ),
            (
                "data/ruby/horizon/item/aitem.json",
                r#"{"template":"minecraft:stone"}"#,
            ),
        ],
    );
    let alpha = folder_mod(
        &scratch.join("alpha"),
        &[
            ("horizon-mod.toml", &manifest("alpha", "")),
            (
                "data/alpha/horizon/block/zblock.json",
                r#"{"template":"minecraft:iron_block"}"#,
            ),
            (
                "data/alpha/horizon/item/aitem.json",
                r#"{"template":"minecraft:stone"}"#,
            ),
        ],
    );
    let mut mods = vec![ruby, alpha];
    for index in 0..2 {
        let plan = Plan::new(&base, &mods, None).unwrap();
        let blocks: Vec<_> = plan
            .blocks
            .iter()
            .map(|block| {
                (
                    block.decl.id.as_str(),
                    block.rename.first,
                    block.rename.states.len(),
                )
            })
            .collect();
        assert_eq!(blocks, [("alpha:zblock", 3, 2), ("ruby:zblock", 5, 1)]);
        let items: Vec<_> = plan
            .items
            .iter()
            .map(|item| item.rename.to.as_str())
            .collect();
        assert_eq!(
            items,
            ["alpha:aitem", "alpha:zblock", "ruby:aitem", "ruby:zblock"]
        );

        // Exercise the same registry rewrite the game and server read.
        let mut registry: J = serde_json::from_str(
            r#"{"minecraft:item":{"entries":{"minecraft:stone":{"protocol_id":4},
                "minecraft:iron_block":{"protocol_id":9}}},
                "minecraft:block":{"entries":{"minecraft:stone":{"protocol_id":7},
                "minecraft:iron_block":{"protocol_id":11}}}}"#,
        )
        .unwrap();
        plan.json("registries.json", &mut registry).unwrap();
        let registry: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&registry).unwrap()).unwrap();
        let id = |kind: &str, name: &str| {
            registry[kind]["entries"][name]["protocol_id"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(id("minecraft:item", "minecraft:stone"), 4);
        assert_eq!(id("minecraft:item", "minecraft:iron_block"), 9);
        assert_eq!(id("minecraft:item", "alpha:aitem"), 10);
        assert_eq!(id("minecraft:item", "alpha:zblock"), 11);
        assert_eq!(id("minecraft:item", "ruby:aitem"), 12);
        assert_eq!(id("minecraft:item", "ruby:zblock"), 13);
        assert_eq!(id("minecraft:block", "minecraft:stone"), 7);
        assert_eq!(id("minecraft:block", "minecraft:iron_block"), 11);
        assert_eq!(id("minecraft:block", "alpha:zblock"), 12);
        assert_eq!(id("minecraft:block", "ruby:zblock"), 13);
        drop(plan);

        let output = scratch.join(format!("output-{index}"));
        build(&base, &mods, &output).unwrap();
        let built: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("registries.json")).unwrap()).unwrap();
        assert_eq!(
            built, registry,
            "the built root must use the tested allocation"
        );
        mods.reverse();
    }
    fs::remove_dir_all(scratch).unwrap();
}

fn prepared() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        manifest.join("../../.local/horizon-client/26.3"),
        manifest.join("../../../../.local/horizon-client/26.3"),
    ]
    .into_iter()
    .find(|root| root.join("gameplay-facts.json").exists())
}

/// A content mod folder: a full block, a door and an item-less glass from
/// templates, an item, a vanilla tag addition and a language file.
fn ruby_mod(dir: &Path) -> ContentMod {
    let files: [(&str, &str); 9] = [
        (
            "horizon-mod.toml",
            "id = \"ruby\"\nname = \"Ruby\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"registries\"]\n",
        ),
        (
            "data/ruby/horizon/block/ruby_block.json",
            r#"{"template": "minecraft:iron_block", "properties": {"hardness": 4.0, "light": 3,
                "sounds": "minecraft:amethyst_block", "map_color": 28, "friction": 0.8}}"#,
        ),
        (
            "data/ruby/horizon/block/ruby_door.json",
            r#"{"template": "minecraft:oak_door"}"#,
        ),
        (
            "data/ruby/horizon/block/ruby_glass.json",
            r#"{"template": "minecraft:glass", "item": false}"#,
        ),
        (
            "data/ruby/horizon/item/ruby.json",
            r#"{"template": "minecraft:diamond", "max_stack_size": 16, "rarity": "rare"}"#,
        ),
        (
            "data/ruby/horizon/item/ruby_sword.json",
            r#"{"template": "minecraft:iron_sword", "components": {"minecraft:max_damage": 900,
                "minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage",
                    "id": "minecraft:base_attack_damage", "amount": 7.0,
                    "operation": "add_value", "slot": "mainhand"}],
                "!minecraft:repairable": {}}}"#,
        ),
        // Carried-over tags name blocks left out and other mods' ids too, and
        // 1.21.1's vanilla tags 26.3 renamed (`concrete_powders`).
        (
            "data/minecraft/tags/block/mineable/pickaxe.json",
            r##"{"values": ["ruby:ruby_block", "ruby:left_out", "othermod:thing",
                {"id": "othermod:optional", "required": false}, "#c:storage_blocks/ruby",
                "#minecraft:concrete_powder"]}"##,
        ),
        (
            "data/c/tags/item/gems/ruby.json",
            r#"{"values": ["ruby:ruby", "othermod:ruby"]}"#,
        ),
        (
            "assets/ruby/lang/en_us.json",
            r#"{"block.ruby.ruby_block": "Block of Ruby"}"#,
        ),
    ];
    for (name, text) in files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let package = horizon_mods::Package::read(dir).unwrap();
    ContentMod {
        id: "ruby".into(),
        content: Content::read(dir, &package.manifest).unwrap(),
    }
}

/// The overlay over the real prepared resources: every loader reads it, the
/// new blocks behave like their templates with the overrides, and vanilla's
/// states are where they were.
#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn content_root_loads_with_every_table() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-content-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let m = ruby_mod(&scratch.join("ruby"));
    // The base as a relative path (cargo runs tests in the crate's folder):
    // the overlay's links still resolve.
    let relative = Path::new("../../.local/horizon-client/26.3");
    let root = resource_root(relative, std::slice::from_ref(&m), &scratch.join("cache")).unwrap();
    assert_ne!(root, base);

    let vanilla = Assets::load(&base).unwrap();
    let assets = Assets::load(&root).unwrap();
    let (old, new) = (vanilla.registry, assets.registry);
    let n = old.len() as u32;
    let iron = old.default_state("minecraft:iron_block").unwrap();
    let door = old.default_state("minecraft:oak_door").unwrap();
    let door_first = (0..n)
        .map(BlockStateId)
        .find(|&s| old.state(s).unwrap().name == "minecraft:oak_door")
        .unwrap();
    // Vanilla keeps its ids; blocks come after, sorted: ruby_block, ruby_door, ruby_glass.
    assert_eq!(new.len(), old.len() + 1 + 64 + 1);
    assert_eq!(new.default_state("minecraft:iron_block"), Some(iron));
    let ruby = new.default_state("ruby:ruby_block").unwrap();
    assert_eq!(ruby, BlockStateId(n));
    let ruby_door = new.default_state("ruby:ruby_door").unwrap();
    assert_eq!(ruby_door, BlockStateId(n + 1 + (door.0 - door_first.0)));
    assert_eq!(
        new.default_state("ruby:ruby_glass"),
        Some(BlockStateId(n + 65))
    );
    // Network ids: the new items and blocks follow vanilla's in the registries report, which
    // the game and its server both read (a missing one travels as air).
    let report = |root: &Path| {
        horizon_protocol::registry::Builtin::load(&root.join("registries.json")).unwrap()
    };
    let (vanilla_ids, ids) = (report(&base), report(&root));
    let item = |b: &horizon_protocol::registry::Builtin, name: &str| {
        b.get("minecraft:item").and_then(|r| r.id(name))
    };
    let vanilla_items = (0..)
        .take_while(|&i| vanilla_ids.get("minecraft:item").unwrap().name(i).is_some())
        .count() as i32;
    assert_eq!(
        item(&ids, "minecraft:stone"),
        item(&vanilla_ids, "minecraft:stone")
    );
    let ruby_item = item(&ids, "ruby:ruby_block").unwrap();
    assert!(ruby_item >= vanilla_items, "{ruby_item} < {vanilla_items}");
    assert_eq!(
        ids.get("minecraft:item").unwrap().name(ruby_item),
        Some("ruby:ruby_block")
    );
    assert!(item(&ids, "ruby:ruby_glass").is_none(), "no item declared");
    assert!(
        ids.get("minecraft:block")
            .unwrap()
            .id("ruby:ruby_glass")
            .is_some()
    );

    let old_facts = BlockFacts::load(&base, old).unwrap();
    let facts = BlockFacts::load(&root, new).unwrap();
    let shared_json: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(
        fs::File::open(root.join("block-behaviours.json")).unwrap(),
    ))
    .unwrap();
    let (shared_facts, shared_behaviours) =
        BlockFacts::load_with_behaviours(&root, new, &shared_json).unwrap();
    let independent_behaviours = horizon_world::level::Behaviours::load(new, &root).unwrap();
    let (deferred_facts, gameplay) =
        BlockFacts::load_with_projection(&root, new, &shared_json).unwrap();
    let deferred_behaviours =
        horizon_world::level::Behaviours::load_with_gameplay(new, &root, &shared_json, &gameplay)
            .unwrap();
    assert_eq!(shared_facts.states.len(), facts.states.len());
    for state in [ruby, ruby_door, BlockStateId(n + 65)] {
        assert_eq!(shared_facts.get(state).hardness, facts.get(state).hardness);
        assert_eq!(
            deferred_facts.get(state).hardness,
            facts.get(state).hardness
        );
        assert_eq!(deferred_facts.get(state).classes, facts.get(state).classes);
        assert_eq!(
            deferred_behaviours.facts(state).flags,
            independent_behaviours.facts(state).flags
        );
        assert_eq!(
            deferred_behaviours.data(state).sounds.place,
            independent_behaviours.data(state).sounds.place
        );
        assert_eq!(shared_facts.get(state).emission, facts.get(state).emission);
        assert_eq!(shared_facts.get(state).classes, facts.get(state).classes);
        assert_eq!(
            shared_behaviours.facts(state).flags,
            independent_behaviours.facts(state).flags,
        );
        assert_eq!(
            shared_behaviours.data(state).sounds.place,
            independent_behaviours.data(state).sounds.place,
        );
    }

    let (a, b) = (old_facts.get(iron), facts.get(ruby));
    assert_eq!((b.hardness, b.emission, b.friction), (4.0, 3, 0.8));
    assert_eq!(b.explosion_resistance, a.explosion_resistance);
    assert_eq!(b.outline, a.outline);
    assert_eq!(b.classes, a.classes);
    assert_eq!(&*b.item, "ruby:ruby_block");
    assert_eq!(b.map_color, 28);
    let amethyst = old_facts.get(old.default_state("minecraft:amethyst_block").unwrap());
    assert_eq!(b.sounds.break_sound, amethyst.sounds.break_sound);
    for i in 0..64 {
        let (v, m) = (
            old_facts.get(BlockStateId(door_first.0 + i)),
            facts.get(BlockStateId(n + 1 + i)),
        );
        assert_eq!(
            (v.outline.clone(), v.hardness, v.classes.clone()),
            (m.outline.clone(), m.hardness, m.classes.clone())
        );
        assert_eq!(&*m.name, "ruby:ruby_door");
    }
    let glass = facts.get(BlockStateId(n + 65));
    assert_eq!(&*glass.item, "minecraft:air");
    // Vanilla's own rows are untouched.
    assert_eq!(facts.get(iron).hardness, a.hardness);
    assert_eq!(&*facts.get(iron).name, "minecraft:iron_block");

    let items = Items::load(&root, new, &facts).unwrap();
    let ruby_block = &items.items["ruby:ruby_block"];
    assert_eq!(ruby_block.block, Some(ruby));
    let gem = &items.items["ruby:ruby"];
    assert_eq!(gem.max_stack, 16);
    let sword = &items.items["ruby:ruby_sword"];
    let iron_sword = &items.items["minecraft:iron_sword"];
    assert_eq!((sword.max_damage, iron_sword.max_damage), (900, 250));
    assert_eq!(sword.modifiers.len(), 1);
    assert_eq!(sword.modifiers[0].2, 7.0);
    assert_eq!(sword.tool.len(), iron_sword.tool.len());
    assert!(!items.items.contains_key("ruby:ruby_glass"));
    let pickaxe = &items.block_tags["minecraft:mineable/pickaxe"];
    assert!(pickaxe.contains("ruby:ruby_block"));
    assert!(!pickaxe.contains("ruby:left_out") && !pickaxe.contains("othermod:thing"));
    assert!(pickaxe.contains("minecraft:stone"));
    // Convention tags load, with the ids nobody registers dropped.
    assert_eq!(items.item_tag_order["c:gems/ruby"], ["ruby:ruby"]);

    crate::collisions::load(&root, new).unwrap();
    crate::models::load_visibility(&root, new).unwrap();
    crate::light::LightTables::load(&root, new, &facts).unwrap();
    crate::motion::MotionResources::load(&root, &assets).unwrap();
    horizon_world::worldgen::generator::GeneratorData::load(&root.join("worldgen"), new).unwrap();
    assert!(root.join("assets/ruby/lang/en_us.json").is_file());
    // Blocks without a blockstate of their own take their template's.
    assert_eq!(
        fs::read(root.join("assets/ruby/blockstates/ruby_door.json")).unwrap(),
        fs::read(base.join("assets/minecraft/blockstates/oak_door.json")).unwrap()
    );

    // The base root was only read.
    assert!(
        fs::symlink_metadata(base.join("facts.json"))
            .unwrap()
            .is_file()
    );
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn refuses_unknown_templates_and_vanilla_ids() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-refuse-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let mut m = ruby_mod(&scratch.join("ruby"));
    m.content.blocks[0].template = "minecraft:not_a_block".into();
    let e = build(&base, std::slice::from_ref(&m), &scratch.join("out")).unwrap_err();
    assert!(e.to_string().contains("is not a block"), "{e}");
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn refuses_more_states_than_a_world_holds() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-states-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let mut m = ruby_mod(&scratch.join("ruby"));
    // Redstone wire has 1296 states: 810 copies pass 2^20 with any game.
    let mut wire = m.content.blocks[0].clone();
    wire.template = "minecraft:redstone_wire".into();
    for i in 0..810 {
        let mut b = wire.clone();
        b.id = format!("ruby:wire_{i}");
        m.content.blocks.push(b);
    }
    let e = build(&base, std::slice::from_ref(&m), &scratch.join("out")).unwrap_err();
    assert!(
        e.to_string()
            .contains("more than the 1048576 a world can hold"),
        "{e}"
    );
    assert!(e.to_string().contains("the mods: ruby "), "{e}");
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn builds_past_sixteen_bit_states() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-wide-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let mut m = ruby_mod(&scratch.join("ruby"));
    // 40 redstone wires: 51,840 states, past 65,536 with vanilla's 35,723.
    let mut wire = m.content.blocks[0].clone();
    wire.template = "minecraft:redstone_wire".into();
    for i in 0..40 {
        let mut b = wire.clone();
        b.id = format!("ruby:wire_{i}");
        m.content.blocks.push(b);
    }
    build(&base, std::slice::from_ref(&m), &scratch.join("out")).unwrap();
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn no_content_keeps_the_base_root() {
    let base = Path::new("/nonexistent/base");
    assert_eq!(
        resource_root(base, &[], Path::new("/nonexistent/cache")).unwrap(),
        base
    );
}

use horizon_world::registry::BlockStateId;

#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn shared_behaviour_document_matches_independent_loaders() {
    let Some(root) = prepared() else { return };
    let assets = Assets::load(&root).expect("assets");
    let doc: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(root.join("block-behaviours.json")).unwrap(),
    ))
    .unwrap();
    let original = BlockFacts::load(&root, assets.registry).unwrap();
    let (shared, b) = BlockFacts::load_with_behaviours(&root, assets.registry, &doc).unwrap();
    let (deferred_facts, projection) =
        BlockFacts::load_with_projection(&root, assets.registry, &doc).unwrap();
    let deferred = horizon_world::level::Behaviours::load_with_gameplay(
        assets.registry,
        &root,
        &doc,
        &projection,
    )
    .unwrap();
    assert_eq!(deferred_facts.states.len(), shared.states.len());
    assert_eq!(original.states.len(), shared.states.len());
    for (a, b) in original.states.iter().zip(&shared.states) {
        assert_eq!(a.classes, b.classes);
        assert_eq!(a.legacy_solid, b.legacy_solid);
        assert_eq!(a.sturdy_full, b.sturdy_full);
        assert_eq!(a.emission, b.emission);
        assert_eq!(a.dampening, b.dampening);
        assert_eq!(a.hardness, b.hardness);
        assert_eq!(a.sounds.volume, b.sounds.volume);
        assert_eq!(a.sounds.pitch, b.sounds.pitch);
    }
    let a = horizon_world::level::Behaviours::load(assets.registry, &root).unwrap();
    for id in 0..assets.registry.len() {
        let state = horizon_world::registry::BlockStateId(id as u32);
        let (a_state, b_state) = (a.facts(state), b.facts(state));
        let deferred_state = deferred.facts(state);
        assert_eq!(b_state.flags, deferred_state.flags);
        assert_eq!(b_state.emission, deferred_state.emission);
        assert_eq!(b_state.dampening, deferred_state.dampening);
        assert_eq!(b_state.hardness, deferred_state.hardness);
        assert_eq!(b_state.push, deferred_state.push);
        assert_eq!(a_state.flags, b_state.flags);
        assert_eq!(a_state.sturdy, b_state.sturdy);
        assert_eq!(a_state.weak, b_state.weak);
        assert_eq!(a_state.strong, b_state.strong);
        assert_eq!(a_state.fluid, b_state.fluid);
        assert_eq!(a_state.emission, b_state.emission);
        assert_eq!(a_state.dampening, b_state.dampening);
        assert_eq!(a_state.hardness, b_state.hardness);
        assert_eq!(a_state.push, b_state.push);
        assert_eq!(a_state.instrument, b_state.instrument);
        assert_eq!(a_state.shape, b_state.shape);
        let (a_sound, b_sound) = (&a.data(state).sounds, &b.data(state).sounds);
        assert_eq!(a_sound.volume, b_sound.volume);
        assert_eq!(a_sound.pitch, b_sound.pitch);
        assert_eq!(a_sound.place, b_sound.place);
        assert_eq!(a_sound.break_, b_sound.break_);
        assert_eq!(a_sound.fall, b_sound.fall);
    }
}

#[cfg(unix)]
#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn independent_reloads_read_changed_behaviour_json_and_report_invalid_files() {
    let Some(base) = prepared() else { return };
    let scratch = std::env::temp_dir().join(format!("horizon-json-reloads-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    // Keep the remaining resource tree intact without copying a huge prepared client.
    for entry in fs::read_dir(&base).unwrap() {
        let entry = entry.unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.path(), scratch.join(entry.file_name())).unwrap();
    }
    let assets = Assets::load(&base).unwrap();
    let registry = assets.registry;
    let original = fs::read(base.join("block-behaviours.json")).unwrap();
    fs::remove_file(scratch.join("block-behaviours.json")).unwrap();
    fs::write(scratch.join("block-behaviours.json"), &original).unwrap();
    // Independent facts and behaviours can load first, followed by paired data.
    let independent_first = BlockFacts::load(&scratch, registry).unwrap();
    let independent_world_first =
        horizon_world::level::Behaviours::load(registry, &scratch).unwrap();
    let (facts, paired_world) = BlockFacts::load_pair(&scratch, registry).unwrap();
    assert_eq!(facts.states.len(), registry.len());
    for state in [0, 1, registry.len() - 1] {
        let state = horizon_world::registry::BlockStateId(state as u32);
        assert_eq!(
            facts.get(state).classes,
            independent_first.get(state).classes
        );
        assert_eq!(
            facts.get(state).sounds.place,
            independent_first.get(state).sounds.place
        );
        assert_eq!(
            paired_world.facts(state).flags,
            independent_world_first.facts(state).flags
        );
    }

    // Override one exported state bit at the same path and verify that a later
    // independent and paired read sees it while the original resource root does not.
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let original_bits = changed["states"][0][1].as_u64().unwrap();
    changed["states"][0][1] = (original_bits ^ 1).into();
    fs::write(
        scratch.join("block-behaviours.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let changed_pair = BlockFacts::load_pair(&scratch, registry).unwrap();
    let changed_independent = BlockFacts::load(&scratch, registry).unwrap();
    let base_facts = BlockFacts::load(&base, registry).unwrap();
    let state = horizon_world::registry::BlockStateId(0);
    assert_ne!(
        changed_pair.0.get(state).legacy_solid,
        base_facts.get(state).legacy_solid
    );
    assert_eq!(
        changed_pair.0.get(state).legacy_solid,
        changed_independent.get(state).legacy_solid
    );
    // The world loader sees the same overridden legacy solidity flag.
    let changed_world = horizon_world::level::Behaviours::load(registry, &scratch).unwrap();
    assert_eq!(
        changed_world.facts(state).flags & 1 != 0,
        changed_pair.0.get(state).legacy_solid
    );

    fs::write(scratch.join("block-behaviours.json"), b"{").unwrap();
    let paired_error = BlockFacts::load_pair(&scratch, registry)
        .err()
        .unwrap()
        .to_string();
    let independent_error = BlockFacts::load(&scratch, registry)
        .err()
        .unwrap()
        .to_string();
    assert_eq!(paired_error, independent_error);
    assert!(horizon_world::level::Behaviours::load(registry, &scratch).is_err());

    fs::write(scratch.join("block-behaviours.json"), b"{}").unwrap();
    assert!(BlockFacts::load_pair(&scratch, registry).is_err());
    assert!(BlockFacts::load(&scratch, registry).is_err());
    assert!(horizon_world::level::Behaviours::load(registry, &scratch).is_err());

    fs::write(scratch.join("block-behaviours.json"), original).unwrap();
    assert_eq!(
        BlockFacts::load_pair(&scratch, registry)
            .unwrap()
            .0
            .states
            .len(),
        registry.len()
    );
    fs::remove_file(scratch.join("gameplay-facts.json")).unwrap();
    fs::write(scratch.join("gameplay-facts.json"), b"{\"states\":[]}").unwrap();
    assert!(BlockFacts::load_pair(&scratch, registry).is_err());
    assert!(BlockFacts::load(&scratch, registry).is_err());
    assert!(horizon_world::level::Behaviours::load(registry, &scratch).is_err());
    fs::write(scratch.join("gameplay-facts.json"), b"{").unwrap();
    assert!(BlockFacts::load_pair(&scratch, registry).is_err());
    assert!(BlockFacts::load(&scratch, registry).is_err());
    assert!(horizon_world::level::Behaviours::load(registry, &scratch).is_err());
    fs::write(
        scratch.join("gameplay-facts.json"),
        fs::read(base.join("gameplay-facts.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        BlockFacts::load_pair(&scratch, registry)
            .unwrap()
            .0
            .states
            .len(),
        registry.len()
    );
    fs::remove_dir_all(&scratch).unwrap();
}

/// A built root is kept for later starts, and built again when a mod's
/// files change.
#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn content_root_is_reused_until_a_mod_changes() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-reuse-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let m = ruby_mod(&scratch.join("ruby"));
    let cache = scratch.join("cache");
    let root = resource_root(&base, std::slice::from_ref(&m), &cache).unwrap();
    // A rebuild starts from an empty folder: this file tells them apart.
    let sentinel = root.join("sentinel");
    fs::write(&sentinel, b"").unwrap();
    assert_eq!(
        resource_root(&base, std::slice::from_ref(&m), &cache).unwrap(),
        root
    );
    assert!(sentinel.exists(), "an unchanged mod rebuilt the root");
    // A copied root with a matching stamp but without its base is rebuilt, not loaded.
    fs::remove_file(root.join("horizon-resources.json")).unwrap();
    resource_root(&base, std::slice::from_ref(&m), &cache).unwrap();
    assert!(!sentinel.exists(), "a root without its base was reused");
    assert!(root.join("horizon-resources.json").is_file());
    fs::write(&sentinel, b"").unwrap();
    let lang = scratch.join("ruby/assets/ruby/lang/en_us.json");
    fs::write(&lang, r#"{"block.ruby.ruby_block": "Ruby Block"}"#).unwrap();
    let file = fs::File::options().write(true).open(&lang).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    resource_root(&base, std::slice::from_ref(&m), &cache).unwrap();
    assert!(!sentinel.exists(), "a changed mod kept the old root");
    assert!(
        fs::read_to_string(root.join("assets/ruby/lang/en_us.json"))
            .unwrap()
            .contains("Ruby Block")
    );
    let _ = fs::remove_dir_all(&scratch);
}

/// A structure mod's worldgen reaches the worldgen loader: its structure,
/// set, pool and templates join the bundles, and its biome tag keeps only
/// what the loader knows.
#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn content_root_carries_a_structure_mod() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-worldgen-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let dir = scratch.join("huts");
    let files = [
        (
            "horizon-mod.toml",
            "id = \"huts\"\nname = \"Huts\"\nversion = \"1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"registries\"]\n",
        ),
        (
            "data/huts/worldgen/structure/hut.json",
            r##"{"type": "minecraft:jigsaw", "biomes": "#huts:has_hut", "step": "surface_structures",
                "spawn_overrides": {}, "terrain_adaptation": "beard_thin", "start_pool": "huts:hut/start",
                "size": 1, "start_height": {"absolute": 0}, "project_start_to_heightmap": "WORLD_SURFACE_WG",
                "max_distance_from_center": 80}"##,
        ),
        (
            "data/huts/worldgen/template_pool/hut/start.json",
            r#"{"fallback": "minecraft:empty", "elements": [{"weight": 1, "element": {
                "element_type": "minecraft:single_pool_element", "location": "huts:hut",
                "projection": "rigid", "processors": "minecraft:empty"}}]}"#,
        ),
        (
            "data/huts/worldgen/structure_set/huts.json",
            r#"{"structures": [{"structure": "huts:hut", "weight": 1}],
                "placement": {"type": "minecraft:random_spread", "salt": 7, "spacing": 4, "separation": 2}}"#,
        ),
        (
            "data/huts/tags/worldgen/biome/has_hut.json",
            r##"{"values": ["#minecraft:is_overworld", "#c:is_plains", "othermod:glade", "minecraft:plains"]}"##,
        ),
        // Additions to vanilla's set and pool; a target nobody defines is skipped.
        (
            "data/huts/worldgen/structure_set/added/villages.json",
            r#"{"horizon:add_to": "minecraft:villages",
                "structures": [{"structure": "huts:hut", "weight": 3}]}"#,
        ),
        (
            "data/huts/worldgen/template_pool/added/houses.json",
            r#"{"horizon:add_to": ["othermod:houses", "minecraft:village/plains/houses"],
                "elements": [{"weight": 2, "element": {"element_type": "minecraft:single_pool_element",
                "location": "huts:hut", "projection": "rigid", "processors": "minecraft:empty"}}]}"#,
        ),
    ];
    for (name, text) in files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    // The template: vanilla's smallest plains house, as the mod's own file.
    let house =
        base.join("worldgen/structure/minecraft/village/plains/houses/plains_small_house_1.nbt");
    fs::create_dir_all(dir.join("data/huts/structure")).unwrap();
    fs::copy(&house, dir.join("data/huts/structure/hut.nbt")).unwrap();
    let package = horizon_mods::Package::read(&dir).unwrap();
    let m = ContentMod {
        id: "huts".into(),
        content: Content::read(&dir, &package.manifest).unwrap(),
    };
    let root = resource_root(&base, &[m], &scratch.join("cache")).unwrap();
    assert!(root.join("worldgen/structure/huts/hut.nbt").exists());
    let tags: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("worldgen/biome_tags.json")).unwrap()).unwrap();
    let hut: serde_json::Value = serde_json::from_str(&tags["huts:has_hut"]).unwrap();
    assert_eq!(
        hut["values"],
        serde_json::json!(["#minecraft:is_overworld", "minecraft:plains"])
    );
    // The additions join vanilla's entries at the end and are no set or pool
    // of their own.
    let bundle = |name: &str, base: &Path| -> BTreeMap<String, String> {
        serde_json::from_slice(&fs::read(base.join("worldgen").join(name)).unwrap()).unwrap()
    };
    for (name, id, list, added) in [
        (
            "structure_sets.json",
            "minecraft:villages",
            "structures",
            serde_json::json!({"structure": "huts:hut", "weight": 3}),
        ),
        (
            "template_pools.json",
            "minecraft:village/plains/houses",
            "elements",
            serde_json::json!({"weight": 2, "element": {"element_type": "minecraft:single_pool_element",
                "location": "huts:hut", "projection": "rigid", "processors": "minecraft:empty"}}),
        ),
    ] {
        let (ours, vanilla) = (bundle(name, &root), bundle(name, &base));
        let entries = |text: &str| -> Vec<serde_json::Value> {
            let v: serde_json::Value = serde_json::from_str(text).unwrap();
            v[list].as_array().unwrap().clone()
        };
        let mut expected = entries(&vanilla[id]);
        expected.push(added);
        assert_eq!(entries(&ours[id]), expected, "{id}");
        assert!(!ours.keys().any(|k| k.starts_with("huts:added/")), "{name}");
        assert!(!ours.contains_key("othermod:houses"));
    }

    // The commands suggest the mod's structure, set and pool.
    let registries: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("command-registries.json")).unwrap()).unwrap();
    for (registry, id) in [
        ("minecraft:worldgen/structure", "huts:hut"),
        ("minecraft:worldgen/structure_set", "huts:huts"),
        ("minecraft:worldgen/template_pool", "huts:hut/start"),
    ] {
        let ids = registries["by_registry"][registry]["ids"]
            .as_array()
            .unwrap();
        assert!(ids.iter().any(|v| v == id), "{registry} lacks {id}");
        assert!(
            ids.iter()
                .any(|v| v.as_str().unwrap().starts_with("minecraft:"))
        );
    }

    let assets = Assets::load(&root).unwrap();
    let biomes = crate::biome::Biomes::load(&root).unwrap();
    let terrain = crate::vanillagen::VanillaTerrain::new(
        &root.join("worldgen"),
        assets.registry,
        &biomes,
        1234,
    )
    .unwrap();
    let (name, at) = terrain
        .locate(&["huts:hut".to_owned()], [0, 64, 0], 16)
        .expect("the mod's structure generates");
    assert_eq!(name, "huts:hut");
    assert!(at[0].abs() < 1024 && at[2].abs() < 1024, "{at:?}");
    let _ = fs::remove_dir_all(&scratch);
}

/// A content mod folder from `(path, text)` pairs, read as the game reads it.
fn folder_mod(dir: &Path, files: &[(&str, &str)]) -> ContentMod {
    for (name, text) in files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let package = horizon_mods::Package::read(dir).unwrap();
    ContentMod {
        id: package.manifest.id.clone(),
        content: Content::read(dir, &package.manifest).unwrap(),
    }
}

fn manifest(id: &str, namespaces: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0\"\napi = \"0.2\"\ncapabilities = [\"registries\"]\nnamespaces = [{namespaces}]\n"
    )
}

/// A mod whose files sit under namespaces besides its id (Towns and Towers'
/// `kaisyn`): they join the root, and loaders read those namespaces.
#[test]
fn listed_namespaces_join_the_root() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-ns-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let toml = manifest("towers", "\"kaisyn\"");
    let m = folder_mod(
        &scratch.join("towers"),
        &[
            ("horizon-mod.toml", &toml),
            (
                "data/kaisyn/worldgen/processor_list/mossy.json",
                r#"{"processors": []}"#,
            ),
            (
                "data/kaisyn/tags/worldgen/biome/has_tower.json",
                r#"{"values": ["minecraft:plains"]}"#,
            ),
            (
                "assets/kaisyn/lang/en_us.json",
                r#"{"block.kaisyn.x": "X"}"#,
            ),
        ],
    );
    let root = scratch.join("root");
    build(&base, &[m], &root).unwrap();
    assert_eq!(namespaces(&root), ["kaisyn", "towers"]);
    assert!(tag_namespaces(&root).contains(&"kaisyn".to_owned()));
    let lists: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("worldgen/processor_lists.json")).unwrap())
            .unwrap();
    assert!(lists.contains_key("kaisyn:mossy"));
    let tags: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("worldgen/biome_tags.json")).unwrap()).unwrap();
    assert!(tags.contains_key("kaisyn:has_tower"));
    let _ = fs::remove_dir_all(&scratch);
}

/// Two mods owning one namespace (one's listed namespace is the other's id,
/// or both list it) are refused together, naming both.
#[test]
fn mods_sharing_a_namespace_are_refused() {
    let scratch = std::env::temp_dir().join(format!("horizon-mod-clash-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let towers = folder_mod(
        &scratch.join("towers"),
        &[("horizon-mod.toml", &manifest("towers", "\"kaisyn\""))],
    );
    let kaisyn = folder_mod(
        &scratch.join("kaisyn"),
        &[("horizon-mod.toml", &manifest("kaisyn", ""))],
    );
    let other = folder_mod(
        &scratch.join("other"),
        &[("horizon-mod.toml", &manifest("other", "\"kaisyn\""))],
    );
    // The check runs before the base is read.
    let base = scratch.join("no-base");
    let out = scratch.join("root");
    let e = build(&base, &[towers, kaisyn], &out).unwrap_err();
    assert_eq!(
        e.to_string(),
        "mods towers and kaisyn both use the namespace kaisyn; turn one of them off"
    );
    let towers = folder_mod(
        &scratch.join("towers"),
        &[("horizon-mod.toml", &manifest("towers", "\"kaisyn\""))],
    );
    let e = build(&base, &[towers, other], &out).unwrap_err();
    assert!(e.to_string().starts_with("mods towers and other"), "{e}");
    let _ = fs::remove_dir_all(&scratch);
}

/// Mods merge into a tag in load order, as data packs do: each adds its
/// values, and one that replaces drops what came before it, with a warning
/// naming what it dropped.
#[test]
fn tags_merge_in_load_order_and_replace_warns() {
    let v = |ids: &[&str]| -> Vec<J> { ids.iter().map(|i| J::Str((*i).into())).collect() };
    let file = "data/minecraft/tags/worldgen/biome/has_structure/village_plains.json";
    let mut merge = TagMerge::default();
    assert_eq!(merge.add("a", file, false, true, &v(&["a:x"])), None);
    let warning = merge.add("b", file, true, true, &v(&["minecraft:plains"]));
    assert_eq!(
        warning.as_deref(),
        Some(
            "mod b replaces data/minecraft/tags/worldgen/biome/has_structure/village_plains.json: \
             vanilla's entries and those of a are left out, as a data pack's replace does; \
             the mods after it in load order still add to it"
        )
    );
    assert_eq!(merge.add("c", file, false, true, &v(&["c:y"])), None);
    assert!(merge.replaced);
    assert_eq!(merge.values, v(&["minecraft:plains", "c:y"]));
    // A second replace drops only the mods' entries.
    let warning = merge.add("d", file, true, true, &v(&["d:z"])).unwrap();
    assert!(
        warning.contains(": those of b, c are left out"),
        "{warning}"
    );
    assert_eq!(merge.values, v(&["d:z"]));
    // A mod replacing a tag of its own that nobody added to drops nothing.
    let own = "data/a/tags/item/gems.json";
    assert_eq!(
        TagMerge::default().add("a", own, true, false, &v(&["a:x"])),
        None
    );
}

/// A replacing mod's tag leaves vanilla's entries out of the tag file and of
/// the worldgen loader's biome tags, and the root keeps the warning for the
/// starts that reuse it.
#[test]
fn a_replacing_mod_tag_reaches_the_root() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch = std::env::temp_dir().join(format!("horizon-mod-replace-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let file = "data/minecraft/tags/worldgen/biome/has_structure/village_plains.json";
    let adds = folder_mod(
        &scratch.join("adds"),
        &[
            ("horizon-mod.toml", &manifest("adds", "")),
            (file, r#"{"values": ["minecraft:forest"]}"#),
        ],
    );
    let towns = folder_mod(
        &scratch.join("towns"),
        &[
            ("horizon-mod.toml", &manifest("towns", "")),
            (file, r#"{"replace": true, "values": ["minecraft:plains"]}"#),
        ],
    );
    let late = folder_mod(
        &scratch.join("late"),
        &[
            ("horizon-mod.toml", &manifest("late", "")),
            (file, r#"{"values": ["minecraft:savanna"]}"#),
        ],
    );
    let vanilla: serde_json::Value =
        serde_json::from_slice(&fs::read(base.join(file)).unwrap()).unwrap();
    assert!(vanilla["values"].as_array().unwrap().len() > 1, "{vanilla}");
    let root = resource_root(&base, &[adds, towns, late], &scratch.join("cache")).unwrap();
    let expected = serde_json::json!(["minecraft:plains", "minecraft:savanna"]);
    let tag: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(file)).unwrap()).unwrap();
    assert_eq!(tag["values"], expected);
    let tags: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(root.join("worldgen/biome_tags.json")).unwrap()).unwrap();
    let bundled: serde_json::Value =
        serde_json::from_str(&tags["minecraft:has_structure/village_plains"]).unwrap();
    assert_eq!(bundled["values"], expected);
    let warnings = fs::read_to_string(root.join(WARNINGS)).unwrap();
    assert!(
        warnings.starts_with("mod towns replaces data/minecraft/tags/worldgen/biome/has_structure/village_plains.json: vanilla's entries and those of adds are left out"),
        "{warnings}"
    );
    let _ = fs::remove_dir_all(&scratch);
}

/// A server table's lines (`id \t block \t properties`) after its header.
fn server_table(path: &Path, lines: &[&str]) {
    let mut text = format!("# horizon-oracle state ids v1\t{}\n", lines.len());
    for (id, line) in lines.iter().enumerate() {
        text.push_str(&format!("{id}\t{line}\n"));
    }
    fs::write(path, text).unwrap();
}

fn nbt_compound(entries: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        entries
            .into_iter()
            .map(|(k, v)| (JavaString::from(k), v))
            .collect(),
    )
}

/// A small base of air and stone with every kind of table the server blocks touch.
fn server_base(base: &Path) {
    let files: [(&str, &str); 12] = [
        (
            "facts.json",
            r#"{"states": [["minecraft:air", true, false, "Block{minecraft:air}", {"id": "minecraft:air"}],
                ["minecraft:stone", false, false, "Block{minecraft:stone}", {"id": "minecraft:stone"}]],
                "defaults": {"minecraft:air": 0, "minecraft:stone": 1}}"#,
        ),
        (
            "gameplay-facts.json",
            r#"{"states": [{"id": 0, "full_collision": false, "item": "minecraft:air", "outline": []},
                {"id": 1, "dampening": 15, "can_occlude": true, "replaceable": false,
                "full_collision": true, "item": "minecraft:stone",
                "outline": [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]], "outline_edges": [[0.0, 0.0, 0.0, 0.0, 0.0, 1.0]],
                "sound": {"volume": 1.0, "pitch": 1.0, "break": {"id": "minecraft:block.stone.break"}}}]}"#,
        ),
        (
            "block-behaviours.json",
            r#"{"blocks": [{"name": "minecraft:air", "item": "minecraft:air"},
                {"name": "minecraft:stone", "class": ["Block"], "item": "minecraft:stone"}],
                "states": [[0, 34, 0], [262143, 5, 0]], "items": {}}"#,
        ),
        ("items.json", r#"{"minecraft:stone": {}}"#),
        (
            "registries.json",
            r#"{"minecraft:item": {"entries": {"minecraft:stone": {"protocol_id": 1}}},
                "minecraft:block": {"entries": {"minecraft:air": {"protocol_id": 0},
                "minecraft:stone": {"protocol_id": 1}}}}"#,
        ),
        (
            "model-states.json",
            r#"{"minecraft:air": {"schema": {}, "states": [{}]}, "minecraft:stone": {"schema": {}, "states": [{}]}}"#,
        ),
        (
            "render-definitions.json",
            r#"{"minecraft:stone": {"rule": "never", "bars_tag": false, "can_occlude": true, "default": 1, "states": [1]}}"#,
        ),
        (
            "worldgen/blocks/structure-blocks.json",
            r#"{"states": 2, "rotate": [[0, 0, 0], [1, 1, 1]], "mirror": [[0, 0], [1, 1]], "full_block": [false, true]}"#,
        ),
        ("horizon-resources.json", "{}"),
        ("assets/minecraft/blockstates/stone.json", "{}"),
        (
            "assets/minecraft/models/block/cube.json",
            r#"{"elements": [{"from": [0, 0, 0], "to": [16, 16, 16]}]}"#,
        ),
        (
            "assets/minecraft/models/block/cube_all.json",
            r#"{"parent": "block/cube"}"#,
        ),
    ];
    for (name, text) in files {
        let path = base.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let state = |name: &str| nbt_compound(vec![("id", Tag::String(name.into()))]);
    let collision = nbt_compound(vec![
        (
            "states",
            Tag::List(vec![state("minecraft:air"), state("minecraft:stone")]),
        ),
        ("cached_shapes", Tag::IntArray(vec![0, 1])),
        (
            "blocks",
            nbt_compound(vec![(
                "minecraft:stone",
                nbt_compound(vec![("friction", Tag::Float(0.6f32.to_bits()))]),
            )]),
        ),
    ]);
    let named = nbt::NamedTag {
        name: "".into(),
        tag: collision,
    };
    fs::write(
        base.join("cached-collision.nbt"),
        nbt::write_named(&named, Limits::tables()).unwrap(),
    )
    .unwrap();
}

/// A mod with a declared block, and blockstates for a chair (no cube), a
/// brick (a cube through vanilla's parents) and an Every Compat chair.
fn furniture_mod(dir: &Path) -> ContentMod {
    folder_mod(
        dir,
        &[
            ("horizon-mod.toml", &manifest("furn", "\"everycomp\"")),
            (
                "data/furn/horizon/block/declared.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "assets/furn/blockstates/chair.json",
                r#"{"variants": {"facing=north": {"model": "furn:block/chair"},
                    "facing=south": {"model": "furn:block/chair", "y": 180}}}"#,
            ),
            (
                "assets/furn/models/block/chair.json",
                r#"{"elements": [{"from": [2, 0, 2], "to": [14, 10, 14]}]}"#,
            ),
            (
                "assets/furn/blockstates/brick.json",
                r#"{"variants": {"": [{"model": "furn:block/brick"}, {"model": "furn:block/brick", "y": 90}]}}"#,
            ),
            (
                "assets/furn/models/block/brick.json",
                r#"{"parent": "minecraft:block/cube_all", "textures": {"all": "furn:block/brick"}}"#,
            ),
            (
                "assets/everycomp/blockstates/hc/furn/oak_chair.json",
                r#"{"variants": {"": {"model": "furn:block/chair"}}}"#,
            ),
        ],
    )
}

const FURNITURE_TABLE: [&str; 10] = [
    "minecraft:air\t",
    "minecraft:stone\t",
    "furn:chair\tfacing=north,waterlogged=true",
    "furn:chair\tfacing=north,waterlogged=false",
    "furn:chair\tfacing=south,waterlogged=true",
    "furn:chair\tfacing=south,waterlogged=false",
    "everycomp:hc/furn/oak_chair\tfacing=north",
    "furn:brick\t",
    "furn:declared\t",
    "nobody:thing\t",
];

/// The server's undeclared blocks with a blockstate in the mods join after
/// every other id with the server's exact states; Every Compat's, declared
/// ones and ones without a blockstate do not.
#[test]
fn server_blocks_join_with_the_servers_exact_states_after_every_other_id() {
    let scratch = std::env::temp_dir().join(format!("horizon-mod-server-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let base = scratch.join("base");
    server_base(&base);
    let mods = vec![furniture_mod(&scratch.join("furn"))];
    let table = scratch.join("server-states.tsv");
    server_table(&table, &FURNITURE_TABLE);

    // Vanilla's and the declared ids are the same with the table as without.
    let without = Plan::new(&base, &mods, None).unwrap();
    let plan = Plan::new(&base, &mods, Some(table.as_path())).unwrap();
    assert!(without.server.is_empty());
    let declared = |plan: &Plan| -> Vec<(String, u32)> {
        let blocks = plan.blocks.iter();
        blocks
            .map(|b| (b.decl.id.clone(), b.rename.first))
            .collect()
    };
    assert_eq!(declared(&plan), declared(&without));
    assert_eq!(declared(&plan), [("furn:declared".to_owned(), 2)]);
    let server: Vec<_> = plan
        .server
        .iter()
        .map(|b| (b.name.as_str(), b.first, b.states.len(), b.cube))
        .collect();
    assert_eq!(
        server,
        [("furn:brick", 3, 1, true), ("furn:chair", 4, 4, false)]
    );
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    // The streaming writer must match the in-memory representation,
    // including field order and the first occurrence of a repeated key.
    let mut doc = read_json(&base.join("gameplay-facts.json")).unwrap();
    let J::Obj(fields) = &mut doc else {
        panic!("gameplay document must be an object");
    };
    fields.insert(0, ("metadata".into(), J::Str("test\nvalue".into())));
    fields.push(("states".into(), J::Arr(Vec::new())));
    let mut expected = doc.clone();
    let Some(J::Arr(rows)) = expected.at("states") else {
        panic!("gameplay document must have a states array");
    };
    plan.states(rows, false, Rows::Gameplay).unwrap();
    let mut streamed = Vec::new();
    write_gameplay_table(&plan, &doc, &mut streamed).unwrap();
    assert_eq!(streamed, serde_json::to_vec(&expected).unwrap());
    drop((plan, without));

    let out = scratch.join("out");
    build_with_server(&base, &mods, Some(table.as_path()), &out).unwrap();
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&fs::read(out.join(name)).unwrap()).unwrap()
    };
    let facts = read("facts.json");
    let rows = facts["states"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert_eq!(
        rows[3],
        serde_json::json!(["furn:brick", false, false, "Block{furn:brick}", {"id": "furn:brick"}])
    );
    assert_eq!(
        rows[4],
        serde_json::json!(["furn:chair", false, false,
            "Block{furn:chair}[facing=north,waterlogged=true]",
            {"properties": {"facing": "north", "waterlogged": "true"}, "id": "furn:chair"}])
    );
    assert_eq!(facts["defaults"]["furn:chair"], 4);
    assert!(rows.iter().all(|r| {
        let name = r[0].as_str().unwrap();
        !name.starts_with("everycomp:") && name != "nobody:thing"
    }));
    // The registry, and so game::server_states, resolves the server's states exactly.
    let registry = horizon_world::registry::BlockRegistry::from_facts_json(
        fs::File::open(out.join("facts.json")).unwrap(),
    )
    .unwrap();
    let state = |name: &str, properties: &[(&str, &str)]| horizon_world::chunk::BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let chair = state(
        "furn:chair",
        &[("facing", "south"), ("waterlogged", "false")],
    );
    assert_eq!(registry.resolve(&chair).unwrap(), BlockStateId(7));
    assert_eq!(
        registry.resolve(&state("furn:brick", &[])).unwrap(),
        BlockStateId(3)
    );

    let gameplay = read("gameplay-facts.json");
    let rows = gameplay["states"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    for (id, row) in rows.iter().enumerate() {
        assert_eq!(row["id"], id, "{row}");
    }
    // Compact: what the readers default is left out. The brick collides
    // and hides what is behind it; the chair does neither.
    let brick = &rows[3];
    assert_eq!(brick["full_collision"], true);
    assert_eq!(brick["can_occlude"], true);
    assert_eq!(brick["item"], "minecraft:air");
    assert!(brick.get("outline").is_none() && brick.get("replaceable").is_none());
    assert_eq!(
        brick["sound"],
        serde_json::json!({"break": {"id": "minecraft:block.stone.break"}})
    );
    let chair_row = &rows[4];
    assert!(chair_row.get("full_collision").is_none() && chair_row.get("can_occlude").is_none());
    assert_eq!(chair_row["dampening"], 0);
    assert_eq!(chair_row["skylight_down"], true);
    // A declared block's rows stay full copies.
    assert_eq!(
        rows[2]["outline"],
        serde_json::json!([[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]])
    );

    let behaviours = read("block-behaviours.json");
    let names: Vec<&str> = behaviours["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "minecraft:air",
            "minecraft:stone",
            "furn:declared",
            "furn:brick",
            "furn:chair"
        ]
    );
    let states = behaviours["states"].as_array().unwrap();
    assert_eq!(states.len(), 8);
    assert_eq!(states[3], serde_json::json!([262143, 5, 0]));
    assert_eq!(
        states[4],
        serde_json::json!([
            0,
            horizon_world::level::data::state_flags::COLLISION_EMPTY,
            0
        ])
    );

    let models = read("model-states.json");
    assert_eq!(
        models["furn:chair"]["schema"],
        serde_json::json!({"facing": {"named": ["north", "south"]},
            "waterlogged": {"named": ["true", "false"]}})
    );
    assert_eq!(
        models["furn:chair"]["states"][3],
        serde_json::json!({"facing": "south", "waterlogged": "false"})
    );
    let render = read("render-definitions.json");
    assert_eq!(
        render["furn:chair"],
        serde_json::json!({"rule": "never", "bars_tag": false, "can_occlude": false,
            "default": 4, "states": [4, 5, 6, 7]})
    );
    assert_eq!(render["furn:brick"]["can_occlude"], true);
    let structure = read("worldgen/blocks/structure-blocks.json");
    assert_eq!(structure["states"], 8);
    assert_eq!(structure["rotate"][5], serde_json::json!([5, 5, 5]));
    assert_eq!(structure["mirror"][3], serde_json::json!([3, 3]));
    assert_eq!(structure["full_block"][3], true);
    assert_eq!(structure["full_block"][4], false);
    let registries = read("registries.json");
    let block_id =
        |name: &str| registries["minecraft:block"]["entries"][name]["protocol_id"].clone();
    assert_eq!(block_id("furn:declared"), 2);
    assert_eq!(block_id("furn:brick"), 3);
    assert_eq!(block_id("furn:chair"), 4);

    // NBT tables: exact identities, the brick's template shape, the chair's air shape.
    let (collision, _) = nbt::read_named(
        &fs::read(out.join("cached-collision.nbt")).unwrap(),
        Limits::tables(),
    )
    .unwrap();
    let Some(Tag::List(identities)) = collision.tag.get("states") else {
        panic!("no states");
    };
    let identities: Vec<_> = identities
        .iter()
        .map(|t| horizon_world::chunk::BlockState::from_nbt(t).unwrap())
        .collect();
    assert_eq!(identities[7], chair);
    assert_eq!(identities[3], state("furn:brick", &[]));
    assert!(matches!(
        collision.tag.get("cached_shapes"),
        Some(Tag::IntArray(shapes)) if shapes == &[0, 1, 1, 1, 0, 0, 0, 0]
    ));
    let Some(Tag::Compound(blocks)) = collision.tag.get("blocks") else {
        panic!("no blocks");
    };
    assert!(blocks.contains_key(&JavaString::from("furn:chair")));

    // The table is part of what the root is built from.
    let with_table = stamp(&base, &mods, Some(table.as_path())).unwrap();
    assert_ne!(stamp(&base, &mods, None).unwrap(), with_table);
    server_table(&table, &FURNITURE_TABLE[..9]);
    assert_ne!(
        stamp(&base, &mods, Some(table.as_path())).unwrap(),
        with_table
    );
    fs::remove_dir_all(&scratch).unwrap();
}

/// A namespace whose server states would pass the cap is left out with a
/// warning; the others still get ids, after the existing ones.
#[test]
fn server_blocks_past_the_cap_leave_their_namespace_out() {
    let block = |name: &str, states: usize| server::ServerBlock {
        name: name.into(),
        states: vec![String::new(); states],
        first: 0,
        cube: true,
        opaque: true,
        donors: Vec::new(),
    };
    let blocks = vec![
        block("a:x", 3),
        block("b:y", 4),
        block("b:z", 1),
        block("c:w", 2),
    ];
    let mut warnings = Vec::new();
    let placed = server::place(blocks, 10, 16, &mut warnings);
    let ids: Vec<_> = placed.iter().map(|b| (b.name.as_str(), b.first)).collect();
    assert_eq!(ids, [("a:x", 10), ("c:w", 13)]);
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("server's b blocks are left out: their 5 states"),
        "{}",
        warnings[0]
    );
}

/// Every loader reads a root with server blocks over the real prepared
/// resources: the NBT tables' identities, the gameplay and behaviour
/// lengths and block order, and the model catalog's states.
#[test]
#[ignore = "full content root: CI and pr_ready run it"]
fn server_blocks_load_with_every_table() {
    let Some(base) = prepared() else {
        return;
    };
    let scratch =
        std::env::temp_dir().join(format!("horizon-mod-server-root-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let dir = scratch.join("ruby");
    let assets_files = [
        (
            "assets/ruby/blockstates/ruby_chair.json",
            r#"{"variants": {"facing=north": {"model": "ruby:block/ruby_chair"},
                "facing=south": {"model": "ruby:block/ruby_chair", "y": 180}}}"#,
        ),
        (
            "assets/ruby/models/block/ruby_chair.json",
            r##"{"textures": {"all": "minecraft:block/oak_planks"}, "elements": [{"from": [2, 0, 2],
                "to": [14, 10, 14], "faces": {"up": {"texture": "#all"}}}]}"##,
        ),
        (
            "assets/ruby/blockstates/ruby_bricks.json",
            r#"{"variants": {"": {"model": "minecraft:block/bricks"}}}"#,
        ),
    ];
    for (name, text) in assets_files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let m = ruby_mod(&dir);
    let table = scratch.join("server-states.tsv");
    server_table(
        &table,
        &[
            "minecraft:air\t",
            "ruby:ruby_chair\tfacing=north,waterlogged=false",
            "ruby:ruby_chair\tfacing=north,waterlogged=true",
            "ruby:ruby_chair\tfacing=south,waterlogged=false",
            "ruby:ruby_chair\tfacing=south,waterlogged=true",
            "ruby:ruby_bricks\t",
            "ruby:ruby_block\t",
        ],
    );
    let out = scratch.join("out");
    build_with_server(&base, std::slice::from_ref(&m), Some(table.as_path()), &out).unwrap();

    let n = horizon_world::registry::BlockRegistry::from_facts_json(
        fs::File::open(base.join("facts.json")).unwrap(),
    )
    .unwrap()
    .len() as u32;
    let assets = Assets::load(&out).unwrap();
    let registry = assets.registry;
    // After ruby's 66 declared states: the bricks, then the chair's four.
    let bricks = BlockStateId(n + 66);
    assert_eq!(registry.default_state("ruby:ruby_bricks"), Some(bricks));
    let chair = registry
        .resolve(&horizon_world::chunk::BlockState {
            name: "ruby:ruby_chair".into(),
            properties: [("facing", "south"), ("waterlogged", "true")]
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .into(),
        })
        .unwrap();
    assert_eq!(chair, BlockStateId(n + 70));
    assert_eq!(registry.len() as u32, n + 71);

    let facts = BlockFacts::load(&out, registry).unwrap();
    let behaviours = horizon_world::level::Behaviours::load(registry, &out).unwrap();
    use horizon_world::level::data::state_flags::{COLLISION_EMPTY, FULL_COLLISION};
    assert_ne!(behaviours.facts(bricks).flags & FULL_COLLISION, 0);
    assert_eq!(behaviours.facts(chair).flags & FULL_COLLISION, 0);
    assert_ne!(behaviours.facts(chair).flags & COLLISION_EMPTY, 0);
    Items::load(&out, registry, &facts).unwrap();
    crate::collisions::load(&out, registry).unwrap();
    crate::models::load_visibility(&out, registry).unwrap();
    crate::light::LightTables::load(&out, registry, &facts).unwrap();
    crate::motion::MotionResources::load(&out, &assets).unwrap();
    horizon_world::worldgen::generator::GeneratorData::load(&out.join("worldgen"), registry)
        .unwrap();
    // Each binds its own blockstate over the server's states.
    let mut sprites = crate::sprites::Sprites::default();
    let prepared = crate::models::prepare(&out, registry, &mut sprites).unwrap();
    let position = horizon_core::position::BlockPos::new(0, 0, 0);
    assert_eq!(prepared.catalog.collect(chair, position).unwrap().len(), 1);
    assert_eq!(prepared.catalog.collect(bricks, position).unwrap().len(), 1);
    fs::remove_dir_all(&scratch).unwrap();
}

/// A block or item tag entry naming a vanilla tag the base lacks is dropped
/// (1.21.1's `concrete_powder`); one the base has stays, and a base without
/// that kind's folder keeps every `minecraft:` tag as before.
#[test]
fn tag_entries_naming_missing_vanilla_tags_are_dropped() {
    let dir = std::env::temp_dir().join(format!("horizon-tag-paths-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let block_tags = dir.join("data/minecraft/tags/block");
    fs::create_dir_all(block_tags.join("mineable")).unwrap();
    fs::write(block_tags.join("mineable/pickaxe.json"), "{}").unwrap();
    fs::write(block_tags.join("logs.json"), "{}").unwrap();
    fs::write(block_tags.join("notes.txt"), "").unwrap();
    let mut paths = std::collections::BTreeSet::new();
    tag_paths(&block_tags, "", &mut paths).unwrap();
    assert_eq!(
        paths.iter().map(String::as_str).collect::<Vec<_>>(),
        ["logs", "mineable/pickaxe"]
    );

    let known = |vanilla_tags: BTreeMap<&'static str, std::collections::BTreeSet<String>>| Known {
        blocks: ["minecraft:stone".to_owned()].into(),
        items: Default::default(),
        biomes: Default::default(),
        biome_tags: Default::default(),
        tag_files: ["data/c/tags/block/storage_blocks/ruby.json".to_owned()].into(),
        vanilla_tags,
    };
    let file = "data/minecraft/tags/block/mineable/pickaxe.json";
    let entry = |s: &str| J::Str(s.into());
    let with_base = known(BTreeMap::from([("block", paths)]));
    assert!(with_base.keeps(file, &entry("#minecraft:mineable/pickaxe")));
    assert!(with_base.keeps(file, &entry("#logs")));
    assert!(with_base.keeps(file, &entry("#c:storage_blocks/ruby")));
    assert!(with_base.keeps(file, &entry("minecraft:stone")));
    assert!(!with_base.keeps(file, &entry("#minecraft:concrete_powder")));
    assert!(!with_base.keeps(file, &entry("#c:not_added")));
    // Item tags have no folder in this base: their vanilla tags all stay.
    let item_file = "data/minecraft/tags/item/gems.json";
    assert!(with_base.keeps(item_file, &entry("#minecraft:concrete_powder")));
    let without_base = known(BTreeMap::new());
    assert!(without_base.keeps(file, &entry("#minecraft:concrete_powder")));

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn declared_non_cube_and_cutout_blocks_do_not_occlude_neighbors() {
    let scratch =
        std::env::temp_dir().join(format!("horizon-mod-declared-occlude-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let base = scratch.join("base");
    server_base(&base);
    // Copy cached-collision.nbt shape table layout [0, 1] into occlusion.nbt as well.
    fs::copy(base.join("cached-collision.nbt"), base.join("occlusion.nbt")).unwrap();

    let m = folder_mod(
        &scratch.join("bwg"),
        &[
            ("horizon-mod.toml", &manifest("bwg", "")),
            (
                "data/bwg/horizon/block/clover_patch.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "data/bwg/horizon/block/glass_bush.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "data/bwg/horizon/block/marble.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "data/bwg/horizon/block/depot.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "data/bwg/horizon/block/petals.json",
                r#"{"template": "minecraft:stone"}"#,
            ),
            (
                "assets/bwg/blockstates/petals.json",
                r#"{"variants": {"": [{"model": "bwg:block/petals"}, {"model": "bwg:block/petals", "y": 90}]}}"#,
            ),
            (
                "assets/bwg/models/block/petals.json",
                r#"{"render_type": "minecraft:cutout", "elements": [{"from": [0, 2, 0], "to": [16, 2, 16]}, {"from": [8, 0, 0], "to": [8, 6, 16]}]}"#,
            ),
            (
                "assets/bwg/blockstates/depot.json",
                r#"{"variants": {"": {"model": "bwg:block/depot"}}}"#,
            ),
            (
                "assets/bwg/models/block/depot.json",
                r#"{"parent": "minecraft:block/block", "elements": [{"from": [1, 0, 1], "to": [15, 13, 15]}]}"#,
            ),
            (
                "assets/bwg/blockstates/clover_patch.json",
                r#"{"variants": {"": {"model": "bwg:block/clover_patch"}}}"#,
            ),
            (
                "assets/bwg/models/block/clover_patch.json",
                r#"{"parent": "minecraft:block/cross", "render_type": "minecraft:cutout"}"#,
            ),
            (
                "assets/bwg/blockstates/glass_bush.json",
                r#"{"variants": {"": {"model": "bwg:block/glass_bush"}}}"#,
            ),
            (
                "assets/bwg/models/block/glass_bush.json",
                r#"{"parent": "minecraft:block/cube_all", "render_type": "minecraft:cutout"}"#,
            ),
            (
                "assets/bwg/blockstates/marble.json",
                r#"{"variants": {"": {"model": "bwg:block/marble"}}}"#,
            ),
            (
                "assets/bwg/models/block/marble.json",
                r#"{"parent": "minecraft:block/cube_all"}"#,
            ),
        ],
    );
    let out = scratch.join("out");
    build_with_server(&base, &[m], None, &out).unwrap();
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&fs::read(out.join(name)).unwrap()).unwrap()
    };

    let render = read("render-definitions.json");
    assert_eq!(render["bwg:clover_patch"]["can_occlude"], false);
    assert_eq!(render["bwg:glass_bush"]["can_occlude"], false);
    assert_eq!(render["bwg:marble"]["can_occlude"], true);

    let gameplay = read("gameplay-facts.json");
    let rows = gameplay["states"].as_array().unwrap();
    let facts = read("facts.json");
    let clover_id = facts["defaults"]["bwg:clover_patch"].as_u64().unwrap() as usize;
    let bush_id = facts["defaults"]["bwg:glass_bush"].as_u64().unwrap() as usize;
    let marble_id = facts["defaults"]["bwg:marble"].as_u64().unwrap() as usize;
    let depot_id = facts["defaults"]["bwg:depot"].as_u64().unwrap() as usize;
    let petals_id = facts["defaults"]["bwg:petals"].as_u64().unwrap() as usize;
    assert_eq!(render["bwg:depot"]["can_occlude"], false);
    // Flat planes only (a petal patch): walked through like a cross plant.
    assert_eq!(rows[petals_id]["full_collision"], false);
    assert_eq!(rows[clover_id]["can_occlude"], false);
    assert_eq!(rows[clover_id]["full_collision"], false);
    // A see-through cube (leaves, glass, ice) and a shape that is no plant
    // stay solid like the server's: only occlusion is dropped.
    assert_eq!(rows[bush_id]["can_occlude"], false);
    assert_eq!(rows[bush_id]["full_collision"], true);
    assert_eq!(rows[depot_id]["can_occlude"], false);
    assert_eq!(rows[depot_id]["full_collision"], true);
    assert_eq!(rows[marble_id]["can_occlude"], true);
    assert_eq!(rows[marble_id]["full_collision"], true);

    let (occ, _) =
        nbt::read_named(&fs::read(out.join("occlusion.nbt")).unwrap(), Limits::tables()).unwrap();
    let Some(Tag::IntArray(occ_shapes)) = occ.tag.get("cached_shapes") else {
        panic!("no occlusion shapes");
    };
    assert_eq!(occ_shapes[clover_id], occ_shapes[0]);
    assert_eq!(occ_shapes[bush_id], occ_shapes[0]);
    assert_eq!(occ_shapes[depot_id], occ_shapes[0]);
    assert_eq!(occ_shapes[marble_id], occ_shapes[1]);

    let (col, _) = nbt::read_named(
        &fs::read(out.join("cached-collision.nbt")).unwrap(),
        Limits::tables(),
    )
    .unwrap();
    let Some(Tag::IntArray(col_shapes)) = col.tag.get("cached_shapes") else {
        panic!("no collision shapes");
    };
    assert_eq!(col_shapes[clover_id], col_shapes[0]);
    assert_eq!(col_shapes[bush_id], col_shapes[1]);
    assert_eq!(col_shapes[petals_id], col_shapes[0]);
    assert_eq!(col_shapes[depot_id], col_shapes[1]);
    assert_eq!(col_shapes[marble_id], col_shapes[1]);

    fs::remove_dir_all(&scratch).unwrap();
}

/// A full-cube template's occlusion goes only to a model that hides as much;
/// slabs and stairs keep theirs unless the model is see-through.
#[test]
fn template_occlusion_passes_only_to_models_that_hide_as_much() {
    let base = std::env::temp_dir().join(format!(
        "horizon-template-occlusion-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let files = [
        (
            "assets/minecraft/models/block/cube.json",
            r#"{"elements": [{"from": [0, 0, 0], "to": [16, 16, 16]}]}"#,
        ),
        (
            "assets/minecraft/models/block/cube_all.json",
            r#"{"parent": "minecraft:block/cube"}"#,
        ),
        (
            "assets/minecraft/blockstates/furnace.json",
            r#"{"variants": {"lit=false": {"model": "minecraft:block/furnace"}}}"#,
        ),
        (
            "assets/minecraft/models/block/furnace.json",
            r#"{"parent": "minecraft:block/cube_all"}"#,
        ),
        (
            "assets/minecraft/blockstates/oak_stairs.json",
            r#"{"variants": {"half=bottom": {"model": "minecraft:block/stairs"}}}"#,
        ),
        (
            "assets/minecraft/models/block/stairs.json",
            r#"{"elements": [{"from": [0, 0, 0], "to": [16, 8, 16]}, {"from": [8, 8, 0], "to": [16, 16, 16]}]}"#,
        ),
        (
            "assets/m/blockstates/wall_torch.json",
            r#"{"variants": {"lit=false": {"model": "m:block/wall_torch"}}}"#,
        ),
        (
            "assets/m/models/block/wall_torch.json",
            r#"{"render_type": "minecraft:cutout", "elements": [{"from": [7, 3, 12], "to": [9, 13, 16]}]}"#,
        ),
        (
            "assets/m/blockstates/kiln.json",
            r#"{"variants": {"lit=false": {"model": "m:block/kiln"}}}"#,
        ),
        (
            "assets/m/models/block/kiln.json",
            r#"{"parent": "minecraft:block/cube_all"}"#,
        ),
        (
            "assets/m/blockstates/brick_stairs.json",
            r#"{"variants": {"half=bottom": {"model": "m:block/brick_stairs"}}}"#,
        ),
        (
            "assets/m/models/block/brick_stairs.json",
            r#"{"parent": "minecraft:block/stairs"}"#,
        ),
        (
            "assets/m/blockstates/glass_stairs.json",
            r#"{"variants": {"half=bottom": {"model": "m:block/glass_stairs"}}}"#,
        ),
        (
            "assets/m/models/block/glass_stairs.json",
            r#"{"parent": "minecraft:block/stairs", "render_type": "minecraft:translucent"}"#,
        ),
    ];
    for (name, text) in files {
        let path = base.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let mut assets = server::ModAssets::new(&base, &[]);
    let keeps = |assets: &mut server::ModAssets<'_>, id: &str, template: &str| {
        keeps_template_occlusion(assets, id, template, false)
    };
    assert!(keeps(&mut assets, "m:kiln", "minecraft:furnace"));
    assert!(!keeps(&mut assets, "m:wall_torch", "minecraft:furnace"));
    assert!(keeps(&mut assets, "m:brick_stairs", "minecraft:oak_stairs"));
    assert!(!keeps(&mut assets, "m:glass_stairs", "minecraft:oak_stairs"));
    drop(assets);
    fs::remove_dir_all(&base).unwrap();
}

/// A see-through cube the server sends (glass, ice, leaves) keeps its
/// collision but occludes nothing; an opaque one keeps both.
#[test]
fn server_see_through_cubes_collide_but_do_not_occlude() {
    let scratch =
        std::env::temp_dir().join(format!("horizon-mod-server-glass-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let base = scratch.join("base");
    server_base(&base);
    fs::copy(base.join("cached-collision.nbt"), base.join("occlusion.nbt")).unwrap();
    let m = folder_mod(
        &scratch.join("glassy"),
        &[
            ("horizon-mod.toml", &manifest("glassy", "")),
            (
                "assets/glassy/blockstates/glass.json",
                r#"{"variants": {"": {"model": "glassy:block/glass"}}}"#,
            ),
            (
                "assets/glassy/models/block/glass.json",
                r#"{"parent": "minecraft:block/cube_all", "render_type": "minecraft:translucent"}"#,
            ),
            (
                "assets/glassy/blockstates/rock.json",
                r#"{"variants": {"": {"model": "glassy:block/rock"}}}"#,
            ),
            (
                "assets/glassy/models/block/rock.json",
                r#"{"parent": "minecraft:block/cube_all"}"#,
            ),
        ],
    );
    let table = scratch.join("server-states.tsv");
    server_table(
        &table,
        &[
            "minecraft:air\t",
            "minecraft:stone\t",
            "glassy:glass\t",
            "glassy:rock\t",
        ],
    );
    let plan = Plan::new(&base, std::slice::from_ref(&m), Some(table.as_path())).unwrap();
    let flags: Vec<_> = plan
        .server
        .iter()
        .map(|b| (b.name.as_str(), b.cube, b.opaque))
        .collect();
    assert_eq!(
        flags,
        [("glassy:glass", true, false), ("glassy:rock", true, true)]
    );
    let id = |name: &str| plan.server.iter().find(|b| b.name == name).unwrap().first as usize;
    let (glass, rock) = (id("glassy:glass"), id("glassy:rock"));
    drop(plan);

    let out = scratch.join("out");
    build_with_server(&base, &[m], Some(table.as_path()), &out).unwrap();
    let gameplay: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("gameplay-facts.json")).unwrap()).unwrap();
    let rows = gameplay["states"].as_array().unwrap();
    assert_eq!(rows[glass]["full_collision"], true);
    assert_ne!(rows[glass]["can_occlude"], true);
    assert_eq!(rows[rock]["full_collision"], true);
    assert_eq!(rows[rock]["can_occlude"], true);
    let shapes = |name: &str| {
        let (tag, _) =
            nbt::read_named(&fs::read(out.join(name)).unwrap(), Limits::tables()).unwrap();
        let Some(Tag::IntArray(shapes)) = tag.tag.get("cached_shapes") else {
            panic!("no shapes in {name}");
        };
        shapes.clone()
    };
    let (occlusion, collision) = (shapes("occlusion.nbt"), shapes("cached-collision.nbt"));
    assert_eq!(occlusion[glass], occlusion[0]);
    assert_eq!(collision[glass], collision[1]);
    assert_eq!(occlusion[rock], occlusion[1]);
    assert_eq!(collision[rock], collision[1]);
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn overlay_write_and_batch_do_not_mutate_base_files() {
    let scratch = std::env::temp_dir().join(format!("horizon-overlay-batch-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let base = scratch.join("base");
    let out = scratch.join("out");
    fs::create_dir_all(base.join("assets/minecraft")).unwrap();
    fs::write(base.join("assets/minecraft/a.txt"), b"base-a").unwrap();
    fs::write(base.join("assets/minecraft/b.txt"), b"base-b").unwrap();
    fs::create_dir_all(out.join(".horizon-derived")).unwrap();
    fs::write(out.join(".horizon-derived/keep.txt"), b"keep").unwrap();

    let overlay = Overlay::new(&base, &out).unwrap();
    overlay.write("assets/minecraft/a.txt", b"mod-a").unwrap();
    overlay
        .write_batch(
            &[
                ("assets/minecraft/b.txt", b"mod-b"),
                ("assets/mod/new/c.txt", b"mod-c"),
            ],
            2,
        )
        .unwrap();

    assert_eq!(fs::read(base.join("assets/minecraft/a.txt")).unwrap(), b"base-a");
    assert_eq!(fs::read(base.join("assets/minecraft/b.txt")).unwrap(), b"base-b");
    assert_eq!(fs::read(out.join("assets/minecraft/a.txt")).unwrap(), b"mod-a");
    assert_eq!(fs::read(out.join("assets/minecraft/b.txt")).unwrap(), b"mod-b");
    assert_eq!(fs::read(out.join("assets/mod/new/c.txt")).unwrap(), b"mod-c");
    assert_eq!(fs::read(out.join(".horizon-derived/keep.txt")).unwrap(), b"keep");

    fs::remove_dir_all(&scratch).unwrap();
}
