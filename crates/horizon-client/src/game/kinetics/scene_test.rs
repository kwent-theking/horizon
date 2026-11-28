//! Asset-free regression scene driven by synthetic block entity updates.

use super::{
    TEMPLATE_CACHE_TESTS, create_piston, emit_template_quads, get_or_bake_template, speed_from_nbt,
};
use super::{deployer, extra_addons, flap_display, fluids, indicators, machines};
use glam::{Mat4, Vec3};
use horizon_assets::{model::CuboidModel, resolve::ModelLibrary};
use horizon_client::{
    block_entity_render::draw_text_call,
    hud::Rect,
    mesher::Vertex,
    world::{DeployerState, FlapDisplayState, NixieState, WhistleState},
};
use horizon_protocol::{
    buf::Reader,
    packets::play::{BlockEntityData, Clientbound},
    registry::Ids,
};
use horizon_world::nbt::{Tag, parse_snbt};
use std::{collections::HashMap, sync::Arc};

mod aeronautics_parts_fixture;
mod batches;
mod extra_addons_fixture;
mod release_assets;

const LIGHT: u32 = 0xf0f0;
const FIXTURE_SPRITE: &str = "horizon:block/fixture_white";
const TILE: &str = "horizon:block/fixture_floor";
const ITEM: &str = "horizon:block/fixture_item";
const WHITE: Rect = Rect {
    u0: 0.0,
    v0: 0.0,
    u1: 1.0,
    v1: 1.0,
    w: 16,
    h: 16,
};

fn block_entity_types() -> Ids {
    Ids::from_names(
        [
            "create:deployer",
            "create:fluid_tank",
            "create:basin",
            "create:spout",
            "create:item_drain",
            "create:glass_fluid_pipe",
            "create:mechanical_piston",
            "create:nixie_tube",
            "create:track_signal",
            "create:flap_display",
            "createbigcannons:cannon_mount",
        ]
        .map(str::to_owned)
        .to_vec(),
    )
}

fn wire_tag(registry: &Ids, kind: &str, pos: [i32; 3], snbt: &str) -> Tag {
    let (tag, parsed) = parse_snbt(snbt).expect("fixture SNBT");
    assert_eq!(parsed, snbt.len(), "{kind}");
    let kind_id = registry.id(kind).expect("fixture type");
    let wire = Clientbound::BlockEntityData(BlockEntityData {
        pos,
        kind: kind_id,
        tag,
    })
    .encode();
    let mut reader = Reader::new(&wire);
    let Clientbound::BlockEntityData(packet) =
        Clientbound::decode(reader.varint().unwrap(), reader.rest()).expect("wire packet")
    else {
        panic!("expected a block entity update");
    };
    assert_eq!(packet.pos, pos);
    assert_eq!(registry.name(packet.kind), Some(kind));
    packet.tag
}

struct FixtureModels {
    library: Arc<ModelLibrary>,
    sprites: HashMap<String, Rect>,
    release: Option<release_assets::ReleaseAssets>,
}

impl FixtureModels {
    fn new() -> Self {
        let partials = [
            "create:block/deployer/pole",
            "create:block/deployer/hand_pointing",
            "create:block/deployer/hand_holding",
            "create:block/mechanical_piston/normal/head",
            "create:block/mechanical_piston/sticky/head",
            "create:block/piston_extension_pole",
            "create:block/steam_whistle/medium_mouth",
            "create:block/track_signal/panel",
            "create:block/track_signal/yellow_tube",
            "createbigcannons:block/cannon_mount/rotating_mount",
            "createbigcannons:block/cannon_carriage/carriage_axle",
            "create:block/shaft",
            "create:block/shaft_half",
            ITEM,
            TILE,
        ];
        let models = partials.into_iter().map(|name| {
            let (from, to) = if name == TILE {
                ([0, 0, 0], [16, 2, 16])
            } else if name == ITEM {
                ([4, 10, 2], [12, 14, 14])
            } else {
                ([4, 5, 3], [12, 11, 13])
            };
            let faces = ["down", "up", "north", "south", "west", "east"]
                .map(|face| (face.to_owned(), serde_json::json!({"texture":"#sprite"})))
                .into_iter()
                .collect::<serde_json::Map<_, _>>();
            let json = serde_json::json!({
                "textures": {"sprite": FIXTURE_SPRITE},
                "elements": [{"from":from,"to":to,"faces":faces}],
            });
            let model = CuboidModel::from_json(&json.to_string()).expect("fixture cuboid");
            (name.to_owned(), model)
        });
        Self {
            library: Arc::new(ModelLibrary::new(models.collect()).expect("fixture models")),
            sprites: HashMap::from([(FIXTURE_SPRITE.to_owned(), WHITE)]),
            release: None,
        }
    }

    fn parts(
        &self,
        origin: Vec3,
        parts: impl IntoIterator<Item = machines::MachinePart>,
        tint: u32,
        out: &mut Vec<Vertex>,
    ) {
        for part in parts {
            let transform = Mat4::from_translation(origin) * part.local;
            if let Some(release) = &self.release {
                release.draw_part(self, transform, &part, out);
                continue;
            }
            let mesh = get_or_bake_template(&self.library, &self.sprites, 0, part.model)
                .unwrap_or_else(|| panic!("unexpected partial {}", part.model));
            assert!(!mesh.quads.is_empty(), "empty {}", part.model);
            emit_template_quads(
                &mesh,
                &self.sprites,
                transform,
                LIGHT,
                part.tint.or(Some(tint)),
                part.sprite_shift,
                part.full_bright,
                out,
            );
        }
    }
}

fn machine_rows(
    models: &FixtureModels,
    tag: &impl Fn(&str, [i32; 3], &str) -> Tag,
    after: bool,
    out: &mut Vec<Vertex>,
) {
    let position = |x| Vec3::new(x, 0.0, 5.0);
    let textured = models.release.is_some();
    let mut state = DeployerState::default();
    let mut speed = 0.0;
    if after {
        let held = if textured {
            "create:shaft"
        } else {
            "minecraft:iron_pickaxe"
        };
        let update = tag(
            "create:deployer",
            [0, 0, 5],
            &format!(
                r#"{{Speed:64f,Mode:"USE",State:"EXPANDING",Timer:500,Reach:1f,HeldItem:{{id:"{held}",count:1}}}}"#,
            ),
        );
        assert!(deployer::is_deployer_be_tag("create:deployer", &update));
        deployer::apply_deployer_tag(&mut state, &update);
        speed = speed_from_nbt(&update).unwrap();
        deployer::tick_deployer_state(&mut state, speed);
    }
    let (parts, items) = deployer::deployer_parts_and_items(&deployer::DeployerBlock {
        facing: "south",
        along_first: false,
        speed,
        time: 0.0,
        alpha: 1.0,
        deployer: after.then_some(&state),
        held_item_is_block: textured,
    });
    assert_eq!(items.len(), usize::from(after));
    assert_eq!(
        parts[1].model,
        if after {
            "create:block/deployer/hand_holding"
        } else {
            "create:block/deployer/hand_pointing"
        }
    );
    let tip = parts[1].local.transform_point3(Vec3::splat(0.5));
    assert!(tip.abs_diff_eq(Vec3::new(0.5, 0.5, if after { 1.445 } else { 0.5 }), 1e-5));
    models.parts(position(0.0), parts, 0xca8a45, out);
    for item in items {
        let (name, local) = if textured {
            assert_eq!(item.stack.item.as_ref(), "create:shaft");
            (
                "create:block/shaft",
                item.local * Mat4::from_translation(-Vec3::splat(0.5)),
            )
        } else {
            assert_eq!(item.stack.item.as_ref(), "minecraft:iron_pickaxe");
            (ITEM, item.local)
        };
        models.parts(position(0.0), [machines::part(name, local)], 0x6ae6ee, out);
    }

    for (x, block, head) in [
        (
            2,
            "create:mechanical_piston",
            "create:block/mechanical_piston/normal/head",
        ),
        (
            4,
            "create:sticky_mechanical_piston",
            "create:block/mechanical_piston/sticky/head",
        ),
    ] {
        let state = after.then(|| {
            let tag = tag(
                "create:mechanical_piston",
                [x, 0, 5],
                "{Speed:64f,ExtensionLength:2,Offset:1.25f,Running:1b}",
            );
            assert!(create_piston::is_piston_be_tag(
                "create:mechanical_piston",
                &tag
            ));
            let mut piston = create_piston::piston_state_from_nbt(None, &tag).unwrap();
            create_piston::tick_piston_state(&mut piston, "south", speed_from_nbt(&tag).unwrap());
            piston
        });
        let parts = create_piston::piston_parts(
            block,
            "south",
            after,
            state.as_ref(),
            1.0,
            Vec3::ZERO,
            16.0,
        );
        assert_eq!(parts.len(), if after { 3 } else { 0 });
        if after {
            assert_eq!(parts[0].model, head);
            assert!(
                parts[0]
                    .local
                    .transform_point3(Vec3::splat(0.5))
                    .abs_diff_eq(Vec3::new(0.5, 0.5, 1.875), 1e-5)
            );
        }
        models.parts(position(x as f32), parts, 0x9ab768, out);
    }

    let mut whistle = WhistleState::default();
    for _ in 0..4 {
        indicators::tick_whistle_state(&mut whistle, after);
    }
    let mouth = indicators::whistle_part("north", "medium", Some(&whistle), 0.0, 1.0);
    assert_eq!(
        mouth.local.transform_point3(Vec3::splat(0.5)).y > 0.7,
        after
    );
    models.parts(position(6.0), [mouth], 0xe0c450, out);

    let glyph = |ch: char| {
        models
            .release
            .as_ref()
            .map_or((Some(WHITE), 6.0), |release| release.glyph(ch))
    };
    let advance = |ch| glyph(ch).1;
    let mut display = NixieState::default();
    if after {
        indicators::apply_nixie_tag(
            &mut display,
            &tag("create:nixie_tube", [8, 0, 5], "{RedstoneStrength:12}"),
        );
        assert_eq!(display.characters, ['1', '2']);
    }
    let nixie = indicators::NixieBlock {
        block: "create:nixie_tube",
        face: "wall",
        facing: "south",
        state: after.then_some(&display),
        signal: None,
        time: 0.0,
    };
    let before_text = out.len();
    for call in indicators::nixie_text_calls(&nixie, position(8.0), &advance) {
        draw_text_call(&glyph, &call, true, out);
    }
    assert!(out.len() > before_text, "nixie text vertices");

    let signal = after.then(|| {
        indicators::track_signal_state(&tag(
            "create:track_signal",
            [10, -1, 5],
            r#"{State:"YELLOW"}"#,
        ))
    });
    let lamp = indicators::nixie_signal_parts(&indicators::NixieBlock {
        face: "floor",
        signal,
        ..nixie
    });
    assert_eq!(lamp.len(), if after { 2 } else { 0 });
    if after {
        assert_eq!(lamp[1].model, "create:block/track_signal/yellow_tube");
        assert!(lamp[1].full_bright);
    }
    models.parts(position(10.0), lamp, 0xf7dd76, out);

    let mut board = FlapDisplayState::default();
    if after {
        let packet = tag(
            "create:flap_display",
            [12, 0, 5],
            r#"{Controller:1b,XSize:1,YSize:1,GlowingLine0:1b,Dye0:"YELLOW",Display0:{Key:"Default",Sections:[{Width:28f,Cycle:"alphabet",Text:'{"text":"GO"}'}]}}"#,
        );
        assert!(flap_display::apply_flap_display_tag(&mut board, &packet, 0));
    }
    let calls = flap_display::flap_display_text_calls(
        &board,
        "south",
        position(12.0),
        LIGHT,
        20.0,
        64.0,
        &advance,
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.text.as_str())
            .collect::<String>(),
        if after { "GO" } else { "" }
    );
    for call in calls {
        assert_eq!(call.light, LIGHT);
        draw_text_call(&glyph, &call, true, out);
    }

    if textured {
        return;
    }
    let mount = after.then(|| {
        extra_addons::cannon_mount_from_nbt(
            None,
            &tag(
                "createbigcannons:cannon_mount",
                [14, 0, 5],
                "{Running:1b,CannonYaw:60f,CannonPitch:25f,PitchInterface:{Speed:-8f},YawInterface:{Speed:32f}}",
            ),
        )
        .unwrap()
    });
    let parts =
        extra_addons::cannon_mount_parts("south", "down", mount.as_ref(), 4.0, 1.0, [14, 0, 5]);
    let rotated = parts[0].local.transform_vector3(Vec3::Z);
    assert!((rotated.x - if after { -0.8660254 } else { 0.0 }).abs() < 1e-5);
    models.parts(position(14.0), parts, 0xa4a8cb, out);
}

fn render_png(mut before: Vec<Vertex>, mut after: Vec<Vertex>, path: &std::path::Path) {
    const WIDTH: u32 = 1024;
    const HEIGHT: u32 = 384;
    for vertex in &mut after {
        vertex.pos[0] += 18.0;
    }
    before.extend(after);
    let center = Vec3::new(17.0, 0.6, 2.5);
    let view = Mat4::look_at_rh(center + Vec3::new(0.0, 11.0, 30.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-18.0, 18.0, -6.75, 6.75, 0.1, 90.0) * view;
    let pixels = horizon_client::gfx::test_frame::render(&before, view_proj, WIDTH, HEIGHT)
        .expect("render Create fixture");
    compare_and_write_png(&pixels, WIDTH, HEIGHT, path);
}

fn compare_and_write_png(pixels: &[u8], width: u32, height: u32, path: &std::path::Path) {
    assert_eq!(pixels.len(), (width * height * 4) as usize);
    let mut visible = [0, 0];
    let mut changed = 0;
    for row in pixels.chunks_exact(width as usize * 4) {
        let (left, right) = row.split_at(width as usize * 2);
        for (a, b) in left.chunks_exact(4).zip(right.chunks_exact(4)) {
            visible[0] += usize::from(a[..3].iter().any(|&channel| channel > 24));
            visible[1] += usize::from(b[..3].iter().any(|&channel| channel > 24));
            changed += usize::from(a[..3].iter().zip(&b[..3]).any(|(a, b)| a.abs_diff(*b) > 24));
        }
    }
    assert!(visible.into_iter().all(|count| count > 400), "{visible:?}");
    assert!(changed > 150, "only {changed} changed pixels");

    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
    eprintln!("Create fixture: {visible:?} visible pixels, {changed} changed pixels");
}

#[test]
fn create_fixture_packet_scene() {
    let _cache = TEMPLATE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let registry = block_entity_types();
    let tag = |kind: &str, pos, snbt: &str| wire_tag(&registry, kind, pos, snbt);
    let (mut before, mut after) = fluids::scene_fixture::draw(&tag);
    let models = FixtureModels::new();
    for (updated, vertices) in [(false, &mut before), (true, &mut after)] {
        for (z, count) in [(0, 5), (5, 8)] {
            for x in 0..count {
                models.parts(
                    Vec3::new((x * 2) as f32, -0.125, z as f32),
                    [machines::part(TILE, Mat4::IDENTITY)],
                    0x637286,
                    vertices,
                );
            }
        }
        machine_rows(&models, &tag, updated, vertices);
        assert!(vertices.iter().all(|v| Vec3::from(v.pos).is_finite()));
        assert_eq!(vertices.len() % 4, 0);
    }
    assert!(after.len() > before.len());
    if let Some(path) = std::env::var_os("HORIZON_CREATE_FIXTURE_PNG") {
        render_png(before, after, std::path::Path::new(&path));
    }
}

#[test]
#[ignore = "requires the official Create release JAR and a rendering adapter"]
fn create_fixture_public_assets() {
    let _cache = TEMPLATE_CACHE_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let jar =
        std::env::var_os("HORIZON_CREATE_FIXTURE_JAR").expect("path to the official Create JAR");
    let path = std::env::var_os("HORIZON_CREATE_FIXTURE_PNG").expect("output PNG path");
    let models = release_assets::load(std::path::Path::new(&jar));
    let release = models.release.as_ref().unwrap();
    let registry = block_entity_types();
    let tag = |kind: &str, pos, snbt: &str| wire_tag(&registry, kind, pos, snbt);
    let (mut before, mut after, spout) =
        fluids::scene_fixture::draw_with_atlas(&tag, &models.sprites);

    const WIDTH: u32 = 800;
    const HEIGHT: u32 = 512;
    let center = Vec3::new(6.35, 0.45, 2.9);
    let view = Mat4::look_at_rh(center + Vec3::new(4.25, 13.0, 27.0), center, Vec3::Y);
    let view_proj = Mat4::orthographic_rh(-7.5, 7.5, -4.8, 4.8, 0.1, 90.0) * view;
    let vertices = [false, true].map(|active| {
        let mut vertices = Vec::new();
        release.draw_bodies(&models, active, &mut vertices);
        machine_rows(&models, &tag, active, &mut vertices);
        models.parts(
            Vec3::new(4.0, 1.0, 0.0),
            fluids::spout_parts(active.then_some(&spout), 0.0),
            0xffffff,
            &mut vertices,
        );
        vertices.append(if active { &mut after } else { &mut before });
        vertices
    });
    render_release_comparison(
        release,
        vertices,
        view,
        view_proj,
        [WIDTH, HEIGHT],
        std::path::Path::new(&path),
    );
}

#[test]
#[ignore = "requires the official Create release JAR and a rendering adapter"]
fn create_batches_fixture_public_assets() {
    batches::render();
}

#[test]
#[ignore = "requires the official Create add-on JARs and a rendering adapter"]
fn extra_addons_fixture_public_assets() {
    extra_addons_fixture::render();
}

#[test]
#[ignore = "requires the official Create Aeronautics, Create and vanilla JARs and a rendering adapter"]
fn aeronautics_parts_fixture_public_assets() {
    aeronautics_parts_fixture::render();
}

fn render_release_comparison(
    release: &release_assets::ReleaseAssets,
    vertices: [Vec<Vertex>; 2],
    view: Mat4,
    view_proj: Mat4,
    [width, height]: [u32; 2],
    path: &std::path::Path,
) {
    let [left, right] = vertices.map(|vertices| {
        let is_translucent =
            |quad: &[Vertex]| quad[0].layer == horizon_client::gfx::TRANSLUCENT_ITEM_OVERLAY;
        let depth = |quad: &[Vertex]| {
            quad.iter()
                .map(|vertex| view.transform_point3(Vec3::from(vertex.pos)).z)
                .sum::<f32>()
        };
        let mut quads: Vec<_> = vertices.chunks_exact(4).collect();
        quads.sort_by(|a, b| {
            let a_translucent = is_translucent(a);
            let b_translucent = is_translucent(b);
            a_translucent.cmp(&b_translucent).then_with(|| {
                if a_translucent && b_translucent {
                    depth(a).total_cmp(&depth(b))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        let vertices: Vec<_> = quads.into_iter().flatten().copied().collect();
        horizon_client::gfx::test_frame::render_with_atlas(
            &vertices,
            view_proj,
            width,
            height,
            release.size,
            &release.pixels,
        )
        .expect("render official Create assets")
    });
    let row = width as usize * 4;
    let mut comparison = Vec::with_capacity(left.len() + right.len());
    for (left, right) in left.chunks_exact(row).zip(right.chunks_exact(row)) {
        comparison.extend_from_slice(left);
        comparison.extend_from_slice(right);
    }
    compare_and_write_png(&comparison, width * 2, height, path);
}
