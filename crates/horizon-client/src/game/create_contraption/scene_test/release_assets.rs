//! Model and texture inputs read directly from the two pinned release archives.

use glam::{Mat4, Vec3};
use horizon_assets::{
    bake::{BlockModelTransform, bake_block_model},
    blockstate::Variant,
    model::{CuboidModel, Direction},
    resolve::ModelLibrary,
};
use horizon_client::{
    Result,
    gfx::TRANSLUCENT_ITEM_OVERLAY,
    hud::{Rect, decode_bytes},
    mesher::{CULL_BACK, Vertex},
};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};
use zip::ZipArchive;

pub(super) const PISTON: &str = "create:mechanical_piston";
pub(super) const PISTON_HEAD: &str = "create:mechanical_piston_head";
pub(super) const PISTON_POLE: &str = "create:piston_extension_pole";
pub(super) const CHASSIS: &str = "create:linear_chassis";
pub(super) const ANDESITE: &str = "create:andesite_casing";
pub(super) const BRASS: &str = "create:brass_casing";
pub(super) const MOUNT: &str = "createbigcannons:cannon_mount";
pub(super) const ROTATING_MOUNT: &str = "createbigcannons:block/cannon_mount/rotating_mount";
pub(super) const BREECH: &str = "createbigcannons:cast_iron_sliding_breech";
pub(super) const CHAMBER: &str = "createbigcannons:cast_iron_cannon_chamber";
pub(super) const BARREL: &str = "createbigcannons:cast_iron_cannon_barrel";
pub(super) const SHELL: &str = "createbigcannons:he_shell";
pub(super) const AUTO_BREECH: &str = "createbigcannons:cast_iron_autocannon_breech";
pub(super) const AUTO_SPRING: &str = "createbigcannons:cast_iron_autocannon_recoil_spring";
pub(super) const AUTO_BARREL: &str = "createbigcannons:cast_iron_autocannon_barrel";
/// The stationary barrel state (`assembled=true`) has no elements of its own.
pub(super) const AUTO_BARREL_ASSEMBLED: &str =
    "createbigcannons:cast_iron_autocannon_barrel[assembled=true]";
/// The recoil spring's ejector partial model.
pub(super) const SPRING_ROD: &str = super::super::ejector::CAST_IRON;
const FIXTURE_WHITE: &str = "horizon:fixture_white";

const STATES: &[(&str, &str)] = &[
    (PISTON, "axis_along_first=false,facing=south,state=moving"),
    (PISTON_HEAD, "facing=south,type=normal"),
    (PISTON_POLE, "facing=south"),
    (CHASSIS, "axis=z,sticky_bottom=false,sticky_top=false"),
    (ANDESITE, ""),
    (BRASS, ""),
    (
        MOUNT,
        "assembly_powered=false,facing=south,fire_powered=false,vertical_direction=down",
    ),
    (BREECH, "axis_along_first=true,facing=south"),
    (CHAMBER, "facing=south"),
    (BARREL, "facing=south"),
    (SHELL, "facing=north,waterlogged=false"),
];

/// Fixture name, block and blockstate variant key of the autocannon states.
const AUTOCANNON_STATES: &[(&str, &str, &str)] = &[
    (AUTO_BREECH, AUTO_BREECH, "facing=south,handle=false"),
    (AUTO_SPRING, AUTO_SPRING, "facing=south"),
    (
        AUTO_BARREL,
        AUTO_BARREL,
        "assembled=false,end=nothing,facing=south",
    ),
    (
        AUTO_BARREL_ASSEMBLED,
        AUTO_BARREL,
        "assembled=true,end=nothing,facing=south",
    ),
];

#[derive(Deserialize)]
struct BlockState<'a> {
    #[serde(borrow)]
    variants: BTreeMap<String, &'a RawValue>,
}

struct Releases {
    create: ZipArchive<File>,
    cbc: ZipArchive<File>,
}

impl Releases {
    fn read(&mut self, id: &str, kind: &str, ext: &str) -> Result<Vec<u8>> {
        let (namespace, name) = id.split_once(':').ok_or("unqualified fixture asset")?;
        let archive = match namespace {
            "create" => &mut self.create,
            "createbigcannons" => &mut self.cbc,
            _ => return Err(format!("unexpected fixture asset {id}").into()),
        };
        let path = format!("assets/{namespace}/{kind}/{name}.{ext}");
        let mut entry = archive.by_name(&path).map_err(|e| format!("{path}: {e}"))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn variant(&mut self, block: &str, key: &str) -> Result<Variant> {
        let bytes = self.read(block, "blockstates", "json")?;
        let state: BlockState<'_> = serde_json::from_slice(&bytes)?;
        let variant = state
            .variants
            .get(key)
            .ok_or_else(|| format!("{block} [{key}]: no variant"))?;
        Variant::from_json(variant.get()).map_err(|e| format!("{block} [{key}]: {e}").into())
    }

    fn load_model(&mut self, id: &str, models: &mut BTreeMap<String, CuboidModel>) -> Result<()> {
        if models.contains_key(id) {
            return Ok(());
        }
        let bytes = self.read(id, "models", "json")?;
        let model = CuboidModel::from_json(std::str::from_utf8(&bytes)?)
            .map_err(|e| format!("{id}: {e}"))?;
        let parent = model.parent.clone();
        models.insert(id.to_owned(), model);
        if let Some(parent) = parent {
            self.load_model(&parent, models)?;
        }
        Ok(())
    }
}

/// Generic unit-box parents required by the official models. The children
/// supply all textures, cuboid coordinates and variant rotations as usual.
fn cube_parents() -> Result<BTreeMap<String, CuboidModel>> {
    let mut models = BTreeMap::from([(
        "minecraft:block/block".to_owned(),
        CuboidModel::from_json("{}")?,
    )]);
    for (id, bottom, top, side) in [
        ("minecraft:block/cube_all", "#all", "#all", "#all"),
        ("minecraft:block/cube_column", "#end", "#end", "#side"),
        (
            "minecraft:block/cube_bottom_top",
            "#bottom",
            "#top",
            "#side",
        ),
    ] {
        let faces = ["down", "up", "north", "south", "west", "east"]
            .into_iter()
            .zip([bottom, top, side, side, side, side])
            .map(|(face, texture)| (face.to_owned(), serde_json::json!({"texture":texture})))
            .collect::<serde_json::Map<_, _>>();
        let model = serde_json::json!({
            "elements": [{"from":[0,0,0],"to":[16,16,16],"faces":faces}],
        });
        models.insert(id.to_owned(), CuboidModel::from_json(&model.to_string())?);
    }
    Ok(models)
}

pub(super) struct ReleaseAssets {
    pub(super) size: [u32; 2],
    pub(super) pixels: Vec<u8>,
    meshes: BTreeMap<&'static str, Vec<Vertex>>,
}

impl ReleaseAssets {
    pub(super) fn load(create: &Path, cbc: &Path) -> Result<Self> {
        let mut releases = Releases {
            create: ZipArchive::new(File::open(create)?)?,
            cbc: ZipArchive::new(File::open(cbc)?)?,
        };
        let mut variants = BTreeMap::new();
        for &(block, state) in STATES {
            variants.insert(block, releases.variant(block, state)?);
        }
        for &(name, block, state) in AUTOCANNON_STATES {
            variants.insert(name, releases.variant(block, state)?);
        }
        variants.insert(
            SPRING_ROD,
            Variant {
                model: SPRING_ROD.to_owned(),
                rotations: [0; 3],
                uv_lock: false,
            },
        );
        variants.insert(
            ROTATING_MOUNT,
            Variant {
                model: ROTATING_MOUNT.to_owned(),
                rotations: [0; 3],
                uv_lock: false,
            },
        );
        let mut models = cube_parents()?;
        for variant in variants.values() {
            releases.load_model(&variant.model, &mut models)?;
        }
        let library = ModelLibrary::new(models)?;
        let mut baked = BTreeMap::new();
        let mut referenced = BTreeSet::new();
        for (block, variant) in variants {
            let transform = BlockModelTransform::new(variant.rotations, variant.uv_lock);
            let model = library.resolve(&variant.model)?;
            let quads = bake_block_model(&model, transform)?;
            if quads.is_empty() && block != AUTO_BARREL_ASSEMBLED {
                return Err(format!("{} has no fixture geometry", variant.model).into());
            }
            referenced.extend(quads.iter().map(|quad| quad.material.sprite.clone()));
            baked.insert(block, quads);
        }

        let mut sprites = vec![(FIXTURE_WHITE.to_owned(), 1, 1, vec![255; 4], false)];
        for name in referenced {
            // The piston models reference these vanilla textures. Keep the
            // original atlas slots and fill them with public Create sprites.
            let texture = match name.as_str() {
                "minecraft:block/piston_side" => "create:block/andesite_casing_piston",
                "minecraft:block/piston_top" => "create:block/piston_bottom",
                "minecraft:block/piston_top_sticky" => "create:block/mechanical_press_head",
                _ => &name,
            };
            let (w, h, pixels) = decode_bytes(&releases.read(texture, "textures", "png")?)?;
            let translucent = pixels
                .chunks_exact(4)
                .any(|pixel| (1..255).contains(&pixel[3]));
            sprites.push((name, w, h, pixels, translucent));
        }
        let cell = sprites
            .iter()
            .map(|(_, w, h, _, _)| (*w).max(*h))
            .max()
            .ok_or("no fixture sprites")?
            + 2;
        let size = [8 * cell, (sprites.len() as u32).div_ceil(8) * cell];
        let mut pixels = vec![0; (size[0] * size[1] * 4) as usize];
        let mut atlas = BTreeMap::new();
        for (i, (name, w, h, source, translucent)) in sprites.into_iter().enumerate() {
            let x = (i as u32 % 8) * cell + 1;
            let y = (i as u32 / 8) * cell + 1;
            let sprite_row = w as usize * 4;
            for (dy, row) in source.chunks_exact(sprite_row).enumerate() {
                let offset = ((y as usize + dy) * size[0] as usize + x as usize) * 4;
                pixels[offset..offset + sprite_row].copy_from_slice(row);
            }
            let rect = Rect {
                u0: (x as f32 + 0.5) / size[0] as f32,
                v0: (y as f32 + 0.5) / size[1] as f32,
                u1: ((x + w) as f32 - 0.5) / size[0] as f32,
                v1: ((y + h) as f32 - 0.5) / size[1] as f32,
                w,
                h,
            };
            atlas.insert(name, (rect, translucent));
        }

        let meshes = baked
            .into_iter()
            .map(|(block, quads)| {
                let mut vertices = Vec::with_capacity(quads.len() * 4);
                for quad in quads {
                    let (rect, translucent) = atlas[&quad.material.sprite];
                    let layer = if translucent || quad.material.force_translucent {
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
                    for (pos, [u, v]) in quad.positions.into_iter().zip(quad.uv) {
                        vertices.push(Vertex {
                            pos,
                            uv: [
                                rect.u0 + u * (rect.u1 - rect.u0),
                                rect.v0 + v * (rect.v1 - rect.v0),
                            ],
                            layer,
                            color: u32::from_le_bytes([shade, shade, shade, 255]),
                            light: 0xf0f0,
                            normal: CULL_BACK,
                            glint_uv: [0.0; 2],
                        });
                    }
                }
                (block, vertices)
            })
            .collect();

        Ok(Self {
            size,
            pixels,
            meshes,
        })
    }

    pub(super) fn append(&self, block: &str, matrix: Mat4, out: &mut Vec<Vertex>) {
        let vertices = self
            .meshes
            .get(block)
            .unwrap_or_else(|| panic!("unknown fixture block {block}"));
        out.extend(vertices.iter().map(|&vertex| {
            Vertex {
                pos: matrix
                    .transform_point3(Vec3::from_array(vertex.pos))
                    .to_array(),
                ..vertex
            }
        }));
    }
}
