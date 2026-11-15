//! Bedrock / GeckoLib `.geo.json` (format_version 1.12.0) into the vanilla `ModelPart`
//! layout that `mobs.rs` (`Part`, `Polygon`) consumes.
//!
//! Conventions, matching `VanillaEntityModelExporter.java`:
//! - Lengths are model pixels (1/16 block); `render.rs` divides poses and vertices by 16.
//! - Axes are Java entity-model axes: +Y points down and the feet sit at y = 24.
//!   Bedrock Y-up maps as `y_java = 24 - y_bedrock`, X and Z unchanged. Check: the vanilla
//!   HumanoidModel right arm pivot is (-5, 2, 0), and Bedrock's is (-5, 22, 0).
//! - Rotations: the Y flip is a reflection, which negates rotations about X and Z but not
//!   about Y, so Bedrock degrees (x, y, z) become Java radians (-x, y, -z). The Euler order
//!   stays ZYX as in `render.rs` `part_matrix`.
//! - UVs are normalized (texel / texture size), like `ModelPart.Vertex`.
//! - Quad vertex order makes `cross(v1 - v0, v2 - v0)` equal the outward normal, the same
//!   rule `mobs.rs` uses for legacy polygons without stored normals.

use serde_json::Value;
use std::collections::HashMap;

/// Java model-space y of the Bedrock origin (the feet).
const GROUND_Y: f32 = 24.0;
const DEFAULT_TEXTURE_SIZE: u32 = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct GeoPolygon {
    pub normal: [f32; 3],
    pub vertices: [[f32; 5]; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeoPart {
    pub name: String,
    /// x, y, z, xRot, yRot, zRot, xScale, yScale, zScale
    pub pose: [f32; 9],
    pub polygons: Vec<GeoPolygon>,
    pub children: Vec<GeoPart>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeoModel {
    pub roots: Vec<GeoPart>,
    pub texture_width: u32,
    pub texture_height: u32,
}

type Vec3 = [f32; 3];
type FaceUv = ([f32; 2], [f32; 2]);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Face {
    East,
    West,
    North,
    South,
    Up,
    Down,
}

const FACES: [Face; 6] = [
    Face::East,
    Face::West,
    Face::North,
    Face::South,
    Face::Up,
    Face::Down,
];

/// Outward normal plus the directions of the texture's right and up on the face, in Bedrock axes.
struct FaceFrame {
    normal: Vec3,
    right: Vec3,
    up: Vec3,
}

fn face_frame(face: Face) -> FaceFrame {
    match face {
        Face::East => FaceFrame {
            normal: [1.0, 0.0, 0.0],
            right: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        },
        Face::West => FaceFrame {
            normal: [-1.0, 0.0, 0.0],
            right: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
        },
        Face::North => FaceFrame {
            normal: [0.0, 0.0, -1.0],
            right: [-1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        },
        Face::South => FaceFrame {
            normal: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        },
        Face::Up => FaceFrame {
            normal: [0.0, 1.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, -1.0],
        },
        Face::Down => FaceFrame {
            normal: [0.0, -1.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        },
    }
}

fn face_from_name(name: &str) -> Option<Face> {
    match name {
        "east" => Some(Face::East),
        "west" => Some(Face::West),
        "north" => Some(Face::North),
        "south" => Some(Face::South),
        "up" => Some(Face::Up),
        "down" => Some(Face::Down),
        _ => None,
    }
}

/// Standard box unwrap: east | north | west | south across the top, up | down below.
/// Returns the texture origin and size of one face, in texels.
fn box_uv(face: Face, origin: [f32; 2], size: Vec3) -> FaceUv {
    let [u, v] = origin;
    let [w, h, d] = size;
    match face {
        Face::East => ([u, v + d], [d, h]),
        Face::North => ([u + d, v + d], [w, h]),
        Face::West => ([u + d + w, v + d], [d, h]),
        Face::South => ([u + 2.0 * d + w, v + d], [w, h]),
        Face::Up => ([u + d, v], [w, d]),
        Face::Down => ([u + d + w, v], [w, d]),
    }
}

#[derive(Debug)]
enum CubeUv {
    Box([f32; 2]),
    Faces(Vec<(Face, FaceUv)>),
}

#[derive(Debug)]
struct Cube {
    origin: Vec3,
    size: Vec3,
    inflate: f32,
    mirror: Option<bool>,
    pivot: Option<Vec3>,
    rotation: Option<Vec3>,
    uv: CubeUv,
}

#[derive(Debug)]
struct Bone {
    name: String,
    parent: Option<String>,
    pivot: Vec3,
    rotation: Vec3,
    mirror: bool,
    cubes: Vec<Cube>,
}

fn numbers<const N: usize>(value: &Value) -> Result<[f32; N], String> {
    let items = value
        .as_array()
        .filter(|items| items.len() == N)
        .ok_or_else(|| format!("expected {N} numbers, found {value}"))?;
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item
            .as_f64()
            .ok_or_else(|| format!("non-numeric value {item}"))? as f32;
    }
    Ok(out)
}

fn vec3_field(value: &Value, key: &str) -> Result<Option<Vec3>, String> {
    value.get(key).map(numbers::<3>).transpose()
}

fn parse_cube_uv(value: &Value, size: Vec3) -> Result<CubeUv, String> {
    let uv = value.get("uv").ok_or("cube without uv")?;
    if uv.is_array() {
        return Ok(CubeUv::Box(numbers::<2>(uv)?));
    }
    let faces = uv.as_object().ok_or("cube uv must be an array or object")?;
    let mut parsed = Vec::new();
    for (name, face_value) in faces {
        let Some(face) = face_from_name(name) else {
            continue;
        };
        let origin = numbers::<2>(
            face_value
                .get("uv")
                .ok_or_else(|| format!("{name} face without uv"))?,
        )?;
        let face_size = match face_value.get("uv_size") {
            Some(uv_size) => numbers::<2>(uv_size)?,
            None => box_uv(face, [0.0; 2], size).1,
        };
        parsed.push((face, (origin, face_size)));
    }
    Ok(CubeUv::Faces(parsed))
}

fn parse_cube(value: &Value) -> Result<Cube, String> {
    let origin = numbers::<3>(value.get("origin").ok_or("cube without origin")?)?;
    let size = numbers::<3>(value.get("size").ok_or("cube without size")?)?;
    Ok(Cube {
        origin,
        size,
        inflate: value.get("inflate").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        mirror: value.get("mirror").and_then(Value::as_bool),
        pivot: vec3_field(value, "pivot")?,
        rotation: vec3_field(value, "rotation")?,
        uv: parse_cube_uv(value, size)?,
    })
}

fn parse_bone(value: &Value) -> Result<Bone, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or("bone without a name")?;
    let cubes = match value.get("cubes") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .ok_or_else(|| format!("bone {name}: cubes must be an array"))?
            .iter()
            .map(parse_cube)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("bone {name}: {error}"))?,
    };
    Ok(Bone {
        name: name.to_owned(),
        parent: value
            .get("parent")
            .and_then(Value::as_str)
            .map(str::to_owned),
        pivot: vec3_field(value, "pivot")?.unwrap_or([0.0; 3]),
        rotation: vec3_field(value, "rotation")?.unwrap_or([0.0; 3]),
        mirror: value
            .get("mirror")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        cubes,
    })
}

fn texture_size(description: &Value, key: &str) -> Result<u32, String> {
    match description.get(key) {
        None => Ok(DEFAULT_TEXTURE_SIZE),
        Some(value) => value
            .as_u64()
            .and_then(|size| u32::try_from(size).ok())
            .filter(|&size| size > 0)
            .ok_or_else(|| format!("invalid {key}: {value}")),
    }
}

/// Java-space point: Bedrock origin is at the feet (y = 24).
fn to_java_point(point: Vec3) -> Vec3 {
    [point[0], GROUND_Y - point[1], point[2]]
}

/// Java-space direction (offset or normal): only the Y axis flips.
fn to_java_direction(direction: Vec3) -> Vec3 {
    [direction[0], -direction[1], direction[2]]
}

fn java_rotation(degrees: Vec3) -> Vec3 {
    [
        -degrees[0].to_radians(),
        degrees[1].to_radians(),
        -degrees[2].to_radians(),
    ]
}

/// Pose of a part whose pivot is `point`, relative to `parent_pivot` (absolute for roots).
fn pose(point: Vec3, parent_pivot: Option<Vec3>, rotation_degrees: Vec3) -> [f32; 9] {
    let [x, y, z] = match parent_pivot {
        None => to_java_point(point),
        Some(parent) => to_java_direction(sub(point, parent)),
    };
    let [x_rot, y_rot, z_rot] = java_rotation(rotation_degrees);
    [x, y, z, x_rot, y_rot, z_rot, 1.0, 1.0, 1.0]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Point on the face at texture coordinates (s, t), s rightward and t downward from the
/// top-left corner as seen from outside the cube.
fn surface_point(frame: &FaceFrame, min: Vec3, max: Vec3, s: f32, t: f32) -> Vec3 {
    std::array::from_fn(|axis| {
        if frame.normal[axis] != 0.0 {
            if frame.normal[axis] > 0.0 {
                max[axis]
            } else {
                min[axis]
            }
        } else if frame.right[axis] != 0.0 {
            along(min[axis], max[axis], s, frame.right[axis])
        } else {
            along(min[axis], max[axis], 1.0 - t, frame.up[axis])
        }
    })
}

fn along(min: f32, max: f32, fraction: f32, direction: f32) -> f32 {
    if direction > 0.0 {
        min + (max - min) * fraction
    } else {
        max - (max - min) * fraction
    }
}

fn face_polygon(
    face: Face,
    (min, max): (Vec3, Vec3),
    (uv_origin, uv_size): FaceUv,
    mirror: bool,
    texture: (f32, f32),
    frame_origin: Vec3,
) -> GeoPolygon {
    let frame = face_frame(face);
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let mut vertices = corners.map(|(s, t)| {
        let position = to_java_direction(sub(surface_point(&frame, min, max, s, t), frame_origin));
        let s_texture = if mirror { 1.0 - s } else { s };
        let u = (uv_origin[0] + s_texture * uv_size[0]) / texture.0;
        let v = (uv_origin[1] + t * uv_size[1]) / texture.1;
        [position[0], position[1], position[2], u, v]
    });
    let normal = to_java_direction(frame.normal);
    let edge_a = sub(position_of(vertices[1]), position_of(vertices[0]));
    let edge_b = sub(position_of(vertices[2]), position_of(vertices[0]));
    if dot(cross(edge_a, edge_b), normal) < 0.0 {
        vertices.reverse();
    }
    GeoPolygon { normal, vertices }
}

fn position_of(vertex: [f32; 5]) -> Vec3 {
    [vertex[0], vertex[1], vertex[2]]
}

/// Quads for one cube, relative to `frame_origin` (the bone pivot, or the cube's own pivot).
fn cube_polygons(
    cube: &Cube,
    mirror: bool,
    texture: (f32, f32),
    frame_origin: Vec3,
) -> Vec<GeoPolygon> {
    let min: Vec3 = std::array::from_fn(|axis| cube.origin[axis] - cube.inflate);
    let max: Vec3 = std::array::from_fn(|axis| cube.origin[axis] + cube.size[axis] + cube.inflate);
    FACES
        .iter()
        .filter_map(|&face| {
            let uv = match &cube.uv {
                CubeUv::Box(origin) => box_uv(face, *origin, cube.size),
                CubeUv::Faces(faces) => faces.iter().find(|(f, _)| *f == face)?.1,
            };
            Some(face_polygon(
                face,
                (min, max),
                uv,
                mirror,
                texture,
                frame_origin,
            ))
        })
        .collect()
}

struct Hierarchy {
    roots: Vec<usize>,
    children: Vec<Vec<usize>>,
}

fn hierarchy(bones: &[Bone]) -> Result<Hierarchy, String> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, bone) in bones.iter().enumerate() {
        if index.insert(bone.name.as_str(), i).is_some() {
            return Err(format!("duplicate bone {}", bone.name));
        }
    }
    let mut children = vec![Vec::new(); bones.len()];
    let mut roots = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        match bone.parent.as_deref().and_then(|parent| index.get(parent)) {
            Some(&parent) => children[parent].push(i),
            None => roots.push(i),
        }
    }
    Ok(Hierarchy { roots, children })
}

struct Context<'a> {
    bones: &'a [Bone],
    hierarchy: Hierarchy,
    texture: (f32, f32),
}

fn build_part(
    context: &Context,
    index: usize,
    parent_pivot: Option<Vec3>,
    built: &mut usize,
) -> GeoPart {
    let bone = &context.bones[index];
    *built += 1;
    let mut polygons = Vec::new();
    let mut children = Vec::new();
    for (i, cube) in bone.cubes.iter().enumerate() {
        let mirror = cube.mirror.unwrap_or(bone.mirror);
        if cube.pivot.is_none() && cube.rotation.is_none() {
            polygons.extend(cube_polygons(cube, mirror, context.texture, bone.pivot));
        } else {
            let pivot = cube.pivot.unwrap_or(bone.pivot);
            children.push(GeoPart {
                name: format!("{}_cube{i}", bone.name),
                pose: pose(pivot, Some(bone.pivot), cube.rotation.unwrap_or([0.0; 3])),
                polygons: cube_polygons(cube, mirror, context.texture, pivot),
                children: Vec::new(),
            });
        }
    }
    for &child in &context.hierarchy.children[index] {
        children.push(build_part(context, child, Some(bone.pivot), built));
    }
    GeoPart {
        name: bone.name.clone(),
        pose: pose(bone.pivot, parent_pivot, bone.rotation),
        polygons,
        children,
    }
}

pub fn parse_geo(json: &str) -> Result<GeoModel, String> {
    let document: Value =
        serde_json::from_str(json).map_err(|error| format!("invalid geo json: {error}"))?;
    let geometry = document["minecraft:geometry"]
        .get(0)
        .ok_or("missing minecraft:geometry[0]")?;
    let description = &geometry["description"];
    let texture_width = texture_size(description, "texture_width")?;
    let texture_height = texture_size(description, "texture_height")?;
    let bones = geometry["bones"]
        .as_array()
        .ok_or("geometry has no bones array")?
        .iter()
        .map(parse_bone)
        .collect::<Result<Vec<_>, _>>()?;
    let context = Context {
        bones: &bones,
        hierarchy: hierarchy(&bones)?,
        texture: (texture_width as f32, texture_height as f32),
    };
    let mut built = 0;
    let roots: Vec<GeoPart> = context
        .hierarchy
        .roots
        .iter()
        .map(|&root| build_part(&context, root, None, &mut built))
        .collect();
    if built != bones.len() {
        return Err("bone parent cycle".to_owned());
    }
    Ok(GeoModel {
        roots,
        texture_width,
        texture_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }

    fn assert_pose_close(actual: [f32; 9], expected: [f32; 9]) {
        for (a, e) in actual.iter().zip(expected) {
            assert_close(*a, e);
        }
    }

    fn assert_vertices_close(actual: [[f32; 5]; 4], expected: [[f32; 5]; 4]) {
        for (a, e) in actual.iter().zip(expected) {
            for (x, y) in a.iter().zip(e) {
                assert_close(*x, y);
            }
        }
    }

    fn model_with_bones(bones: &str) -> String {
        format!(
            r#"{{"format_version":"1.12.0","minecraft:geometry":[{{"description":{{"identifier":"geometry.test","texture_width":64,"texture_height":64}},"bones":[{bones}]}}]}}"#
        )
    }

    fn single_cube(cube_fields: &str) -> String {
        model_with_bones(&format!(
            r#"{{"name":"body","pivot":[0,0,0],"cubes":[{{"origin":[-4,0,-4],"size":[8,8,8],{cube_fields}}}]}}"#
        ))
    }

    fn polygon_with_normal(part: &GeoPart, normal: Vec3) -> &GeoPolygon {
        part.polygons
            .iter()
            .find(|polygon| polygon.normal == normal)
            .expect("polygon with normal")
    }

    #[test]
    fn box_uv_unwraps_each_face_to_its_rectangle() {
        let size = [8.0, 8.0, 8.0];
        let rect = |face| {
            let (origin, uv_size) = box_uv(face, [0.0, 0.0], size);
            [origin[0], origin[1], uv_size[0], uv_size[1]]
        };

        assert_eq!(rect(Face::East), [0.0, 8.0, 8.0, 8.0]);
        assert_eq!(rect(Face::North), [8.0, 8.0, 8.0, 8.0]);
        assert_eq!(rect(Face::West), [16.0, 8.0, 8.0, 8.0]);
        assert_eq!(rect(Face::South), [24.0, 8.0, 8.0, 8.0]);
        assert_eq!(rect(Face::Up), [8.0, 0.0, 8.0, 8.0]);
        assert_eq!(rect(Face::Down), [16.0, 0.0, 8.0, 8.0]);
    }

    #[test]
    fn single_cube_emits_six_quads_with_exact_box_uv() {
        let model = parse_geo(&single_cube(r#""uv":[0,0]"#)).unwrap();

        let body = &model.roots[0];
        assert_eq!(body.name, "body");
        assert_pose_close(body.pose, [0.0, 24.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert_eq!(body.polygons.len(), 6);
        assert_eq!(model.texture_width, 64);
        assert!(body.children.is_empty());

        let up = polygon_with_normal(body, [0.0, -1.0, 0.0]);
        assert_vertices_close(
            up.vertices,
            [
                [-4.0, -8.0, -4.0, 8.0 / 64.0, 0.0],
                [4.0, -8.0, -4.0, 16.0 / 64.0, 0.0],
                [4.0, -8.0, 4.0, 16.0 / 64.0, 8.0 / 64.0],
                [-4.0, -8.0, 4.0, 8.0 / 64.0, 8.0 / 64.0],
            ],
        );
    }

    #[test]
    fn every_quad_normal_points_away_from_the_cube_center() {
        let model = parse_geo(&single_cube(r#""uv":[0,0]"#)).unwrap();
        let body = &model.roots[0];
        let center = [0.0, -4.0, 0.0];

        for polygon in &body.polygons {
            let sum = polygon.vertices.iter().fold([0.0; 3], |acc, v| {
                [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2]]
            });
            let centroid = sub([sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0], center);
            let edge_a = sub(
                position_of(polygon.vertices[1]),
                position_of(polygon.vertices[0]),
            );
            let edge_b = sub(
                position_of(polygon.vertices[2]),
                position_of(polygon.vertices[0]),
            );
            let winding = cross(edge_a, edge_b);

            assert!(dot(centroid, polygon.normal) > 0.0, "{:?}", polygon.normal);
            assert!(dot(winding, polygon.normal) > 0.0, "{:?}", polygon.normal);
        }
    }

    #[test]
    fn child_pose_is_relative_to_parent_pivot_with_rotation_sign_flipped_on_x_and_z() {
        let model = parse_geo(&model_with_bones(
            r#"{"name":"a","pivot":[0,8,0],"rotation":[30,45,-20]},
               {"name":"b","parent":"a","pivot":[2,12,-1],"rotation":[10,0,0]}"#,
        ))
        .unwrap();

        let a = &model.roots[0];
        assert_pose_close(
            a.pose,
            [
                0.0,
                16.0,
                0.0,
                -30f32.to_radians(),
                45f32.to_radians(),
                20f32.to_radians(),
                1.0,
                1.0,
                1.0,
            ],
        );
        let b = &a.children[0];
        assert_eq!(b.name, "b");
        assert_pose_close(
            b.pose,
            [
                2.0,
                -4.0,
                -1.0,
                -10f32.to_radians(),
                0.0,
                0.0,
                1.0,
                1.0,
                1.0,
            ],
        );
    }

    #[test]
    fn inflate_grows_geometry_but_keeps_uv_from_the_original_size() {
        let model = parse_geo(&model_with_bones(
            r#"{"name":"body","pivot":[0,0,0],"cubes":[{"origin":[0,0,0],"size":[4,4,4],"inflate":1,"uv":[0,0]}]}"#,
        ))
        .unwrap();

        let up = polygon_with_normal(&model.roots[0], [0.0, -1.0, 0.0]);
        assert_vertices_close(
            up.vertices,
            [
                [-1.0, -5.0, -1.0, 4.0 / 64.0, 0.0],
                [5.0, -5.0, -1.0, 8.0 / 64.0, 0.0],
                [5.0, -5.0, 5.0, 8.0 / 64.0, 4.0 / 64.0],
                [-1.0, -5.0, 5.0, 4.0 / 64.0, 4.0 / 64.0],
            ],
        );
    }

    #[test]
    fn mirror_flips_uv_horizontally_and_keeps_geometry() {
        let plain = parse_geo(&single_cube(r#""uv":[0,0]"#)).unwrap();
        let mirrored = parse_geo(&single_cube(r#""uv":[0,0],"mirror":true"#)).unwrap();

        let north_plain = polygon_with_normal(&plain.roots[0], [0.0, 0.0, -1.0]);
        let north_mirror = polygon_with_normal(&mirrored.roots[0], [0.0, 0.0, -1.0]);
        assert_close(north_plain.vertices[0][3], 8.0 / 64.0);
        assert_close(north_mirror.vertices[0][3], 16.0 / 64.0);
        assert_close(north_mirror.vertices[1][3], 8.0 / 64.0);
        for (a, b) in north_plain.vertices.iter().zip(north_mirror.vertices) {
            assert_close(a[0], b[0]);
            assert_close(a[1], b[1]);
            assert_close(a[2], b[2]);
        }
    }

    #[test]
    fn per_face_uv_supports_negative_size_and_omits_missing_faces() {
        let json = model_with_bones(
            r#"{"name":"body","pivot":[0,0,0],"cubes":[{"origin":[0,0,0],"size":[5,4,4],"uv":{"north":{"uv":[1,2],"uv_size":[4,4]},"down":{"uv":[10,24],"uv_size":[5,-4]}}}]}"#,
        );
        let model = parse_geo(&json).unwrap();

        let body = &model.roots[0];
        assert_eq!(
            body.polygons.len(),
            2,
            "only the two listed faces emit quads"
        );
        let down = polygon_with_normal(body, [0.0, 1.0, 0.0]);
        assert_close(down.vertices[0][3], 10.0 / 64.0);
        assert_close(down.vertices[0][4], 24.0 / 64.0);
        assert_close(down.vertices[3][4], 20.0 / 64.0);
    }

    #[test]
    fn cube_with_its_own_rotation_becomes_a_child_part() {
        let model = parse_geo(&model_with_bones(
            r#"{"name":"body","pivot":[0,0,0],"cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0],"pivot":[0,8,0],"rotation":[0,0,90]}]}"#,
        ))
        .unwrap();

        let body = &model.roots[0];
        assert!(body.polygons.is_empty());
        let cube = &body.children[0];
        assert_eq!(cube.name, "body_cube0");
        assert_eq!(cube.polygons.len(), 6);
        assert_pose_close(
            cube.pose,
            [0.0, -8.0, 0.0, 0.0, 0.0, -90f32.to_radians(), 1.0, 1.0, 1.0],
        );
    }

    #[test]
    fn bone_parent_cycle_is_rejected() {
        let json = model_with_bones(r#"{"name":"a","parent":"b"},{"name":"b","parent":"a"}"#);
        assert!(parse_geo(&json).is_err());
    }

    #[test]
    fn bone_with_unknown_parent_becomes_a_root() {
        let model = parse_geo(&model_with_bones(
            r#"{"name":"orphan","parent":"missing","pivot":[0,0,0]}"#,
        ))
        .unwrap();

        assert_eq!(model.roots.len(), 1);
        assert_eq!(model.roots[0].name, "orphan");
    }
}
