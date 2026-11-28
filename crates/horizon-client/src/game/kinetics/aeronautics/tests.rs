use super::*;

fn tag(snbt: &str) -> Tag {
    horizon_world::nbt::parse_snbt(snbt).unwrap().0
}

fn pose(block: &str, props: &[(&str, &str)], snbt: &str, time: f64) -> Vec<MachinePart> {
    let kind = Part::for_block(block).unwrap();
    let get = |key: &str| props.iter().find(|(name, _)| *name == key).map(|p| p.1);
    let tag = tag(snbt);
    assert!(super::super::is_kinetic_block(block));
    kind.parts(
        get,
        speed_from_nbt(&tag).unwrap(),
        kind.read_pose(None, &tag, 0, speed_from_nbt(&tag).unwrap()),
        time,
        [0, 1, 0],
    )
    .into_iter()
    .collect()
}

#[test]
fn pose_only_updates_are_accepted_for_their_block_entity_types() {
    for (kind, snbt) in [
        (WOODEN, "{RotationSpeed:19.199995f}"),
        (ANDESITE, "{RotationSpeed:19.199995f}"),
        (SMART, "{RotationSpeed:19.199995f}"),
        ("simulated:simple", "{ExtraCogwheel:{Speed:32f}}"),
        (SWIVEL, "{SwivelCog:{Speed:0f}}"),
        (SPRING, "{TorsionSpringOutput:{Angle:0f}}"),
        (BOREHEAD, "{Running:0b,Angle:0f,RotationSpeed:0f}"),
        (BEARING, "{Running:0b,Angle:0f,RotationSpeed:0f}"),
    ] {
        let tag = tag(snbt);
        assert!(super::super::is_kinetic_block_entity(kind));
        assert!(super::super::is_create_aux_block_entity_tag(kind, &tag));
        assert!(!super::super::is_create_aux_block_entity_tag(
            "create:shaft",
            &tag
        ));
    }
}

fn point(part: &MachinePart, from: Vec3, to: Vec3) {
    assert!(
        part.local.transform_point3(from).abs_diff_eq(to, 1e-5),
        "{}: {:?} != {to:?}",
        part.model,
        part.local.transform_point3(from),
    );
}

#[test]
fn small_propellers_follow_their_mount_and_reported_blade_speed() {
    // airpressure/miniprop.nbt records 64 rpm and 19.199995 degrees/tick.
    let tag = "{Speed:64f,RotationSpeed:19.199995f}";
    let time = 90.0 / 19.199995_f32 as f64;
    let center = Vec3::splat(0.5);
    for (block, model) in [
        (
            WOODEN,
            "aeronautics:block/wooden_propeller/propeller_reversed",
        ),
        (
            ANDESITE,
            "aeronautics:block/andesite_propeller/propeller_reversed",
        ),
    ] {
        let rest = pose(block, &[("facing", "up"), ("reversed", "true")], tag, 0.0);
        let turning = pose(block, &[("facing", "up"), ("reversed", "true")], tag, time);
        assert_eq!(turning[0].model, model);
        point(&rest[0], center + Vec3::X, center + Vec3::X);
        point(&turning[0], center + Vec3::X, center - Vec3::Z);
    }
    // A west-facing mount is x=90,y=270 in the blockstate.
    let west = pose(WOODEN, &[("facing", "west")], tag, 0.0);
    point(&west[0], center + Vec3::Y, center - Vec3::X);
    point(&west[0], center + Vec3::X, center - Vec3::Z);
    assert_eq!(
        west[0].model,
        "aeronautics:block/wooden_propeller/propeller"
    );
}

#[test]
fn propeller_accepts_either_a_blade_speed_or_create_rpm() {
    let props = [("facing", "up")];
    let center = Vec3::splat(0.5);
    let positive = pose(
        WOODEN,
        &props,
        "{RotationSpeed:19.199995f}",
        90.0 / 19.199995_f32 as f64,
    );
    let negative = pose(WOODEN, &props, "{Speed:-32f}", 90.0 / 9.6);
    point(&positive[0], center + Vec3::X, center - Vec3::Z);
    point(&negative[0], center + Vec3::X, center + Vec3::Z);
}

#[test]
fn propeller_keeps_its_phase_when_a_speed_update_stops_it() {
    let kind = Part::Wooden;
    let running = kind.read_pose(None, &tag("{Speed:64f,RotationSpeed:19.199995f}"), 0, 64.0);
    let stopped = kind.read_pose(running, &tag("{Speed:0f,RotationSpeed:0f}"), 5, 0.0);
    let at = |pose, time| {
        kind.parts(|_| Some("up"), 0.0, pose, time, [0, 0, 0])
            .into_iter()
            .next()
            .unwrap()
            .local
    };
    assert!(at(running, 5.0).abs_diff_eq(at(stopped, 20.0), 1e-6));
}

#[test]
fn smart_propeller_hub_sits_on_the_hinge_in_both_mounts() {
    // Undo the pitch in item.json. The rotor hub spans y=17..20 in floor
    // orientation and y=6..9 in propeller.json. The hinge stays fixed.
    let hub = Vec3::new(0.5, 6.0 / 16.0, 0.5);
    let mounted = Vec3::new(0.5, 17.0 / 16.0, 0.5);
    let center = Vec3::splat(0.5);
    let snbt = "{Speed:64f,RotationSpeed:19.199995f}";
    let floor = pose(SMART, &[("axis", "z")], snbt, 0.0);
    let ceiling = pose(
        SMART,
        &[("axis", "x"), ("ceiling", "true"), ("reversed", "true")],
        snbt,
        90.0 / 19.199995_f32 as f64,
    );
    assert_eq!(
        ceiling[0].model,
        "aeronautics:block/smart_propeller/propeller_reversed"
    );
    assert_eq!(ceiling[1].model, "aeronautics:block/smart_propeller/hinge");
    point(&floor[0], hub, mounted);
    point(&ceiling[0], hub, Vec3::Y + Vec3::new(0.5, -mounted.y, 0.5));
    point(
        &ceiling[0],
        hub + Vec3::X,
        Vec3::new(1.5, 1.0 - mounted.y, 0.5),
    );
    point(&ceiling[1], center + Vec3::X, center + Vec3::Z);
    point(&floor[2], center + Vec3::Z * 0.5, center - Vec3::Y * 0.5);
}

#[test]
fn envelope_and_auger_turn_about_the_block_axis() {
    for (block, model) in [
        ("aeronautics:white_envelope_encased_shaft", SHAFT),
        (AUGER_COG, "simulated:block/auger_shaft/cog"),
    ] {
        let props = [("axis", "x")];
        let rest = pose(block, &props, "{Speed:0f}", 0.0);
        let turn = pose(block, &props, "{Speed:-32f}", 90.0 / 9.6);
        assert_eq!(turn[0].model, model);
        let center = Vec3::splat(0.5);
        point(&turn[0], center + Vec3::Y, center + Vec3::X);
        let radial = rest[0].local.transform_vector3(Vec3::X);
        point(&turn[0], center + Vec3::X, center - Vec3::X.cross(radial));
    }
}

#[test]
fn analog_transmission_uses_the_cogwheels_own_kinetic_tag() {
    // Ponder shows 10 rpm and 4 rpm for the geared output.
    let props = [("axis", "y"), ("powered", "true")];
    let tag = "{Speed:10f,ExtraCogwheel:{Speed:4f}}";
    let zero = pose(ANALOG, &props, tag, 0.0);
    let ten = pose(ANALOG, &props, tag, 10.0);
    for (index, degrees) in [(0, 30.0_f32), (1, 12.0)] {
        let radial = zero[index].local.transform_vector3(Vec3::X);
        let center = Vec3::splat(0.5);
        point(
            &ten[index],
            center + Vec3::X,
            center + Quat::from_rotation_y(degrees.to_radians()) * radial,
        );
    }
}

#[test]
fn swivel_bearing_passes_shaft_rotation_when_its_cog_is_stationary() {
    // The gyroscopic bearing redstone Ponder scene powers only the shaft.
    let props = [("facing", "north")];
    let tag = "{Speed:-32f,SwivelCog:{Speed:0f}}";
    let rest = pose(SWIVEL, &props, tag, 0.0);
    let running = pose(SWIVEL, &props, tag, 90.0 / 9.6);
    let center = Vec3::splat(0.5);
    point(&running[0], center + Vec3::Y * 0.5, center - Vec3::Z * 0.5);
    point(&running[0], center + Vec3::X, center - Vec3::Y);
    assert_eq!(rest[1].local, running[1].local);
}

#[test]
fn rope_winch_shaft_and_spool_share_the_casing_axis() {
    for (facing, along_first, axis) in [
        ("south", "true", Vec3::X),
        ("south", "false", Vec3::Y),
        ("up", "false", Vec3::Z),
    ] {
        let props = [("facing", facing), ("axis_along_first", along_first)];
        let parts = pose(WINCH, &props, "{Speed:32f}", 90.0 / 9.6);
        let center = Vec3::splat(0.5);
        for part in &parts {
            point(part, center + Vec3::X * 0.5, center + axis * 0.5);
        }
        assert_eq!(parts[0].local, parts[1].local);
    }
}

#[test]
fn torsion_spring_output_holds_the_reported_angle() {
    let props = [("facing", "up")];
    // The stock Ponder scene sets an angle limit of 90 degrees.
    let spring = pose(
        SPRING,
        &props,
        "{Speed:0f,TorsionSpringOutput:{Angle:90f}}",
        20.0,
    );
    let center = Vec3::splat(0.5);
    point(&spring[0], center + Vec3::X, center - Vec3::Z);
    point(&spring[1], center + Vec3::Z * 0.5, center - Vec3::Y * 0.5);
}

#[test]
fn bearing_input_and_output_use_separate_axes_and_motion() {
    let center = Vec3::splat(0.5);
    let rest = pose(
        BEARING,
        &[("facing", "up")],
        "{Speed:32f,Running:0b,Angle:0f,RotationSpeed:0f}",
        90.0 / 9.6,
    );
    point(&rest[0], center + Vec3::X, center + Vec3::X);
    point(&rest[1], center + Vec3::Z * 0.5, center - Vec3::Y * 0.5);

    // Offroad Ponder describes the head turning at one quarter of the
    // rotational input in the opposite direction. Its shaft is perpendicular.
    let running = pose(
        BOREHEAD,
        &[("facing", "south"), ("axis_along_first", "true")],
        "{Speed:32f,Running:1b,Angle:0f,RotationSpeed:-2.4f}",
        90.0 / 2.4,
    );
    point(&running[0], center + Vec3::X, center - Vec3::Y);
    point(&running[1], center + Vec3::Y * 0.5, center + Vec3::X * 0.5);
}
