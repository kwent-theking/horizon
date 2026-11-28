//! Headless before/after scene driven by captured protocol shapes and release models.

mod release_assets;

use super::{ControllerPose, State, VisualPose, projectile};
use glam::{DVec3, Mat4, Vec3};
use horizon_client::{
    gfx::test_frame,
    mesher::Vertex,
    net::{ADVANCED_ENTITY_SPAWN_CHANNEL as ADVANCED_ADD_ENTITY, CBC_ANIMATE_CHANNEL},
};
use horizon_core::position::BlockPos;
use horizon_protocol::{
    buf::{Reader, Writer},
    entity_data::{CBC_BIG_CANNON_PLUME, Particle, ParticleData},
    packets::{
        configuration::CustomPayload,
        play::{
            AddEntity, BlockEntityData, Clientbound, EntityPositionSync, LevelParticles,
            PositionPath, RemoveEntities,
        },
    },
    registry::Ids,
    translation::{p767, set_server_names},
};
use horizon_world::{
    chunk::BlockState,
    nbt::{self, Tag},
    registry::{BlockRegistry, BlockStateId},
};
use release_assets::{
    ANDESITE, AUTO_BARREL, AUTO_BREECH, AUTO_SPRING, BARREL, BRASS, BREECH, CHAMBER, CHASSIS,
    MOUNT, PISTON, PISTON_HEAD, PISTON_POLE, ROTATING_MOUNT, ReleaseAssets, SHELL,
};
use std::{collections::BTreeMap, path::Path};

const PISTON_KIND: &str = "create:stationary_contraption";
const CANNON_KIND: &str = "createbigcannons:pitch_contraption";
const PISTON_ID: i32 = 300;
const CANNON_ID: i32 = 301;
const SHELL_ID: i32 = 302;
const PISTON_START: [f64; 3] = [0.0, 0.0, 1.0];
const PISTON_END: [f64; 3] = [0.0, 0.0, 2.5];
const CANNON_START: [f64; 3] = [5.5, 2.0, 0.5];
const CANNON_YAW: f32 = 35.0;
const CANNON_PITCH: f32 = 22.5;

const PALETTE: &[(&str, &[(&str, &str)])] = &[
    (
        CHASSIS,
        &[
            ("axis", "z"),
            ("sticky_bottom", "false"),
            ("sticky_top", "false"),
        ],
    ),
    (ANDESITE, &[]),
    (BRASS, &[]),
    (BREECH, &[("axis_along_first", "true"), ("facing", "south")]),
    (CHAMBER, &[("facing", "south")]),
    (BARREL, &[("facing", "south")]),
    (PISTON_HEAD, &[("facing", "south"), ("type", "normal")]),
    (PISTON_POLE, &[("facing", "south")]),
    (AUTO_BREECH, &[("facing", "south"), ("handle", "false")]),
    (AUTO_SPRING, &[("facing", "south")]),
    (
        AUTO_BARREL,
        &[
            ("assembled", "true"),
            ("end", "nothing"),
            ("facing", "south"),
        ],
    ),
    (
        AUTO_BARREL,
        &[
            ("assembled", "false"),
            ("end", "nothing"),
            ("facing", "south"),
        ],
    ),
];
const AUTO_BREECH_STATE: usize = 8;
const AUTO_SPRING_STATE: usize = 9;
const AUTO_STATIONARY_BARREL_STATE: usize = 10;

fn resolve(block: &BlockState) -> Option<BlockStateId> {
    PALETTE
        .iter()
        .position(|(name, properties)| {
            block.name == *name
                && block.properties.len() == properties.len()
                && properties.iter().all(|&(key, value)| {
                    block.properties.get(key).map(String::as_str) == Some(value)
                })
        })
        .map(|i| BlockStateId(i as u32 + 1))
}

fn packet_roundtrip(packet: Clientbound) -> Clientbound {
    let bytes = packet.encode();
    let mut wire = Reader::new(&bytes);
    Clientbound::decode(wire.varint().unwrap(), wire.rest()).expect("synthetic packet round trip")
}

fn block_list(blocks: &[([i32; 3], usize)]) -> Tag {
    block_list_with_data(
        &blocks
            .iter()
            .map(|&(pos, state)| (pos, state, None))
            .collect::<Vec<_>>(),
    )
}

/// `Contraption.Blocks` with the optional block entity `Data` of each block.
fn block_list_with_data(blocks: &[([i32; 3], usize, Option<Tag>)]) -> Tag {
    let palette = blocks
        .iter()
        .map(|&(_, state, _)| {
            let (name, properties) = PALETTE[state];
            Tag::compound([
                ("Name", Tag::String(name.into())),
                (
                    "Properties",
                    Tag::compound(
                        properties
                            .iter()
                            .map(|&(key, value)| (key, Tag::String(value.into()))),
                    ),
                ),
            ])
        })
        .collect();
    let list = blocks
        .iter()
        .enumerate()
        .map(|(index, &([x, y, z], _, ref data))| {
            let mut entry = vec![
                ("Pos", Tag::Long(BlockPos::new(x, y, z).pack())),
                ("State", Tag::Int(index as i32)),
            ];
            entry.extend(data.clone().map(|data| ("Data", data)));
            Tag::compound(entry)
        })
        .collect();
    Tag::compound([
        ("Palette", Tag::List(palette)),
        ("BlockList", Tag::List(list)),
    ])
}

fn install(state: &mut State, id: i32, kind: &str, position: [f64; 3], spawn: Tag) {
    let kinds = Ids::from_names(vec![PISTON_KIND.to_owned(), CANNON_KIND.to_owned()]);
    let Clientbound::AddEntity(add) = packet_roundtrip(Clientbound::AddEntity(AddEntity {
        id,
        uuid: id as u128,
        kind: kinds.id(kind).unwrap(),
        position,
        velocity: [0.0; 3],
        yaw: 0,
        pitch: 0,
        head_yaw: 0,
        data: 0,
    })) else {
        panic!("expected add entity");
    };
    let degrees = |angle| f32::from(angle) * (360.0 / 256.0);
    assert!(state.add(
        add.id,
        kinds.name(add.kind).unwrap(),
        add.position,
        degrees(add.yaw),
        degrees(add.pitch)
    ));

    let tag = nbt::write_network(&spawn, nbt::Limits::default()).unwrap();
    let mut bytes = Writer::new();
    bytes.varint(add.id);
    bytes.byte_array(&tag);
    let Clientbound::CustomPayload(payload) =
        packet_roundtrip(Clientbound::CustomPayload(CustomPayload {
            channel: ADVANCED_ADD_ENTITY.to_owned(),
            data: bytes.data,
        }))
    else {
        panic!("expected advanced add entity");
    };
    assert_eq!(payload.channel, ADVANCED_ADD_ENTITY);
    assert!(state.apply_spawn(&payload.data, resolve), "spawn {kind}");
}

fn install_cannon(state: &mut State) {
    install(
        state,
        CANNON_ID,
        CANNON_KIND,
        CANNON_START,
        Tag::compound([
            ("ControllerRelative", Tag::IntArray(vec![0, -2, 0])),
            ("InitialOrientation", Tag::String("SOUTH".into())),
            ("Yaw", Tag::Float(0.0f32.to_bits())),
            ("Pitch", Tag::Float(0.0f32.to_bits())),
            (
                "Contraption",
                Tag::compound([
                    (
                        "Type",
                        Tag::String("createbigcannons:mounted_cannon".into()),
                    ),
                    ("Anchor", Tag::IntArray(vec![5, 2, 0])),
                    (
                        "Blocks",
                        block_list(&[([0, 0, 0], 3), ([0, 0, 1], 4), ([0, 0, 2], 5)]),
                    ),
                ]),
            ),
        ]),
    );
}

fn sync(state: &mut State, id: i32, end: [f64; 3], yaw: f32, pitch: f32) {
    let Clientbound::EntityPositionSync(packet) =
        packet_roundtrip(Clientbound::EntityPositionSync(EntityPositionSync {
            id,
            path: PositionPath::Linear(end),
            yaw,
            pitch,
            on_ground: false,
        }))
    else {
        panic!("expected position sync");
    };
    assert_eq!(state.sync(&packet), Some(end));
}

fn controller_tag(pos: [i32; 3], tag: Tag) -> Tag {
    let Clientbound::BlockEntityData(packet) =
        packet_roundtrip(Clientbound::BlockEntityData(BlockEntityData {
            pos,
            kind: 0,
            tag,
        }))
    else {
        panic!("expected controller update");
    };
    assert_eq!(packet.pos, pos);
    packet.tag
}

fn float(tag: &Tag, field: &str) -> f32 {
    let Some(Tag::Float(bits)) = tag.get(field) else {
        panic!("missing {field} in fixture controller tag");
    };
    f32::from_bits(*bits)
}

fn matrix(pose: &VisualPose<'_>) -> Mat4 {
    Mat4::from_translation(pose.anchor.as_vec3()) * pose.transform
}

fn block_center(pose: &VisualPose<'_>, index: usize) -> Vec3 {
    let local = pose.blocks[index].local.map(|value| value as f32);
    matrix(pose).transform_point3(Vec3::from_array(local) + Vec3::splat(0.5))
}

fn append_body(assets: &ReleaseAssets, pose: &VisualPose<'_>, out: &mut Vec<Vertex>) {
    for block in pose.blocks {
        let (name, _) = PALETTE[block.state.0 as usize - 1];
        let local = Vec3::from_array(block.local.map(|value| value as f32));
        assets.append(name, matrix(pose) * Mat4::from_translation(local), out);
    }
}

fn scene(
    assets: &ReleaseAssets,
    piston: &VisualPose<'_>,
    cannon: &VisualPose<'_>,
    yaw: f32,
) -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for x in -1..=7 {
        for z in -2..=5 {
            let floor = Mat4::from_translation(Vec3::new(x as f32, -0.125, z as f32))
                * Mat4::from_scale(Vec3::new(1.0, 0.125, 1.0));
            assets.append(ANDESITE, floor, &mut vertices);
        }
    }
    assets.append(PISTON, Mat4::IDENTITY, &mut vertices);
    assets.append(MOUNT, Mat4::from_translation(Vec3::X * 5.0), &mut vertices);
    let support = Mat4::from_translation(Vec3::new(5.5, 1.5, 0.5))
        * Mat4::from_rotation_y(-yaw.to_radians())
        * Mat4::from_translation(Vec3::splat(-0.5));
    assets.append(ROTATING_MOUNT, support, &mut vertices);
    for pose in [piston, cannon] {
        append_body(assets, pose, &mut vertices);
    }
    vertices
}

fn render_comparison(assets: &ReleaseAssets, before: &[Vertex], after: &[Vertex], output: &Path) {
    const WIDTH: u32 = 800;
    const HEIGHT: u32 = 480;
    let center = Vec3::new(3.25, 0.95, 2.0);
    let view = Mat4::look_at_rh(center + Vec3::new(0.0, 8.5, 14.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-5.4, 5.4, -3.24, 3.24, 0.1, 90.0) * view;
    let render = |scene: &[Vertex]| {
        test_frame::render_with_atlas(scene, view_proj, WIDTH, HEIGHT, assets.size, &assets.pixels)
            .expect("render public contraption fixture")
    };
    let before = render(before);
    let after = render(after);
    let row = WIDTH as usize * 4;
    assert_eq!(before.len(), row * HEIGHT as usize);
    assert_eq!(after.len(), before.len());
    let mut changed = [0usize; 2];
    for (left, right) in before.chunks_exact(row).zip(after.chunks_exact(row)) {
        for (x, (a, b)) in left.chunks_exact(4).zip(right.chunks_exact(4)).enumerate() {
            changed[usize::from(x >= WIDTH as usize / 2)] +=
                usize::from(a[..3].iter().zip(&b[..3]).any(|(l, r)| l.abs_diff(*r) > 24));
        }
    }
    assert!(
        changed.into_iter().all(|count| count > 250),
        "both fixtures must move on screen: {changed:?}"
    );
    write_comparison(WIDTH, HEIGHT, &before, &after, output);
    eprintln!("Contraption fixture: changed pixels in piston and CBC views {changed:?}");
}

fn write_comparison(width: u32, height: u32, before: &[u8], after: &[u8], output: &Path) {
    let row = width as usize * 4;
    assert_eq!(before.len(), row * height as usize);
    assert_eq!(after.len(), before.len());
    let mut comparison = Vec::with_capacity(before.len() + after.len());
    for (left, right) in before.chunks_exact(row).zip(after.chunks_exact(row)) {
        comparison.extend_from_slice(left);
        comparison.extend_from_slice(right);
    }
    let mut encoder = png::Encoder::new(std::fs::File::create(output).unwrap(), width * 2, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&comparison)
        .unwrap();
}

#[test]
#[ignore = "requires the official Create and CBC release JARs and a rendering adapter"]
fn contraption_fixture_public_assets() {
    let create = std::env::var_os("HORIZON_CREATE_FIXTURE_JAR").expect("official Create JAR path");
    let cbc = std::env::var_os("HORIZON_CBC_FIXTURE_JAR").expect("official CBC JAR path");
    let output = std::env::var_os("HORIZON_CONTRAPTION_FIXTURE_PNG").expect("output PNG path");
    let assets = ReleaseAssets::load(Path::new(&create), Path::new(&cbc))
        .expect("release models and textures");
    let mut state = State::default();
    install(
        &mut state,
        PISTON_ID,
        PISTON_KIND,
        PISTON_START,
        Tag::compound([
            ("ControllerRelative", Tag::IntArray(vec![0, 0, -1])),
            (
                "Contraption",
                Tag::compound([
                    ("Type", Tag::String("create:piston".into())),
                    ("Anchor", Tag::IntArray(vec![0, 0, 1])),
                    ("Orientation", Tag::Int(3)),
                    (
                        "Blocks",
                        block_list(&[
                            ([0, 0, 0], 0),
                            ([0, 0, 1], 1),
                            ([0, 1, 1], 2),
                            ([0, 0, -1], 6),
                            ([0, 0, -2], 7),
                            ([0, 0, -3], 7),
                        ]),
                    ),
                ]),
            ),
        ]),
    );
    install_cannon(&mut state);
    let piston = state
        .visual(PISTON_ID, 1.0, ControllerPose::default())
        .unwrap();
    let cannon = state
        .visual(CANNON_ID, 1.0, ControllerPose::default())
        .unwrap();
    assert_eq!(piston.blocks.len(), 6);
    assert!(state.has_piston_head([0, 0, 0]));
    assert_eq!(piston.blocks[3].local, [0, 0, -1]);
    assert_eq!(piston.blocks[4].local, [0, 0, -2]);
    assert_eq!(piston.blocks[5].local, [0, 0, -3]);
    assert_eq!(cannon.blocks.len(), 3);
    assert!(
        piston
            .anchor
            .abs_diff_eq(DVec3::from_array(PISTON_START), 1e-6)
    );
    let payload_before = block_center(&piston, 2);
    let head_before = block_center(&piston, 3);
    let pole_before = block_center(&piston, 5);
    let pivot = block_center(&cannon, 0);
    let barrel_before = block_center(&cannon, 2);
    let before = scene(&assets, &piston, &cannon, 0.0);

    sync(&mut state, PISTON_ID, PISTON_END, 0.0, 0.0);
    sync(
        &mut state,
        CANNON_ID,
        CANNON_START,
        CANNON_YAW,
        -CANNON_PITCH,
    );
    for _ in 0..3 {
        state.tick();
    }
    let piston_wire = state
        .visual(PISTON_ID, 1.0, ControllerPose::default())
        .unwrap();
    let cannon_wire = state
        .visual(CANNON_ID, 1.0, ControllerPose::default())
        .unwrap();
    let piston_data = controller_tag(
        [0, 0, 0],
        Tag::compound([("Offset", Tag::Float(1.5f32.to_bits()))]),
    );
    let cannon_data = controller_tag(
        [5, 0, 0],
        Tag::compound([
            ("CannonYaw", Tag::Float(CANNON_YAW.to_bits())),
            ("CannonPitch", Tag::Float(CANNON_PITCH.to_bits())),
        ]),
    );
    let piston = state
        .visual(
            PISTON_ID,
            1.0,
            ControllerPose {
                piston_offset: Some(f64::from(float(&piston_data, "Offset"))),
                ..ControllerPose::default()
            },
        )
        .unwrap();
    let yaw = float(&cannon_data, "CannonYaw");
    let cannon = state
        .visual(
            CANNON_ID,
            1.0,
            ControllerPose {
                cannon_angles: Some((yaw, float(&cannon_data, "CannonPitch"))),
                ..ControllerPose::default()
            },
        )
        .unwrap();
    let payload_after = block_center(&piston, 2);
    assert!(payload_after.abs_diff_eq(payload_before + Vec3::Z * 1.5, 1e-5));
    assert!(payload_after.abs_diff_eq(block_center(&piston_wire, 2), 1e-5));
    assert!(block_center(&piston, 3).abs_diff_eq(head_before + Vec3::Z * 1.5, 1e-5));
    assert!(block_center(&piston, 5).abs_diff_eq(pole_before + Vec3::Z * 1.5, 1e-5));
    let barrel_after = block_center(&cannon, 2);
    assert!(barrel_after.abs_diff_eq(block_center(&cannon_wire, 2), 1e-5));
    assert!(pivot.abs_diff_eq(block_center(&cannon, 0), 1e-5));
    assert!((barrel_after.distance(pivot) - barrel_before.distance(pivot)).abs() < 1e-5);
    assert!(barrel_after.y > barrel_before.y + 0.6);
    assert!(barrel_after.x < barrel_before.x - 0.7);
    let after = scene(&assets, &piston, &cannon, yaw);
    render_comparison(&assets, &before, &after, Path::new(&output));
}

const FIRE_WIDTH: u32 = 800;
const FIRE_HEIGHT: u32 = 480;

fn packet_767(id: i32, wire: &[u8]) -> Clientbound {
    let native = p767::cb(id);
    let translated = p767::body767_to_777(native, wire);
    Clientbound::decode(native, translated.as_deref().unwrap_or(wire))
        .expect("synthetic 1.21.1 play packet")
}

fn fire_plume(at: Vec3) -> LevelParticles {
    const PARTICLE_KIND: i32 = 150;
    let mut wire = Writer::new();
    wire.bool(true);
    for coordinate in at.as_dvec3().to_array() {
        wire.f64(coordinate);
    }
    for direction in Vec3::Z.to_array() {
        wire.f32(direction);
    }
    wire.f32(1.0);
    wire.i32(0);
    wire.varint(PARTICLE_KIND);
    wire.f32(1.5);
    wire.f32(2.0);
    wire.varint(10);

    // The synthetic server assigns its own numeric modded registry ID.
    set_server_names(&format!(
        "minecraft:particle_type\t{PARTICLE_KIND}\tcreatebigcannons:big_cannon_plume"
    ));
    let received = packet_767(41, &wire.data);
    set_server_names("");
    let Clientbound::LevelParticles(plume) = received else {
        panic!("expected CBC level particles");
    };
    assert_eq!(
        plume.particle,
        Particle {
            id: CBC_BIG_CANNON_PLUME,
            data: Some(ParticleData::CannonPlume {
                size: 1.5,
                power: 2.0,
                lifetime: 10,
            }),
        }
    );
    plume
}

fn custom_payload(channel: &str, data: Vec<u8>) -> Vec<u8> {
    let Clientbound::CustomPayload(payload) =
        packet_roundtrip(Clientbound::CustomPayload(CustomPayload {
            channel: channel.to_owned(),
            data,
        }))
    else {
        panic!("expected custom payload");
    };
    assert_eq!(payload.channel, channel);
    payload.data
}

fn launch_shell(state: &mut State, start: DVec3, velocity: DVec3) {
    let kinds = Ids::from_names(vec![CANNON_KIND.to_owned(), SHELL.to_owned()]);
    let mut wire = Writer::new();
    wire.varint(SHELL_ID);
    wire.uuid(SHELL_ID as u128);
    wire.varint(kinds.id(SHELL).unwrap());
    for coordinate in start.to_array() {
        wire.f64(coordinate);
    }
    for _ in 0..3 {
        wire.u8(0);
    }
    wire.varint(0);
    for component in velocity.to_array() {
        wire.i16((component * 8000.0).round() as i16);
    }
    let Clientbound::AddEntity(add) = packet_767(1, &wire.data) else {
        panic!("expected shell spawn");
    };
    assert!(state.projectiles.add(kinds.name(add.kind).unwrap(), &add));

    let mut spawn = Writer::new();
    spawn.f32(0.0);
    spawn.f32(0.0);
    for component in velocity.to_array().into_iter().chain(DVec3::Z.to_array()) {
        spawn.f64(component);
    }
    let mut wire = Writer::new();
    wire.varint(SHELL_ID);
    wire.byte_array(&spawn.data);
    assert!(
        state
            .projectiles
            .apply_spawn(&custom_payload(ADVANCED_ADD_ENTITY, wire.data,))
    );
}

fn track_shell(state: &mut State, end: DVec3, velocity: DVec3) {
    let mut wire = Writer::new();
    wire.varint(1);
    wire.varint(SHELL_ID);
    for component in end.to_array().into_iter().chain(velocity.to_array()) {
        wire.f64(component);
    }
    wire.f32(0.0);
    wire.f32(0.0);
    wire.bool(false);
    wire.varint(3);
    assert!(state.projectiles.apply_motion(&custom_payload(
        projectile::PRECISE_MOTION_CHANNEL,
        wire.data,
    )));
}

fn firing_scene(assets: &ReleaseAssets, state: &State) -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for x in 4..=6 {
        for z in -1..=7 {
            let floor = Mat4::from_translation(Vec3::new(x as f32, -0.125, z as f32))
                * Mat4::from_scale(Vec3::new(1.0, 0.125, 1.0));
            assets.append(ANDESITE, floor, &mut vertices);
        }
    }
    assets.append(MOUNT, Mat4::from_translation(Vec3::X * 5.0), &mut vertices);
    assets.append(
        ROTATING_MOUNT,
        Mat4::from_translation(Vec3::new(5.0, 1.0, 0.0)),
        &mut vertices,
    );
    append_body(
        assets,
        &state
            .visual(CANNON_ID, 1.0, ControllerPose::default())
            .unwrap(),
        &mut vertices,
    );
    if let Some(shell) = state.projectiles.visual(SHELL_ID, 1.0) {
        let shell_registry = BlockRegistry::from_ordered_states(
            ["up", "north"]
                .map(|facing| BlockState {
                    name: SHELL.to_owned(),
                    properties: [
                        ("facing".to_owned(), facing.to_owned()),
                        ("waterlogged".to_owned(), "false".to_owned()),
                    ]
                    .into(),
                })
                .to_vec(),
            BTreeMap::from([(SHELL.to_owned(), 0)]),
        )
        .unwrap();
        assert_eq!(
            shell
                .model
                .expect("big cannon block model")
                .resolve(&shell_registry),
            Some(BlockStateId(1))
        );
        assets.append(
            SHELL,
            projectile::transform(shell.pos.as_vec3(), shell.direction),
            &mut vertices,
        );
    }
    vertices
}

fn changed_near(
    before: &[u8],
    after: &[u8],
    view_proj: Mat4,
    at: Vec3,
    radius: i32,
    differs: impl Fn(&[u8], &[u8]) -> bool,
) -> usize {
    let projected = view_proj.project_point3(at);
    let x = ((projected.x + 1.0) * FIRE_WIDTH as f32 * 0.5).round() as i32;
    let y = ((1.0 - projected.y) * FIRE_HEIGHT as f32 * 0.5).round() as i32;
    assert!((radius..FIRE_WIDTH as i32 - radius).contains(&x));
    assert!((radius..FIRE_HEIGHT as i32 - radius).contains(&y));
    let mut count = 0;
    for row in y - radius..=y + radius {
        for col in x - radius..=x + radius {
            let index = (row as usize * FIRE_WIDTH as usize + col as usize) * 4;
            if differs(&before[index..index + 3], &after[index..index + 3]) {
                count += 1;
            }
        }
    }
    count
}

#[test]
#[ignore = "requires the official Create and CBC release JARs and a rendering adapter"]
fn cannon_fire_fixture_public_assets() {
    let create = std::env::var_os("HORIZON_CREATE_FIXTURE_JAR").expect("official Create JAR path");
    let cbc = std::env::var_os("HORIZON_CBC_FIXTURE_JAR").expect("official CBC JAR path");
    let output = std::env::var_os("HORIZON_CBC_FIRE_FIXTURE_PNG").expect("output PNG path");
    let assets = ReleaseAssets::load(Path::new(&create), Path::new(&cbc))
        .expect("release models and textures");
    let mut state = State::default();
    install_cannon(&mut state);
    let rest = state
        .visual(CANNON_ID, 1.0, ControllerPose::default())
        .unwrap();
    let rest_breech = block_center(&rest, 0);
    let rest_centers = [0, 1, 2].map(|index| block_center(&rest, index));
    let (rest_transform, rest_moving) = (rest.transform, rest.moving_transform);
    let rest_barrel = block_center(&rest, 2);
    assert!(state.projectiles.visual(SHELL_ID, 1.0).is_none());
    let before = firing_scene(&assets, &state);

    // The plume is a particle for the particle engine; it must not touch the cannon.
    let plume = fire_plume(rest_barrel - Vec3::Z);
    assert_eq!(plume.pos, (rest_barrel - Vec3::Z).as_dvec3().to_array());
    let shell_start = rest_barrel.as_dvec3();
    let velocity = DVec3::new(0.0, 0.045, 2.4);
    launch_shell(&mut state, shell_start, velocity);
    assert!(
        state
            .projectiles
            .visual(SHELL_ID, 1.0)
            .unwrap()
            .pos
            .abs_diff_eq(shell_start, 1e-7)
    );
    track_shell(&mut state, shell_start + velocity * 3.0, velocity);
    state.tick();
    state.projectiles.tick_in_air();

    let fired = state
        .visual(CANNON_ID, 1.0, ControllerPose::default())
        .unwrap();
    assert_eq!(fired.transform, rest_transform);
    assert_eq!(fired.moving_transform, rest_moving);
    for (index, center) in rest_centers.into_iter().enumerate() {
        assert!(block_center(&fired, index).abs_diff_eq(center, 1e-6));
    }
    assert!(fired.ejectors.is_empty());
    let shell = state.projectiles.visual(SHELL_ID, 1.0).unwrap();
    assert!(shell.pos.z > shell_start.z + 0.25);
    assert!(shell.direction.normalize().dot(Vec3::Z) > 0.99);
    let shell_at = shell.pos.as_vec3();
    let after = firing_scene(&assets, &state);
    assert!(
        after.len() > before.len(),
        "the shell must emit rendering geometry"
    );

    let Clientbound::RemoveEntities(remove) =
        packet_roundtrip(Clientbound::RemoveEntities(RemoveEntities {
            ids: vec![SHELL_ID],
        }))
    else {
        panic!("expected shell removal");
    };
    for id in remove.ids {
        state.remove(id);
    }
    assert!(state.projectiles.visual(SHELL_ID, 1.0).is_none());
    for _ in 0..40 {
        state.tick();
    }
    let recovered = state
        .visual(CANNON_ID, 1.0, ControllerPose::default())
        .unwrap();
    assert!(block_center(&recovered, 0).abs_diff_eq(rest_breech, 1e-6));

    let center = Vec3::new(5.5, 1.75, 2.9);
    let view = Mat4::look_at_rh(center + Vec3::new(-14.0, 6.3, 5.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-4.8, 4.8, -2.88, 2.88, 0.1, 90.0) * view;
    let render = |scene: &[Vertex]| {
        test_frame::render_with_atlas(
            scene,
            view_proj,
            FIRE_WIDTH,
            FIRE_HEIGHT,
            assets.size,
            &assets.pixels,
        )
        .expect("render public CBC firing fixture")
    };
    let before = render(&before);
    let after = render(&after);
    let changed = |a: &[u8], b: &[u8]| a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > 24);
    let cannon_pixels = changed_near(&before, &after, view_proj, rest_breech, 28, changed);
    let shell_pixels = changed_near(&before, &after, view_proj, shell_at, 24, changed);
    assert_eq!(
        cannon_pixels, 0,
        "the cannon must not move: {cannon_pixels}"
    );
    assert!(shell_pixels > 80, "shell absent from frame: {shell_pixels}");
    eprintln!("CBC firing fixture: cannon={cannon_pixels}, shell={shell_pixels} pixels");
    write_comparison(FIRE_WIDTH, FIRE_HEIGHT, &before, &after, Path::new(&output));
}

const AUTOCANNON_ID: i32 = 303;
const AUTOCANNON_BARRELS: [i32; 3] = [2, 3, 4];

fn spring_data() -> Tag {
    // `AutocannonRecoilSpringBlockEntity.write`: offsets from the spring, each
    // with the barrel's moving state (`getMovingState`: `assembled=false`).
    let barrel = Tag::compound([
        ("Name", Tag::String(AUTO_BARREL.into())),
        (
            "Properties",
            Tag::compound(
                [
                    ("assembled", "false"),
                    ("end", "nothing"),
                    ("facing", "south"),
                ]
                .map(|(key, value)| (key, Tag::String(value.into()))),
            ),
        ),
    ]);
    let rendered = [0, 1, 2, 3]
        .map(|dz| {
            Tag::compound([
                ("Pos", Tag::IntArray(vec![0, 0, dz])),
                ("Block", barrel.clone()),
            ])
        })
        .to_vec();
    Tag::compound([
        ("AnimateTicks", Tag::Int(5)),
        ("RenderedBlocks", Tag::List(rendered)),
    ])
}

fn install_autocannon(state: &mut State) {
    let mut blocks = vec![
        ([0, 0, 0], AUTO_BREECH_STATE, None),
        ([0, 0, 1], AUTO_SPRING_STATE, Some(spring_data())),
    ];
    blocks
        .extend(AUTOCANNON_BARRELS.map(|z| ([0, 0, z], AUTO_STATIONARY_BARREL_STATE, None::<Tag>)));
    install(
        state,
        AUTOCANNON_ID,
        CANNON_KIND,
        CANNON_START,
        Tag::compound([
            ("ControllerRelative", Tag::IntArray(vec![0, -2, 0])),
            ("InitialOrientation", Tag::String("SOUTH".into())),
            ("Yaw", Tag::Float(0.0f32.to_bits())),
            ("Pitch", Tag::Float(0.0f32.to_bits())),
            (
                "Contraption",
                Tag::compound([
                    (
                        "Type",
                        Tag::String("createbigcannons:mounted_autocannon".into()),
                    ),
                    ("Anchor", Tag::IntArray(vec![5, 2, 0])),
                    ("Blocks", block_list_with_data(&blocks)),
                ]),
            ),
        ]),
    );
}

/// `AutocannonRecoilSpringBlockEntity.getAnimateOffset`, written out again.
fn cbc_animate_offset(ticks: f32, partial: f32) -> f32 {
    let t = (ticks + partial) * 1.2;
    if t <= 0.0 || t >= 4.8 {
        return 1.0;
    }
    let f = if t < 1.0 { t } else { (4.8 - t) / 3.8 };
    (f * std::f32::consts::FRAC_PI_2).cos()
}

fn moving_center(pose: &VisualPose<'_>, index: usize) -> Vec3 {
    let local = pose.moving_blocks[index].local.map(|value| value as f32);
    (Mat4::from_translation(pose.anchor.as_vec3()) * pose.moving_transform)
        .transform_point3(Vec3::from_array(local) + Vec3::splat(0.5))
}

fn rod_tip(pose: &VisualPose<'_>) -> Vec3 {
    (matrix(pose) * pose.ejectors[0].matrix(pose.ejector_scale))
        .transform_point3(Vec3::new(0.5, 0.5, 1.0))
}

fn autocannon_scene(assets: &ReleaseAssets, pose: &VisualPose<'_>) -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for x in 4..=6 {
        for z in -1..=7 {
            let floor = Mat4::from_translation(Vec3::new(x as f32, -0.125, z as f32))
                * Mat4::from_scale(Vec3::new(1.0, 0.125, 1.0));
            assets.append(ANDESITE, floor, &mut vertices);
        }
    }
    assets.append(MOUNT, Mat4::from_translation(Vec3::X * 5.0), &mut vertices);
    assets.append(
        ROTATING_MOUNT,
        Mat4::from_translation(Vec3::new(5.0, 1.0, 0.0)),
        &mut vertices,
    );
    let body = matrix(pose);
    for block in pose.blocks {
        let (name, properties) = PALETTE[block.state.0 as usize - 1];
        // The stationary barrels are `assembled=true`: that model has no elements.
        if properties.contains(&("assembled", "true")) {
            continue;
        }
        let local = Vec3::from_array(block.local.map(|value| value as f32));
        assets.append(name, body * Mat4::from_translation(local), &mut vertices);
    }
    for rod in pose.ejectors {
        assets.append(
            release_assets::SPRING_ROD,
            body * rod.matrix(pose.ejector_scale),
            &mut vertices,
        );
    }
    let moving = Mat4::from_translation(pose.anchor.as_vec3()) * pose.moving_transform;
    for block in pose.moving_blocks {
        let local = Vec3::from_array(block.local.map(|value| value as f32));
        assets.append(
            AUTO_BARREL,
            moving * Mat4::from_translation(local),
            &mut vertices,
        );
    }
    vertices
}

fn autocannon_pose(state: &State, alpha: f64) -> VisualPose<'_> {
    state
        .visual(AUTOCANNON_ID, alpha, ControllerPose::default())
        .unwrap()
}

fn animate_packet(state: &mut State, id: i32) {
    let mut animate = Writer::new();
    animate.varint(2);
    animate.varint(id);
    state.apply_cannon_animation(&custom_payload(CBC_ANIMATE_CHANNEL, animate.data));
}

#[test]
#[ignore = "requires the official Create and CBC release JARs and a rendering adapter"]
fn autocannon_recoil_fixture_public_assets() {
    let create = std::env::var_os("HORIZON_CREATE_FIXTURE_JAR").expect("official Create JAR path");
    let cbc = std::env::var_os("HORIZON_CBC_FIXTURE_JAR").expect("official CBC JAR path");
    let output = std::env::var_os("HORIZON_CBC_AUTOCANNON_FIXTURE_PNG").expect("output PNG path");
    let assets = ReleaseAssets::load(Path::new(&create), Path::new(&cbc))
        .expect("release models and textures");
    let mut state = State::default();
    install_autocannon(&mut state);

    let pose = autocannon_pose;
    let rest = pose(&state, 1.0);
    assert_eq!(rest.blocks.len(), 5);
    assert_eq!(rest.moving_blocks.len(), 4);
    assert_eq!(rest.ejectors.len(), 1);
    assert_eq!(rest.ejector_scale, 1.0);
    let rest_fixed: Vec<Vec3> = (0..5).map(|index| block_center(&rest, index)).collect();
    let rest_moving: Vec<Vec3> = (0..4).map(|index| moving_center(&rest, index)).collect();
    // Moving barrels sit on the spring and on the stationary barrels.
    assert!(rest_moving[0].abs_diff_eq(rest_fixed[1], 1e-5));
    for (moving, barrel) in rest_moving[1..].iter().zip(&rest_fixed[2..]) {
        assert!(moving.abs_diff_eq(*barrel, 1e-5));
    }
    let rest_tip = rod_tip(&rest);
    let rest_moving_transform = rest.moving_transform;
    let before = autocannon_scene(&assets, &rest);

    // A packet for some other entity changes nothing.
    animate_packet(&mut state, AUTOCANNON_ID + 1000);
    assert!(
        pose(&state, 0.8)
            .moving_transform
            .abs_diff_eq(rest_moving_transform, 1e-6)
    );

    animate_packet(&mut state, AUTOCANNON_ID);
    for ticks in 0..5u8 {
        for partial in [0.0f32, 0.4, 0.8] {
            let pose = pose(&state, f64::from(partial));
            let scale = cbc_animate_offset(f32::from(ticks), partial);
            let back = (1.0 - scale) * 0.5;
            assert!((pose.ejector_scale - scale).abs() < 1e-5);
            for (index, center) in rest_fixed.iter().enumerate() {
                assert!(
                    block_center(&pose, index).abs_diff_eq(*center, 1e-5),
                    "fixed {index}"
                );
            }
            for (index, center) in rest_moving.iter().enumerate() {
                assert!(
                    moving_center(&pose, index).abs_diff_eq(*center - Vec3::Z * back, 1e-5),
                    "moving {index} at {ticks}+{partial}"
                );
            }
            // The rod shortens by exactly the distance the barrel slides back.
            assert!(rod_tip(&pose).abs_diff_eq(rest_tip - Vec3::Z * back, 1e-5));
        }
        state.tick();
    }
    let settled = pose(&state, 0.0);
    for (index, center) in rest_moving.iter().enumerate() {
        assert!(moving_center(&settled, index).abs_diff_eq(*center, 1e-6));
    }
    assert_eq!(settled.ejector_scale, 1.0);

    // The deepest point of the stroke, t = 1.0: half a block back.
    animate_packet(&mut state, AUTOCANNON_ID);
    let peak = pose(&state, 1.0 / 1.2);
    assert!(peak.ejector_scale.abs() < 1e-5);
    assert!(
        moving_center(&peak, 3).abs_diff_eq(rest_moving[3] - Vec3::Z * 0.5, 1e-5),
        "peak recoil is half a block"
    );
    assert!(block_center(&peak, 0).abs_diff_eq(rest_fixed[0], 1e-6));
    let after = autocannon_scene(&assets, &peak);

    let center = Vec3::new(5.5, 1.75, 2.9);
    let view = Mat4::look_at_rh(center + Vec3::new(-14.0, 6.3, 5.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-4.8, 4.8, -2.88, 2.88, 0.1, 90.0) * view;
    let render = |scene: &[Vertex]| {
        test_frame::render_with_atlas(
            scene,
            view_proj,
            FIRE_WIDTH,
            FIRE_HEIGHT,
            assets.size,
            &assets.pixels,
        )
        .expect("render public CBC autocannon fixture")
    };
    let before = render(&before);
    let after = render(&after);
    let changed = |a: &[u8], b: &[u8]| a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > 24);
    let breech_pixels = changed_near(&before, &after, view_proj, rest_fixed[0], 24, changed);
    let barrel_pixels = changed_near(
        &before,
        &after,
        view_proj,
        rest_moving[3] + Vec3::Z * 0.25,
        24,
        changed,
    );
    assert_eq!(breech_pixels, 0, "the breech must stay: {breech_pixels}");
    assert!(
        barrel_pixels > 100,
        "barrel must slide back: {barrel_pixels}"
    );
    eprintln!("CBC autocannon fixture: breech={breech_pixels}, barrel={barrel_pixels} pixels");
    write_comparison(FIRE_WIDTH, FIRE_HEIGHT, &before, &after, Path::new(&output));
}
