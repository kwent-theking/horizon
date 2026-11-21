use super::*;

#[test]
fn binary_registry_preserves_order_defaults_and_validates_descriptors() {
    let state = |name: &str| BlockState {
        name: name.into(),
        properties: BTreeMap::new(),
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
