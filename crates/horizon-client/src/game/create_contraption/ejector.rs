//! The recoil spring's ejector rod: `AutocannonRecoilSpringVisual` scales the
//! per-material spring partial model along the spring's axis while it fires.

use glam::{Mat4, Quat, Vec3};
use horizon_assets::{
    bake::{BlockModelTransform, bake_block_model},
    model::Direction,
    resolve::ModelLibrary,
};
use horizon_client::{hud::Rect, mesher::Vertex};
use horizon_world::chunk::BlockState;
use std::{
    collections::HashMap,
    f32::consts::{FRAC_PI_2, PI},
    sync::{Arc, Mutex, Weak},
};

pub(super) const CAST_IRON: &str = "createbigcannons:block/autocannon/cast_iron_autocannon_spring";
pub(super) const BRONZE: &str = "createbigcannons:block/autocannon/bronze_autocannon_spring";
pub(super) const STEEL: &str = "createbigcannons:block/autocannon/steel_autocannon_spring";

#[derive(Clone, Copy)]
pub(super) struct SpringEjector {
    pub(super) local: [i32; 3],
    facing: &'static str,
    pub(super) model: &'static str,
}

impl SpringEjector {
    pub(super) fn new(local: [i32; 3], state: &BlockState) -> Option<Self> {
        let material = state
            .name
            .strip_prefix("createbigcannons:")?
            .strip_suffix("_autocannon_recoil_spring")?;
        let facing = match state.properties.get("facing")?.as_str() {
            "north" => "north",
            "south" => "south",
            "east" => "east",
            "west" => "west",
            "up" => "up",
            "down" => "down",
            _ => return None,
        };
        // `AutocannonRecoilSpringRenderer.getPartialModelForState`.
        let model = match material {
            "bronze" => BRONZE,
            "steel" => STEEL,
            _ => CAST_IRON,
        };
        Some(Self {
            local,
            facing,
            model,
        })
    }

    /// Contraption-local transform of the rod at the spring's
    /// `getAnimateOffset` `scale`: the model (native +Z) turned to the facing
    /// and shortened to `scale * 0.5 + 0.5` towards the breech end of the block.
    pub(super) fn matrix(&self, scale: f32) -> Mat4 {
        let (axis, positive, rotation) = match self.facing {
            "south" => (Vec3::Z, true, Quat::IDENTITY),
            "north" => (Vec3::Z, false, Quat::from_rotation_y(PI)),
            "east" => (Vec3::X, true, Quat::from_rotation_y(FRAC_PI_2)),
            "west" => (Vec3::X, false, Quat::from_rotation_y(-FRAC_PI_2)),
            "up" => (Vec3::Y, true, Quat::from_rotation_x(-FRAC_PI_2)),
            _ => (Vec3::Y, false, Quat::from_rotation_x(FRAC_PI_2)),
        };
        let anchor = if positive { Vec3::ZERO } else { axis };
        let stretch = Vec3::ONE + axis * (scale * 0.5 + 0.5 - 1.0);
        let local = Vec3::from_array(self.local.map(|v| v as f32));
        Mat4::from_translation(local + anchor)
            * Mat4::from_scale(stretch)
            * Mat4::from_translation(-anchor)
            * Mat4::from_translation(Vec3::splat(0.5))
            * Mat4::from_quat(rotation)
            * Mat4::from_translation(Vec3::splat(-0.5))
    }
}

pub(super) struct Quad {
    positions: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    shade: u32,
}

struct Cache {
    library: Weak<ModelLibrary>,
    atlas_generation: u64,
    models: HashMap<&'static str, Arc<[Quad]>>,
}

pub(super) fn baked(
    library: &Arc<ModelLibrary>,
    sprites: &HashMap<String, Rect>,
    atlas_generation: u64,
    model: &'static str,
) -> Option<Arc<[Quad]>> {
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let mut guard = CACHE.lock().ok()?;
    let weak = Arc::downgrade(library);
    let cache = guard.get_or_insert_with(|| Cache {
        library: weak.clone(),
        atlas_generation,
        models: HashMap::new(),
    });
    if !cache.library.ptr_eq(&weak) || cache.atlas_generation != atlas_generation {
        cache.library = weak;
        cache.atlas_generation = atlas_generation;
        cache.models.clear();
    }
    if let Some(quads) = cache.models.get(model) {
        return Some(quads.clone());
    }
    let resolved = library.resolve(model).ok()?;
    let source = bake_block_model(&resolved, BlockModelTransform::new([0; 3], false)).ok()?;
    let quads: Arc<[Quad]> = source
        .iter()
        .filter_map(|quad| {
            let name = quad.material.sprite.as_str();
            let rect = sprites.get(name.strip_prefix("minecraft:").unwrap_or(name))?;
            Some(Quad {
                positions: quad.positions,
                uv: quad.uv.map(|[u, v]| {
                    [
                        rect.u0 + u * (rect.u1 - rect.u0),
                        rect.v0 + v * (rect.v1 - rect.v0),
                    ]
                }),
                shade: match quad.facing {
                    Direction::Up => 255,
                    Direction::Down => 127,
                    Direction::North | Direction::South => 204,
                    Direction::East | Direction::West => 153,
                },
            })
        })
        .collect();
    cache.models.insert(model, quads.clone());
    Some(quads)
}

pub(super) fn append(quads: &[Quad], matrix: Mat4, light: u32, out: &mut Vec<Vertex>) {
    for quad in quads {
        let color = 0xFF00_0000 | (quad.shade << 16) | (quad.shade << 8) | quad.shade;
        out.extend((0..4).map(|i| {
            Vertex {
                pos: matrix
                    .transform_point3(Vec3::from_array(quad.positions[i]))
                    .to_array(),
                uv: quad.uv[i],
                layer: 0,
                color,
                light,
                normal: [0; 4],
                glint_uv: [0.0; 2],
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spring(name: &str, facing: &str) -> Option<SpringEjector> {
        let state = BlockState {
            name: name.to_owned(),
            properties: [("facing".to_owned(), facing.to_owned())].into(),
        };
        SpringEjector::new([2, 3, 4], &state)
    }

    #[test]
    fn the_rod_shortens_towards_the_breech_end() {
        for (facing, along, front, back) in [
            ("south", Vec3::Z, 1.0, 0.0),
            ("north", Vec3::Z, 0.0, 1.0),
            ("east", Vec3::X, 1.0, 0.0),
            ("west", Vec3::X, 0.0, 1.0),
            ("up", Vec3::Y, 1.0, 0.0),
            ("down", Vec3::Y, 0.0, 1.0),
        ] {
            let rod = spring("createbigcannons:steel_autocannon_recoil_spring", facing).unwrap();
            // The model is native +Z: its muzzle end is z = 1, its breech end z = 0.
            let at = |scale: f32, native: f32| {
                let p = rod
                    .matrix(scale)
                    .transform_point3(Vec3::new(0.5, 0.5, native));
                (p - Vec3::new(2.0, 3.0, 4.0)).dot(along)
            };
            assert!((at(1.0, 1.0) - front).abs() < 1e-6, "{facing}");
            assert!((at(1.0, 0.0) - back).abs() < 1e-6, "{facing}");
            assert!((at(0.0, 0.0) - back).abs() < 1e-6, "{facing}");
            let halfway = back + (front - back) * 0.5;
            assert!((at(0.0, 1.0) - halfway).abs() < 1e-6, "{facing}");
        }
    }

    #[test]
    fn the_partial_follows_the_spring_material() {
        let model = |name| spring(name, "south").unwrap().model;
        assert_eq!(
            model("createbigcannons:bronze_autocannon_recoil_spring"),
            BRONZE
        );
        assert_eq!(
            model("createbigcannons:cast_iron_autocannon_recoil_spring"),
            CAST_IRON
        );
        assert!(spring("createbigcannons:cast_iron_autocannon_barrel", "south").is_none());
    }
}
