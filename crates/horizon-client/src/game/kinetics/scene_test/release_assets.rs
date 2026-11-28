//! Comparison scenes using model JSON and PNGs from official release JARs.

use super::{
    FixtureModels, LIGHT, emit_template_quads, get_or_bake_template, machines::MachinePart,
};
use glam::{Mat4, Vec3};
use horizon_assets::{
    bake::{BlockModelTransform, bake_block_model},
    blockstate::Variant,
    model::{CuboidModel, Direction},
    resolve::ModelLibrary,
};
use horizon_client::{
    gfx::TRANSLUCENT_ITEM_OVERLAY,
    hud::{Rect, decode_bytes},
    mesher::{CULL_BACK, Vertex},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    io::{Read, Seek},
    path::Path,
    sync::Arc,
};
use zip::ZipArchive;

pub(super) type Jars<R = File> = HashMap<&'static str, ZipArchive<R>>;

const PARTS: &[&str] = &[
    "create:block/deployer/pole",
    "create:block/deployer/hand_pointing",
    "create:block/deployer/hand_holding",
    "create:block/mechanical_piston/normal/head",
    "create:block/mechanical_piston/sticky/head",
    "create:block/piston_extension_pole",
    "create:block/shaft",
    "create:block/steam_whistle/medium_mouth",
    "create:block/track_signal/panel",
    "create:block/track_signal/yellow_tube",
    "create:block/spout/top",
    "create:block/spout/middle",
    "create:block/spout/bottom",
];

const FLUIDS: &[&str] = &[
    "create:fluid/honey_still",
    "create:fluid/honey_flow",
    "create:fluid/chocolate_still",
    "create:fluid/chocolate_flow",
    "create:fluid/potion_still",
    "create:fluid/potion_flow",
];

const GLYPHS: [(char, [u8; 7]); 5] = [
    ('0', [14, 17, 19, 21, 25, 17, 14]),
    ('1', [4, 12, 4, 4, 4, 4, 14]),
    ('2', [14, 17, 1, 2, 4, 8, 31]),
    ('G', [14, 17, 16, 23, 17, 17, 14]),
    ('O', [14, 17, 17, 17, 17, 17, 14]),
];

#[derive(Deserialize)]
struct VariantBlockState {
    variants: BTreeMap<String, Box<RawValue>>,
}

#[derive(Deserialize)]
struct MultipartBlockState {
    multipart: Vec<Multipart>,
}

#[derive(Deserialize)]
struct Multipart {
    when: BTreeMap<String, serde_json::Value>,
    apply: Box<RawValue>,
}

pub(super) struct Placement {
    transform: Mat4,
    variant: Variant,
}

pub(super) struct ReleaseAssets {
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
    glyphs: HashMap<char, Rect>,
    translucent: BTreeSet<String>,
    placements: [Vec<Placement>; 2],
}

pub(super) fn read(jar: &mut ZipArchive<impl Read + Seek>, name: &str) -> Vec<u8> {
    let mut entry = jar.by_name(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut data = Vec::new();
    entry.read_to_end(&mut data).unwrap();
    data
}

fn load_model<R: Read + Seek>(
    jars: &mut Jars<R>,
    models: &mut BTreeMap<String, CuboidModel>,
    id: &str,
) {
    if models.contains_key(id) {
        return;
    }
    let (namespace, path) = id.split_once(':').unwrap();
    let source = jars
        .get_mut(namespace)
        .unwrap_or_else(|| panic!("no JAR for model {id}"));
    let data = read(source, &format!("assets/{namespace}/models/{path}.json"));
    let mut model = CuboidModel::from_json(std::str::from_utf8(&data).unwrap())
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    let jars_ref = RefCell::new(&mut *jars);
    model
        .attach_obj(&|namespace, path| {
            let mut jars = jars_ref.borrow_mut();
            let archive = jars.get_mut(namespace)?;
            let mut entry = archive
                .by_name(&format!("assets/{namespace}/{path}"))
                .ok()?;
            let mut text = String::new();
            entry.read_to_string(&mut text).ok()?;
            Some(text)
        })
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    let parent = model.parent.clone();
    models.insert(id.to_owned(), model);
    if let Some(parent) = parent {
        load_model(jars, models, &parent);
    }
}

fn placements(jar: &mut ZipArchive<File>, active: bool) -> Vec<Placement> {
    let piston = if active {
        "axis_along_first=false,facing=south,state=moving"
    } else {
        "axis_along_first=false,facing=south,state=retracted"
    };
    let whistle = if active {
        "facing=north,powered=true,size=medium,wall=false"
    } else {
        "facing=north,powered=false,size=medium,wall=false"
    };
    [
        (
            "fluid_tank",
            "bottom=true,shape=window,top=true",
            [0.0, 0.0, 0.0],
        ),
        ("basin", "facing=down", [2.0, 0.0, 0.0]),
        ("spout", "", [4.0, 1.0, 0.0]),
        ("item_drain", "", [6.0, 0.0, 0.0]),
        ("glass_fluid_pipe", "alt=false,axis=x", [8.0, 0.0, 0.0]),
        (
            "deployer",
            "axis_along_first=false,facing=south",
            [0.0, 0.0, 5.0],
        ),
        ("mechanical_piston", piston, [2.0, 0.0, 5.0]),
        ("sticky_mechanical_piston", piston, [4.0, 0.0, 5.0]),
        ("steam_whistle", whistle, [6.0, 0.0, 5.0]),
        (
            "nixie_tube",
            "double_face=wall,facing=south,waterlogged=false",
            [8.0, 0.0, 5.0],
        ),
        (
            "track_signal",
            "powered=false,type=entry_signal",
            [10.0, -1.0, 5.0],
        ),
        (
            "nixie_tube",
            "double_face=floor,facing=south,waterlogged=false",
            [10.0, 0.0, 5.0],
        ),
        (
            "display_board",
            "down=false,facing=south,up=false,waterlogged=false",
            [12.0, 0.0, 5.0],
        ),
    ]
    .into_iter()
    .map(|(block, key, at)| {
        let block = format!("create:{block}");
        let state: VariantBlockState = blockstate(jar, &block);
        let variant = state
            .variants
            .get(key)
            .unwrap_or_else(|| panic!("{block} [{key}]: no variant"));
        let variant =
            Variant::from_json(variant.get()).unwrap_or_else(|e| panic!("{block} [{key}]: {e}"));
        Placement {
            transform: Mat4::from_translation(at.into()),
            variant,
        }
    })
    .collect()
}

fn texture<R: Read + Seek>(jars: &mut Jars<R>, name: &str) -> (u32, u32, Vec<u8>) {
    if name == "minecraft:missingno" || name == "missingno" {
        // MissingTextureAtlasSprite: 16x16, 8x8 squares of 0xF800F8 and black,
        // so an unresolved texture shows in the picture as it would in game.
        let rgba = (0..16 * 16)
            .flat_map(|i| {
                let (x, y) = (i % 16, i / 16);
                if (x < 8) == (y < 8) {
                    [0xF8, 0x00, 0xF8, 0xFF]
                } else {
                    [0x00, 0x00, 0x00, 0xFF]
                }
            })
            .collect();
        return (16, 16, rgba);
    }
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    let (namespace, path) = if namespace == "minecraft" && !jars.contains_key(namespace) {
        let stand_in = match path {
            "block/piston_side" => "block/andesite_casing_piston",
            "block/piston_top" => "block/piston_bottom",
            "block/piston_top_sticky" => "block/mechanical_press_head",
            _ => panic!("no vanilla JAR for sprite {name}"),
        };
        ("create", stand_in)
    } else {
        (namespace, path)
    };
    let jar = jars
        .get_mut(namespace)
        .unwrap_or_else(|| panic!("no JAR for sprite {name}"));
    let (width, mut height, mut rgba) = decode_bytes(&read(
        jar,
        &format!("assets/{namespace}/textures/{path}.png"),
    ))
    .unwrap_or_else(|_| panic!("invalid PNG sprite {name}"));
    if namespace == "create" && path.starts_with("fluid/") && height > width {
        // These pinned Create sprites start at frame zero and have square frames.
        height = width;
        rgba.truncate((width * height * 4) as usize);
    }
    (width, height, rgba)
}

pub(super) fn load(path: &Path) -> FixtureModels {
    let mut jar = open(path);
    let scene = [placements(&mut jar, false), placements(&mut jar, true)];
    load_scene(
        &mut [("create", jar)].into(),
        scene,
        PARTS.iter().copied(),
        FLUIDS.iter().copied(),
    )
}

pub(super) fn open(path: &Path) -> ZipArchive<File> {
    let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    ZipArchive::new(file).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub(super) fn block_body(
    jar: &mut ZipArchive<impl Read + Seek>,
    block: &str,
    props: &[(&str, &str)],
    transform: Mat4,
) -> Placement {
    let state: VariantBlockState = blockstate(jar, block);
    let (key, variant) = state
        .variants
        .iter()
        .find(|(key, _)| {
            props.iter().all(|&(name, value)| {
                key.split(',')
                    .any(|term| term.split_once('=') == Some((name, value)))
            })
        })
        .or_else(|| state.variants.get_key_value(""))
        .unwrap_or_else(|| panic!("{block} {props:?}: no variant"));
    let variant = if variant.get().starts_with('[') {
        serde_json::from_str::<Vec<&RawValue>>(variant.get())
            .unwrap_or_else(|e| panic!("{block} [{key}]: {e}"))[0]
    } else {
        variant
    };
    let variant =
        Variant::from_json(variant.get()).unwrap_or_else(|e| panic!("{block} [{key}]: {e}"));
    Placement { transform, variant }
}

fn blockstate<T: DeserializeOwned>(jar: &mut ZipArchive<impl Read + Seek>, block: &str) -> T {
    let (namespace, name) = block.split_once(':').unwrap();
    serde_json::from_slice(&read(
        jar,
        &format!("assets/{namespace}/blockstates/{name}.json"),
    ))
    .unwrap_or_else(|e| panic!("{block}: {e}"))
}

/// The Auger Cogwheel has separate multipart casing and connector models.
pub(super) fn multipart_bodies(
    jar: &mut ZipArchive<impl Read + Seek>,
    block: &str,
    props: &[(&str, &str)],
    transform: Mat4,
) -> Vec<Placement> {
    let state: MultipartBlockState = blockstate(jar, block);
    state
        .multipart
        .into_iter()
        .filter(|part| {
            part.when.iter().all(|(key, values)| {
                props.iter().any(|(name, value)| {
                    *name == key.as_str()
                        && values
                            .as_str()
                            .unwrap()
                            .split('|')
                            .any(|term| term == *value)
                })
            })
        })
        .map(|part| Placement {
            transform,
            variant: Variant::from_json(part.apply.get())
                .unwrap_or_else(|e| panic!("{block} multipart apply: {e}")),
        })
        .collect()
}

pub(super) fn load_scene<'a, R: Read + Seek>(
    jars: &mut Jars<R>,
    scene: [Vec<Placement>; 2],
    parts: impl IntoIterator<Item = &'a str>,
    sprites: impl IntoIterator<Item = &'a str>,
) -> FixtureModels {
    let mut models = BTreeMap::new();
    if !jars.contains_key("minecraft") {
        models.insert(
            "minecraft:block/block".to_owned(),
            CuboidModel::from_json("{}").unwrap(),
        );
    }
    let models_used: BTreeSet<String> = parts
        .into_iter()
        .map(str::to_owned)
        .chain(scene.iter().flatten().map(|p| p.variant.model.clone()))
        .collect();
    for id in &models_used {
        load_model(jars, &mut models, id);
    }
    let library = Arc::new(ModelLibrary::new(models).unwrap());
    let mut referenced = BTreeSet::new();
    for id in &models_used {
        let model = library.resolve(id).unwrap();
        let quads = bake_block_model(&model, BlockModelTransform::new([0; 3], false)).unwrap();
        referenced.extend(quads.into_iter().map(|quad| quad.material.sprite.clone()));
    }
    referenced.extend(sprites.into_iter().map(str::to_owned));
    let mut translucent = BTreeSet::new();
    let mut images = Vec::new();
    for name in referenced {
        let (w, h, rgba) = texture(jars, &name);
        if rgba.chunks_exact(4).any(|p| (1..255).contains(&p[3])) {
            translucent.insert(name.clone());
        }
        images.push((name, w, h, rgba));
    }
    for (ch, rows) in GLYPHS {
        let mut rgba = vec![0; 8 * 8 * 4];
        for (y, bits) in rows.into_iter().enumerate() {
            for x in 0..5 {
                if bits & (16 >> x) != 0 {
                    let at = (y * 8 + x + 1) * 4;
                    rgba[at..at + 4].fill(255);
                }
            }
        }
        images.push((format!("glyph/{ch}"), 8, 8, rgba));
    }

    let cell = images
        .iter()
        .map(|(_, w, h, _)| (*w).max(*h))
        .max()
        .unwrap()
        + 2;
    // A square-ish grid keeps the atlas within the GPU's 8192 limit when a
    // large sprite sets the cell size.
    let columns = (images.len() as f64).sqrt().ceil().max(1.0) as u32;
    let size = [
        cell * columns,
        (images.len() as u32).div_ceil(columns) * cell,
    ];
    let mut pixels = vec![0; (size[0] * size[1] * 4) as usize];
    let mut sprites = HashMap::new();
    let mut glyphs = HashMap::new();
    for (index, (name, w, h, rgba)) in images.into_iter().enumerate() {
        let x = (index as u32 % columns) * cell + 1;
        let y = (index as u32 / columns) * cell + 1;
        for dy in 0..h {
            let at = (((y + dy) * size[0] + x) * 4) as usize;
            let source = (dy * w * 4) as usize;
            pixels[at..at + (w * 4) as usize]
                .copy_from_slice(&rgba[source..source + (w * 4) as usize]);
        }
        let rect = Rect {
            u0: (x as f32 + 0.5) / size[0] as f32,
            v0: (y as f32 + 0.5) / size[1] as f32,
            u1: ((x + w) as f32 - 0.5) / size[0] as f32,
            v1: ((y + h) as f32 - 0.5) / size[1] as f32,
            w,
            h,
        };
        if let Some(ch) = name.strip_prefix("glyph/") {
            glyphs.insert(ch.chars().next().unwrap(), rect);
        } else {
            sprites.insert(name, rect);
        }
    }
    FixtureModels {
        library,
        sprites,
        release: Some(ReleaseAssets {
            size,
            pixels,
            glyphs,
            translucent,
            placements: scene,
        }),
    }
}

impl ReleaseAssets {
    pub(super) fn glyph(&self, ch: char) -> (Option<Rect>, f32) {
        (self.glyphs.get(&ch).copied(), 6.0)
    }

    pub(super) fn draw_bodies(&self, models: &FixtureModels, after: bool, out: &mut Vec<Vertex>) {
        for Placement {
            transform: local,
            variant,
        } in &self.placements[usize::from(after)]
        {
            let model = models.library.resolve(&variant.model).unwrap();
            let transform = BlockModelTransform::new(variant.rotations, variant.uv_lock);
            let quads = bake_block_model(&model, transform).unwrap();
            for quad in quads {
                let sprite = &quad.material.sprite;
                let rect = &models.sprites[sprite];
                let layer = if quad.material.force_translucent || self.translucent.contains(sprite)
                {
                    TRANSLUCENT_ITEM_OVERLAY
                } else {
                    0
                };
                let shade = match quad.facing {
                    Direction::Up => 255,
                    Direction::Down => 127,
                    Direction::North | Direction::South => 204,
                    Direction::East | Direction::West => 153,
                };
                for (point, [u, v]) in quad.positions.into_iter().zip(quad.uv) {
                    out.push(Vertex {
                        pos: local.transform_point3(Vec3::from_array(point)).to_array(),
                        uv: [
                            rect.u0 + u * (rect.u1 - rect.u0),
                            rect.v0 + v * (rect.v1 - rect.v0),
                        ],
                        light: LIGHT,
                        layer,
                        color: u32::from_le_bytes([shade, shade, shade, 255]),
                        normal: CULL_BACK,
                        glint_uv: [0.0; 2],
                    });
                }
            }
        }
    }

    pub(super) fn draw_part(
        &self,
        models: &FixtureModels,
        transform: Mat4,
        part: &MachinePart,
        out: &mut Vec<Vertex>,
    ) {
        let template = get_or_bake_template(&models.library, &models.sprites, 0, part.model)
            .unwrap_or_else(|| panic!("missing original partial {}", part.model));
        assert!(!template.quads.is_empty(), "{}", part.model);
        let start = out.len();
        emit_template_quads(
            &template,
            &models.sprites,
            transform,
            LIGHT,
            part.tint,
            part.sprite_shift,
            part.full_bright,
            out,
        );
        for (quad, vertices) in template.quads.iter().zip(out[start..].chunks_exact_mut(4)) {
            if self.translucent.contains(&quad.sprite) {
                for vertex in vertices {
                    vertex.layer = TRANSLUCENT_ITEM_OVERLAY;
                }
            }
        }
    }
}

#[test]
fn release_fixture_rotations_wrap_to_quarter_turns() {
    use std::io::{Cursor, Write};

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "assets/test/blockstates/body.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer
        .write_all(
            br#"{
                "variants":{
                    "facing=north":[{"model":"test:block/body","x":-90,"y":450,"uvlock":true}],
                    "facing=south":{"model":"test:block/body","x":270,"y":90,"uvlock":true}
                },
                "multipart":[
                    {"when":{"facing":"north"},"apply":{"model":"test:block/body","x":-90,"y":450,"uvlock":true}}
                ]
            }"#,
        )
        .unwrap();
    writer
        .start_file(
            "assets/test/models/block/body.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer
        .write_all(
            br##"{"elements":[{"from":[1,2,3],"to":[5,7,9],
                "faces":{"north":{"texture":"#missing"}}}]}"##,
        )
        .unwrap();
    let mut jar = ZipArchive::new(writer.finish().unwrap()).unwrap();
    let [wrapped, canonical] = ["north", "south"]
        .map(|facing| block_body(&mut jar, "test:body", &[("facing", facing)], Mat4::IDENTITY));
    assert_eq!(wrapped.variant, canonical.variant);
    assert_eq!(wrapped.variant.rotations, [3, 1, 0]);
    assert!(wrapped.variant.uv_lock);

    let multipart = multipart_bodies(
        &mut jar,
        "test:body",
        &[("facing", "north")],
        Mat4::IDENTITY,
    );
    assert_eq!(multipart.len(), 1);
    assert_eq!(multipart[0].variant, canonical.variant);

    let models = load_scene(
        &mut [("test", jar)].into(),
        [vec![wrapped], vec![canonical]],
        [],
        [],
    );
    let release = models.release.as_ref().unwrap();
    let [wrapped, canonical] = [false, true].map(|after| {
        let mut vertices = Vec::new();
        release.draw_bodies(&models, after, &mut vertices);
        vertices
    });
    assert_eq!(wrapped.len(), 4);
    assert_eq!(
        bytemuck::cast_slice::<Vertex, u8>(&wrapped),
        bytemuck::cast_slice::<Vertex, u8>(&canonical)
    );
}
