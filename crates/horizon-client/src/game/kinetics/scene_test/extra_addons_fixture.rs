//! Before/after panels for the Create add-on renderers.

use super::extra_addons::{
    CANNON_MOUNT, CARBON_BRUSHES, ORE_KINETIC_INPUT, PRINTER, addition_machine_parts,
    cannon_mount_from_nbt, cannon_mount_parts, carbon_brush_parts, ore_kinetic_input_part,
    printer_parts, printer_ticks_from_nbt,
};
use super::{
    TEMPLATE_CACHE_TESTS, machines::MachinePart, release_assets, render_release_comparison,
    speed_from_nbt, wire_tag,
};
use glam::{Mat4, Vec3};
use horizon_protocol::registry::Ids;
use std::path::Path;

const ALTERNATOR: &str = "createaddition:alternator";
const ROLLING_MILL: &str = "createaddition:rolling_mill";
const TYPES: &[&str] = &[
    CANNON_MOUNT,
    ORE_KINETIC_INPUT,
    CARBON_BRUSHES,
    ALTERNATOR,
    ROLLING_MILL,
    PRINTER,
];

const JARS: &[(&str, &str)] = &[
    ("create", "HORIZON_CREATE_FIXTURE_JAR"),
    ("createbigcannons", "HORIZON_CBC_FIXTURE_JAR"),
    ("minecraft", "HORIZON_CREATE_FIXTURE_VANILLA_JAR"),
    ("createoreexcavation", "HORIZON_ORE_EXCAVATION_FIXTURE_JAR"),
    ("create_new_age", "HORIZON_NEW_AGE_FIXTURE_JAR"),
    ("createaddition", "HORIZON_ADDITIONS_FIXTURE_JAR"),
    ("create_enchantment_industry", "HORIZON_CEI_FIXTURE_JAR"),
];

#[derive(Default)]
struct Frame {
    bodies: Vec<release_assets::Placement>,
    parts: Vec<(Vec3, MachinePart)>,
}

impl Frame {
    fn place(
        &mut self,
        jars: &mut release_assets::Jars,
        pos: [i32; 3],
        block: &str,
        props: &[(&str, &str)],
        parts: impl IntoIterator<Item = MachinePart>,
    ) {
        let at = Vec3::from_array(pos.map(|coord| coord as f32));
        let (namespace, _) = block.split_once(':').unwrap();
        self.bodies.push(release_assets::block_body(
            jars.get_mut(namespace).unwrap(),
            block,
            props,
            Mat4::from_translation(at),
        ));
        self.parts.extend(parts.into_iter().map(|part| (at, part)));
    }
}

fn frame(jars: &mut release_assets::Jars, registry: &Ids, active: bool) -> Frame {
    let mut frame = Frame::default();
    let facing = "south";
    let time = if active { 7.0 } else { 0.0 };
    let speed = |block, pos| {
        let snbt = if active { "{Speed:32f}" } else { "{Speed:0f}" };
        speed_from_nbt(&wire_tag(registry, block, pos, snbt)).unwrap()
    };

    let pos = [0, 1, 0];
    let vertical = "down";
    let mount_tag = wire_tag(
        registry,
        CANNON_MOUNT,
        pos,
        if active {
            "{Running:1b,CannonYaw:60f,CannonPitch:25f,PitchInterface:{Speed:-8f},YawInterface:{Speed:32f}}"
        } else {
            "{Running:0b,CannonYaw:0f,CannonPitch:0f,PitchInterface:{Speed:0f},YawInterface:{Speed:0f}}"
        },
    );
    let mount = cannon_mount_from_nbt(None, &mount_tag).unwrap();
    frame.place(
        jars,
        pos,
        CANNON_MOUNT,
        &[
            ("assembly_powered", if active { "true" } else { "false" }),
            ("facing", facing),
            ("fire_powered", "false"),
            ("vertical_direction", vertical),
        ],
        cannon_mount_parts(facing, vertical, Some(&mount), time, 1.0, pos),
    );

    let pos = [3, 1, 0];
    let shaft = "south";
    frame.place(
        jars,
        pos,
        ORE_KINETIC_INPUT,
        &[("facing", "north"), ("shaft", shaft)],
        [ore_kinetic_input_part(shaft, speed(ORE_KINETIC_INPUT, pos), time, pos).unwrap()],
    );

    for (pos, block) in [
        ([6, 1, 0], CARBON_BRUSHES),
        ([0, 0, 5], ALTERNATOR),
        ([3, 0, 5], ROLLING_MILL),
    ] {
        let speed = speed(block, pos);
        let parts = if block == CARBON_BRUSHES {
            carbon_brush_parts(facing, speed, time, pos)
        } else {
            addition_machine_parts(block, facing, speed, time, pos)
        };
        frame.place(jars, pos, block, &[("facing", facing)], parts.unwrap());
    }

    let pos = [6, 0, 5];
    let printer_tag = wire_tag(
        registry,
        PRINTER,
        pos,
        if active {
            "{ProcessingTicks:25}"
        } else {
            "{ProcessingTicks:-1}"
        },
    );
    let ticks = printer_ticks_from_nbt(&printer_tag).unwrap();
    frame.place(
        jars,
        pos,
        PRINTER,
        &[("facing", facing)],
        printer_parts(Some(ticks), 1.0),
    );
    frame
}

pub(super) fn render() {
    let _cache = TEMPLATE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = std::env::var_os("HORIZON_EXTRA_ADDONS_FIXTURE_PNG").expect("output PNG path");
    let mut jars = JARS
        .iter()
        .map(|&(namespace, var)| {
            let jar = std::env::var_os(var).unwrap_or_else(|| panic!("{var}: official JAR path"));
            (namespace, release_assets::open(Path::new(&jar)))
        })
        .collect();
    let registry = Ids::from_names(TYPES.iter().map(|name| (*name).to_owned()).collect());
    let [before, after] = [false, true].map(|active| frame(&mut jars, &registry, active));
    let models = release_assets::load_scene(
        &mut jars,
        [before.bodies, after.bodies],
        before
            .parts
            .iter()
            .chain(&after.parts)
            .map(|(_, part)| part.model),
        [],
    );
    let release = models.release.as_ref().unwrap();
    let vertices = [(false, before.parts), (true, after.parts)].map(|(active, parts)| {
        let mut out = Vec::new();
        release.draw_bodies(&models, active, &mut out);
        for (at, part) in parts {
            models.parts(at, [part], 0xffffff, &mut out);
        }
        assert!(out.iter().all(|v| Vec3::from(v.pos).is_finite()));
        assert_eq!(out.len() % 4, 0);
        out
    });
    // A shallow view keeps the printer nozzles visible beneath the body.
    let center = Vec3::new(3.5, 1.1, 3.0);
    let view = Mat4::look_at_rh(center + Vec3::new(5.0, 2.5, 21.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-4.6, 4.6, -3.2, 3.2, 0.1, 90.0) * view;
    render_release_comparison(
        release,
        vertices,
        view,
        view_proj,
        [920, 640],
        Path::new(&output),
    );
}
