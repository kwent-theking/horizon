//! Static casings and moving parts from the original bundled Aeronautics JAR.

use super::super::{
    aeronautics::{
        ANALOG, ANDESITE, AUGER_BE, AUGER_COG, BEARING, BOREHEAD, ENVELOPE_BE, Part, SMART, SPRING,
        SWIVEL, WINCH, WOODEN, is_kinetic_be,
    },
    machines::MachinePart,
};
use super::{
    TEMPLATE_CACHE_TESTS, release_assets, render_release_comparison, speed_from_nbt, wire_tag,
};
use glam::{Mat4, Vec3};
use horizon_protocol::registry::Ids;
use std::{io::Cursor, path::Path};
use zip::ZipArchive;

const ENVELOPE: &str = "aeronautics:white_envelope_encased_shaft";
const NESTED: &[(&str, &str)] = &[
    (
        "aeronautics",
        "dev.eriksonn.aeronautics.aeronautics-neoforge-1.21.1-1.3.2.jar",
    ),
    (
        "simulated",
        "dev.simulated_team.simulated.simulated-neoforge-1.21.1-1.3.2.jar",
    ),
    (
        "offroad",
        "dev.ryanhcode.offroad.offroad-neoforge-1.21.1-1.3.2.jar",
    ),
];

struct Sample {
    block: &'static str,
    kind: &'static str,
    props: &'static [(&'static str, &'static str)],
    running: &'static str,
}

const SAMPLES: &[Sample] = &[
    Sample {
        block: WOODEN,
        kind: WOODEN,
        props: &[("facing", "south"), ("reversed", "false")],
        running: "{Speed:64f,RotationSpeed:19.199995f}",
    },
    Sample {
        block: ANDESITE,
        kind: ANDESITE,
        props: &[("facing", "south"), ("reversed", "true")],
        running: "{Speed:64f,RotationSpeed:19.199995f}",
    },
    Sample {
        block: SMART,
        kind: SMART,
        props: &[("axis", "z"), ("ceiling", "false"), ("reversed", "false")],
        running: "{Speed:64f,RotationSpeed:19.199995f}",
    },
    Sample {
        block: BEARING,
        kind: BEARING,
        props: &[("facing", "west")],
        running: "{Speed:-32f,Running:0b,Angle:0f,RotationSpeed:0f}",
    },
    Sample {
        block: ENVELOPE,
        kind: ENVELOPE_BE,
        props: &[("axis", "z")],
        running: "{Speed:-32f}",
    },
    Sample {
        block: BOREHEAD,
        kind: BOREHEAD,
        props: &[("facing", "south"), ("axis_along_first", "true")],
        running: "{Speed:32f,Running:1b,Angle:0f,RotationSpeed:-2.4f}",
    },
    Sample {
        block: ANALOG,
        kind: "simulated:simple",
        props: &[("axis", "y"), ("powered", "true")],
        running: "{Speed:10f,ExtraCogwheel:{Speed:4f}}",
    },
    Sample {
        block: AUGER_COG,
        kind: AUGER_BE,
        props: &[("axis", "z"), ("encased", "false"), ("section", "single")],
        running: "{Speed:-32f}",
    },
    Sample {
        block: WINCH,
        kind: WINCH,
        props: &[("facing", "up"), ("axis_along_first", "false")],
        running: "{Speed:32f}",
    },
    Sample {
        block: SPRING,
        kind: SPRING,
        props: &[("facing", "south"), ("powered", "false")],
        running: "{Speed:32f,TorsionSpringOutput:{Angle:90f}}",
    },
    Sample {
        block: SWIVEL,
        kind: SWIVEL,
        props: &[
            ("facing", "up"),
            ("assembled", "false"),
            ("powered", "false"),
        ],
        running: "{Speed:-32f,SwivelCog:{Speed:0f}}",
    },
];

#[derive(Default)]
struct Frame {
    bodies: Vec<release_assets::Placement>,
    parts: Vec<(Vec3, MachinePart)>,
}

fn frame(jars: &mut release_assets::Jars<Cursor<Vec<u8>>>, ids: &Ids, active: bool) -> Frame {
    let mut frame = Frame::default();
    for (index, sample) in SAMPLES.iter().enumerate() {
        let pos = [(index as i32 % 6) * 3, 0, (index as i32 / 6) * 4];
        let at = Vec3::from_array(pos.map(|coord| coord as f32));
        let (namespace, _) = sample.block.split_once(':').unwrap();
        let jar = jars.get_mut(namespace).unwrap();
        let transform = Mat4::from_translation(at);
        if sample.block == AUGER_COG {
            let bodies =
                release_assets::multipart_bodies(jar, sample.block, sample.props, transform);
            assert_eq!(bodies.len(), 3);
            frame.bodies.extend(bodies);
        } else {
            frame.bodies.push(release_assets::block_body(
                jar,
                sample.block,
                sample.props,
                transform,
            ));
        }
        assert!(is_kinetic_be(sample.kind));
        let tag = wire_tag(
            ids,
            sample.kind,
            pos,
            if active { sample.running } else { "{Speed:0f}" },
        );
        let rpm = speed_from_nbt(&tag).unwrap();
        let kind = Part::for_block(sample.block).unwrap();
        let prop = |key: &str| {
            sample
                .props
                .iter()
                .find(|(name, _)| *name == key)
                .map(|p| p.1)
        };
        let time = if active { 2.5 } else { 0.0 };
        let parts = kind.parts(prop, rpm, kind.read_pose(None, &tag, 0, rpm), time, pos);
        frame.parts.extend(parts.into_iter().map(|part| (at, part)));
    }
    frame
}

pub(super) fn render() {
    let _cache = TEMPLATE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = std::env::var_os("HORIZON_AERONAUTICS_FIXTURE_PNG").expect("output PNG path");
    let pinned =
        std::env::var_os("HORIZON_AERONAUTICS_FIXTURE_JAR").expect("official bundled JAR path");
    let mut bundled = release_assets::open(Path::new(&pinned));
    let mut jars = release_assets::Jars::new();
    for &(namespace, name) in NESTED {
        let entry = release_assets::read(&mut bundled, &format!("META-INF/jarjar/{name}"));
        jars.insert(namespace, ZipArchive::new(Cursor::new(entry)).unwrap());
    }
    for (namespace, var) in [
        ("create", "HORIZON_CREATE_FIXTURE_JAR"),
        ("minecraft", "HORIZON_CREATE_FIXTURE_VANILLA_JAR"),
    ] {
        let jar = std::env::var_os(var).unwrap_or_else(|| panic!("{var}: official JAR path"));
        let bytes = std::fs::read(&jar).unwrap_or_else(|e| panic!("{var}: {e}"));
        jars.insert(namespace, ZipArchive::new(Cursor::new(bytes)).unwrap());
    }
    let ids = Ids::from_names(
        SAMPLES
            .iter()
            .map(|sample| sample.kind.to_owned())
            .collect(),
    );
    let [before, after] = [false, true].map(|active| frame(&mut jars, &ids, active));
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
            let previous_len = out.len();
            models.parts(at, [part], 0xffffff, &mut out);
            assert!(out.len() > previous_len, "empty fixture partial at {at}");
        }
        assert!(out.iter().all(|vertex| Vec3::from(vertex.pos).is_finite()));
        out
    });
    assert_eq!(vertices[0].len(), vertices[1].len());
    assert!(
        vertices[0]
            .iter()
            .zip(&vertices[1])
            .any(|(a, b)| a.pos != b.pos)
    );

    let center = Vec3::new(8.0, 0.8, 2.5);
    let view = Mat4::look_at_rh(center + Vec3::new(4.0, 10.0, 22.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-9.6, 9.6, -5.6, 5.6, 0.1, 90.0) * view;
    render_release_comparison(
        release,
        vertices,
        view,
        view_proj,
        [1120, 650],
        Path::new(&output),
    );
}
