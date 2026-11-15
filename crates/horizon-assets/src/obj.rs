//! NeoForge `neoforge:obj` models: Wavefront OBJ geometry (with its `.mtl`)
//! turned into textured quads in block units. Triangles repeat their last
//! vertex, so the mesher only ever sees quads; larger polygons are fanned.
use crate::json::*;
use std::collections::{BTreeMap, BTreeSet};

/// What a model JSON asks of its OBJ file.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjSource {
    /// `namespace:path` of the `.obj` under `assets/<namespace>/`.
    pub model: String,
    pub flip_v: bool,
    pub automatic_culling: bool,
    /// Object and group names the JSON's `visibility` map turns off.
    pub hidden_parts: BTreeSet<String>,
}
impl ObjSource {
    /// The `model`, `flip_v`, `automatic_culling` and `visibility` keys of a
    /// model JSON; `None` without a usable `model`.
    pub(crate) fn decode(obj: &Object<'_>) -> Option<Self> {
        let model = string(obj.get("model")?).ok()?;
        let flag = |key: &str, default: bool| {
            obj.get(key)
                .and_then(|raw| boolean(raw).ok())
                .unwrap_or(default)
        };
        let hidden_parts = obj
            .get("visibility")
            .and_then(|raw| object(raw).ok())
            .into_iter()
            .flatten()
            .filter(|(_, raw)| boolean(raw).is_ok_and(|visible| !visible))
            .map(|(name, _)| name)
            .collect();
        Some(Self {
            model,
            flip_v: flag("flip_v", false),
            automatic_culling: flag("automatic_culling", true),
            hidden_parts,
        })
    }
}
/// One face of the OBJ; `texture` is a `map_Kd` value, or `#<material>`
/// when the material names none, resolved against the model's textures.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjQuad {
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub texture: String,
}
/// The JSON's request and, once the files were read and parsed, its quads.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjModel {
    pub source: ObjSource,
    pub quads: Option<Vec<ObjQuad>>,
}

/// `material name -> map_Kd` of a `.mtl` file; materials without one are
/// listed with `None`.
pub fn parse_materials(mtl: &str) -> BTreeMap<String, Option<String>> {
    let mut materials = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in mtl.lines() {
        let line = line.trim();
        let Some((keyword, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim();
        match keyword {
            "newmtl" => {
                materials.insert(rest.to_owned(), None);
                current = Some(rest.to_owned());
            }
            "map_Kd" => {
                // Options such as `-s 1 1 1` precede the file name.
                let Some(name) = rest.split_whitespace().last() else {
                    continue;
                };
                if let Some(entry) = current.as_ref().and_then(|c| materials.get_mut(c)) {
                    *entry = Some(name.to_owned());
                }
            }
            _ => {}
        }
    }
    materials
}
/// The `mtllib` file names an OBJ lists.
pub fn material_libraries(obj: &str) -> Vec<String> {
    obj.lines()
        .filter_map(|line| line.trim().strip_prefix("mtllib"))
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .flat_map(|rest| rest.split_whitespace().map(str::to_owned))
        .collect()
}

fn number(token: Option<&str>, line: usize) -> std::result::Result<f32, String> {
    token
        .and_then(|t| t.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("line {line}: bad number"))
}
/// A 1-based (or negative, from the end) OBJ index as a 0-based one.
fn index(token: &str, count: usize, line: usize) -> std::result::Result<usize, String> {
    let value: i64 = token
        .parse()
        .map_err(|_| format!("line {line}: bad index `{token}`"))?;
    let resolved = if value < 0 {
        count as i64 + value
    } else {
        value - 1
    };
    usize::try_from(resolved)
        .ok()
        .filter(|i| *i < count)
        .ok_or_else(|| format!("line {line}: index {value} out of range"))
}
/// Parse `text`; `materials` come from the `.mtl` files it lists.
pub fn parse_obj(
    text: &str,
    materials: &BTreeMap<String, Option<String>>,
    source: &ObjSource,
) -> std::result::Result<Vec<ObjQuad>, String> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut quads = Vec::new();
    let mut part = String::new();
    let mut texture = String::new();
    for (number_of_line, line) in text.lines().enumerate() {
        let line_number = number_of_line + 1;
        let line = line.trim();
        let Some((keyword, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim();
        let mut tokens = rest.split_whitespace();
        match keyword {
            "v" => positions.push([
                number(tokens.next(), line_number)?,
                number(tokens.next(), line_number)?,
                number(tokens.next(), line_number)?,
            ]),
            "vt" => {
                let u = number(tokens.next(), line_number)?;
                let v = tokens
                    .next()
                    .map_or(Ok(0.0), |t| number(Some(t), line_number))?;
                uvs.push([u, if source.flip_v { 1.0 - v } else { v }]);
            }
            "o" | "g" => rest.clone_into(&mut part),
            "usemtl" => {
                texture = match materials.get(rest) {
                    Some(Some(map)) => map.clone(),
                    _ => format!("#{rest}"),
                };
            }
            "f" => {
                if source.hidden_parts.contains(&part) {
                    continue;
                }
                let mut corners = Vec::new();
                for token in tokens {
                    let mut parts = token.split('/');
                    let position = index(parts.next().unwrap_or(""), positions.len(), line_number)?;
                    let uv = match parts.next().filter(|t| !t.is_empty()) {
                        Some(t) => uvs[index(t, uvs.len(), line_number)?],
                        None => [0.0, 0.0],
                    };
                    corners.push((positions[position], uv));
                }
                if corners.len() < 3 {
                    return Err(format!("line {line_number}: face with fewer than 3 vertices"));
                }
                let make = |ids: [usize; 4]| ObjQuad {
                    positions: ids.map(|i| corners[i].0),
                    uv: ids.map(|i| corners[i].1),
                    texture: texture.clone(),
                };
                if corners.len() == 4 {
                    quads.push(make([0, 1, 2, 3]));
                } else {
                    for i in 1..corners.len() - 1 {
                        quads.push(make([0, i, i + 1, i + 1]));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(quads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(flip_v: bool) -> ObjSource {
        ObjSource {
            model: "m:models/x.obj".to_owned(),
            flip_v,
            automatic_culling: true,
            hidden_parts: BTreeSet::new(),
        }
    }
    const CUBE: &str = "mtllib c.mtl\no cube\n\
        v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
        vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\nusemtl m_0\n\
        f 5/1/1 6/2/1 7/3/1 8/4/1\nf 2/1 1/2 4/3 3/4\nf 1//1 5//1 8//1 4//1\n\
        f 6/1 2/2 3/3 7/4\nf 4 3 7 8\nf 1 2 6 5\n";

    #[test]
    fn a_cube_is_six_quads_with_the_material_texture() {
        let materials = parse_materials("newmtl m_0\nKa 1 1 1\nmap_Kd #0\n");
        let quads = parse_obj(CUBE, &materials, &source(false)).unwrap();
        assert_eq!(quads.len(), 6);
        assert!(quads.iter().all(|q| q.texture == "#0"));
        assert_eq!(quads[0].positions[0], [0.0, 0.0, 1.0]);
        assert_eq!(quads[0].uv[2], [1.0, 1.0]);
        assert_eq!(material_libraries(CUBE), ["c.mtl"]);
    }
    #[test]
    fn a_triangle_repeats_its_last_vertex_and_an_ngon_is_fanned() {
        let quads = parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nv 2 1 0\nf 1 2 3\nf 1 2 4 5 3\n",
            &BTreeMap::new(),
            &source(false),
        )
        .unwrap();
        assert_eq!(quads.len(), 4);
        assert_eq!(quads[0].positions[2], quads[0].positions[3]);
        assert_eq!(quads[0].texture, "");
        assert_eq!(quads[1].positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(quads[1].positions[2], [1.0, 1.0, 0.0]);
    }
    #[test]
    fn flip_v_mirrors_the_v_coordinate_and_materials_without_a_map_use_their_name() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0.25\nvt 1 0.25\nvt 1 1\nvt 0 1\n\
            usemtl wood\nf -4/-4 -3/-3 -2/-2 -1/-1\n";
        let flipped = parse_obj(obj, &BTreeMap::new(), &source(true)).unwrap();
        let plain = parse_obj(obj, &BTreeMap::new(), &source(false)).unwrap();
        assert_eq!(flipped[0].uv[0], [0.0, 0.75]);
        assert_eq!(flipped[0].uv[2], [1.0, 0.0]);
        assert_eq!(plain[0].uv[0], [0.0, 0.25]);
        assert_eq!(flipped[0].texture, "#wood");
    }
    #[test]
    fn hidden_parts_and_broken_faces() {
        let mut hidden = source(false);
        hidden.hidden_parts.insert("b".to_owned());
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\ng a\nf 1 2 3\ng b\nf 1 2 3\n";
        assert_eq!(parse_obj(obj, &BTreeMap::new(), &hidden).unwrap().len(), 1);
        assert!(parse_obj("v 0 0 0\nf 1 2 3\n", &BTreeMap::new(), &source(false)).is_err());
        assert!(parse_obj("v 0 0 0\nv 1 0 0\nf 1 2\n", &BTreeMap::new(), &source(false)).is_err());
        assert!(parse_obj("v 0 x 0\n", &BTreeMap::new(), &source(false)).is_err());
    }
}
