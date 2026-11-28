use super::*;

#[test]
fn registry_interning_preserves_properties_and_copy_on_write() {
    let state = |name: &str, facing: &str| BlockState {
        name: name.to_owned(),
        properties: [
            ("waterlogged".to_owned(), "false".to_owned()),
            ("facing".to_owned(), facing.to_owned()),
        ]
        .into(),
    };
    let descriptors = vec![
        state("test:first", "north"),
        state("test:first", "south"),
        state("test:second", "north"),
    ];
    let registry = BlockRegistry::from_ordered_states(
        descriptors.clone(),
        BTreeMap::from([("test:first".into(), 0), ("test:second".into(), 2)]),
    )
    .unwrap();
    for (i, descriptor) in descriptors.iter().enumerate() {
        let id = BlockStateId(i as u32);
        let restored = registry.state(id).unwrap();
        assert_eq!(restored, descriptor);
        assert_eq!(restored.to_nbt(), descriptor.to_nbt());
        assert_eq!(registry.id(descriptor), Some(id));
    }
    let properties = |id| &registry.state(BlockStateId(id)).unwrap().properties;
    let legacy_states: Vec<_> = descriptors
        .iter()
        .map(|state| {
            (
                &state.name,
                state.properties.iter().collect::<BTreeMap<_, _>>(),
            )
        })
        .collect();
    let defaults = BTreeMap::from([("test:first", 0i64), ("test:second", 2)]);
    let old_bytes = postcard::to_allocvec(&(legacy_states, defaults)).unwrap();
    assert_eq!(postcard::to_allocvec(&registry).unwrap(), old_bytes);
    let first = properties(0);
    let second = properties(1);
    let third = properties(2);
    assert!(std::ptr::eq(
        first.get("facing").unwrap(),
        third.get("facing").unwrap()
    ));
    assert!(std::ptr::eq(
        first.keys().next().unwrap(),
        second.keys().next().unwrap()
    ));
    assert!(std::ptr::eq(
        first.get("waterlogged").unwrap(),
        second.get("waterlogged").unwrap()
    ));
    assert!(first.shares_storage(third));

    let mut changed = registry.state(BlockStateId(0)).unwrap().clone();
    *changed.properties.get_mut("facing").unwrap() = "south".into();
    assert_eq!(registry.id(&changed), Some(BlockStateId(1)));
    assert_eq!(first["facing"], "north");
    assert_eq!(third["facing"], "north");
}

#[test]
fn binary_registry_preserves_order_defaults_and_validates_descriptors() {
    let state = |name: &str| BlockState {
        name: name.into(),
        properties: Default::default(),
    };
    let registry = BlockRegistry::from_ordered_states(
        vec![state("test:second"), state("test:first")],
        BTreeMap::from([("test:second".into(), 0), ("test:first".into(), 1)]),
    )
    .unwrap();
    let bytes = postcard::to_allocvec(&registry).unwrap();
    let restored: BlockRegistry = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(
        restored.state(BlockStateId(0)),
        registry.state(BlockStateId(0))
    );
    assert_eq!(restored.default_state("test:first"), Some(BlockStateId(1)));
    assert_eq!(restored.id(&state("test:second")), Some(BlockStateId(0)));
    assert_eq!(restored.cache_identity(), registry.cache_identity());
    for (states, defaults) in [
        (
            vec![state("test:a"), state("test:a")],
            BTreeMap::from([("test:a".into(), 0i64)]),
        ),
        (
            vec![state("test:a")],
            BTreeMap::from([("test:a".into(), 2)]),
        ),
        (vec![state("test:a")], BTreeMap::<&str, i64>::new()),
    ] {
        let bytes = postcard::to_allocvec(&(states, defaults)).unwrap();
        assert!(postcard::from_bytes::<BlockRegistry>(&bytes).is_err());
    }
}
