//! `neoforge:obj` faces as model quads. OBJ winding is counterclockwise seen
//! from outside, the order the mesher already expects of cuboid faces.
use super::{BlockModelTransform, ModelQuad};
use crate::{model::Direction, resolve::ObjDraw};

pub(super) fn bake_obj_quad(draw: &ObjDraw, transform: BlockModelTransform) -> ModelQuad {
    let source = draw.quad.positions;
    let cull = draw
        .automatic_culling
        .then(|| boundary_direction(&source))
        .flatten();
    let positions = source.map(|v| transform.position(v));
    ModelQuad {
        positions,
        uv: draw.quad.uv,
        facing: nearest_direction(&positions).unwrap_or(Direction::Up),
        cull: cull.map(|d| transform.direction(d)),
        material: draw.material.clone(),
        tint_index: -1,
        shade_direction_override: None,
        light_emission: 0,
    }
}
/// The direction a polygon faces (Newell's method, robust to a degenerate
/// first corner), as the closest of the six axes.
fn nearest_direction(vertices: &[[f32; 3]; 4]) -> Option<Direction> {
    let mut n = [0.0f32; 3];
    for i in 0..4 {
        let (a, b) = (vertices[i], vertices[(i + 1) % 4]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    if n.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mut best = 0.0;
    let mut result = None;
    for (direction, dot) in Direction::ALL
        .into_iter()
        .zip([-n[1], n[1], -n[2], n[2], -n[0], n[0]])
    {
        if dot > best {
            best = dot;
            result = Some(direction);
        }
    }
    result
}
/// The block side a quad lies flat against, if it does.
fn boundary_direction(vertices: &[[f32; 3]; 4]) -> Option<Direction> {
    let direction = nearest_direction(vertices)?;
    let (axis, plane) = match direction {
        Direction::Down => (1, 0.0),
        Direction::Up => (1, 1.0),
        Direction::North => (2, 0.0),
        Direction::South => (2, 1.0),
        Direction::West => (0, 0.0),
        Direction::East => (0, 1.0),
    };
    vertices
        .iter()
        .all(|v| v[axis] == plane)
        .then_some(direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_follow_the_winding_and_cull_only_on_the_block_boundary() {
        let top = [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]];
        assert_eq!(nearest_direction(&top), Some(Direction::Up));
        assert_eq!(boundary_direction(&top), Some(Direction::Up));
        let inner = top.map(|[x, _, z]| [x, 0.5, z]);
        assert_eq!(nearest_direction(&inner), Some(Direction::Up));
        assert_eq!(boundary_direction(&inner), None);
        let triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(nearest_direction(&triangle), Some(Direction::South));
    }
}
