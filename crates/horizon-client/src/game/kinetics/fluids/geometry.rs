use super::{AXES, CreateFluid, CreateFluidState, CreateVesselKind, Rect, Vertex};
use glam::Vec3;
use horizon_client::gfx::TRANSLUCENT_ITEM_OVERLAY;
use horizon_client::mesher::CULL_BACK;
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct Stream {
    axis: usize,
    positive: bool,
    start_v: f32,
}

pub(super) struct Volume<'a> {
    pub fluid: &'a CreateFluid,
    min: Vec3,
    max: Vec3,
    stream: Option<Stream>,
    hidden: u8,
}

fn volume(fluid: &CreateFluid, min: Vec3, max: Vec3) -> Volume<'_> {
    Volume {
        fluid,
        min,
        max,
        stream: None,
        hidden: 0,
    }
}

fn reaches_pipe_center(inward: bool, progress: f32) -> bool {
    if inward {
        progress >= 1.0
    } else {
        progress > 0.0
    }
}

pub(super) fn volumes<'a>(
    block: &str,
    axis: Option<&str>,
    state: &'a CreateFluidState,
    alpha: f64,
    mut emit: impl FnMut(Volume<'a>),
) {
    if !state.matches_block(block) {
        return;
    }
    match state {
        CreateFluidState::Tank(tank) if tank.window && tank.controller.is_none() => {
            if let Some(fluid) = &tank.fluid {
                let height = f32::from(tank.height);
                let edge = f32::from(tank.width) - 9.0 / 128.0;
                let fill = tank.level.get(alpha).clamp(0.0, 1.0);
                let depth = (height - 9.0 / 16.0) * fill;
                if depth > 1.0 / 512.0 {
                    emit(volume(
                        fluid,
                        Vec3::new(9.0 / 128.0, 5.0 / 16.0, 9.0 / 128.0),
                        Vec3::new(edge, 5.0 / 16.0 + depth, edge),
                    ));
                }
            }
        }
        CreateFluidState::Basin(fluids) => {
            let total: f32 = fluids.iter().map(|f| f.amount as f32).sum();
            if total > 0.0 {
                let edge = 2.0 / 16.0;
                let width = 12.0 / 16.0;
                let empty = 1.0 - (total / 2_000.0).min(1.0);
                let surface = edge + width * (1.0 - empty * empty);
                let mut x = edge;
                for (i, fluid) in fluids.iter().enumerate() {
                    let next = x + width * fluid.amount as f32 / total;
                    let mut part = volume(
                        fluid,
                        Vec3::new(x, edge, edge),
                        Vec3::new(next, surface, 1.0 - edge),
                    );
                    part.hidden = (u8::from(i > 0) << 4) | (u8::from(i + 1 < fluids.len()) << 5);
                    emit(part);
                    x = next;
                }
            }
        }
        CreateFluidState::Vessel {
            fluid,
            level,
            processing,
            stream_fluid,
            kind,
            ..
        } => {
            let spout = *kind == CreateVesselKind::Spout;
            if let Some(fluid) = fluid {
                let fill = level.get(alpha).clamp(0.0, 1.0);
                if fill > 0.0 {
                    let (edge, bottom, height) = if *kind != CreateVesselKind::Drain {
                        (2.5 / 16.0, 2.5 / 16.0, 11.0 / 16.0 * fill.max(0.175))
                    } else {
                        (2.0 / 16.0, 5.0 / 16.0, 7.0 / 16.0 * fill)
                    };
                    emit(volume(
                        fluid,
                        Vec3::new(edge, bottom, edge),
                        Vec3::new(1.0 - edge, bottom + height, 1.0 - edge),
                    ));
                }
            }
            let s = ((15.0 - (*processing as f32 - alpha as f32)) / 10.0).clamp(0.0, 1.0);
            let radius = s * (1.0 - s) / 8.0;
            if *processing >= 0
                && radius > 0.0
                && let Some(fluid) = if spout {
                    fluid.as_ref()
                } else {
                    stream_fluid.as_ref().or(fluid.as_ref())
                }
            {
                let (low, high) = if spout { (-1.2, 0.0) } else { (0.25, 1.0) };
                let mut jet = volume(
                    fluid,
                    Vec3::new(0.5 - radius, low, 0.5 - radius),
                    Vec3::new(0.5 + radius, high, 0.5 + radius),
                );
                jet.stream = Some(Stream {
                    axis: 1,
                    positive: false,
                    start_v: 0.0,
                });
                emit(jet);
            }
        }
        CreateFluidState::Pipe(flows) => {
            let pipe_axis = match axis {
                Some("x") => 0,
                Some("y") => 1,
                Some("z") => 2,
                _ => return,
            };
            for (i, flow) in flows.iter().enumerate() {
                let Some(flow) = flow else {
                    continue;
                };
                if AXES[i] != pipe_axis {
                    continue;
                }
                let progress = flow.progress.get(alpha).clamp(0.0, 1.0);
                if progress <= 0.0 {
                    continue;
                }
                let positive = i % 2 == 1;
                let outer = if positive { 1.0 } else { 0.0 };
                let start = if flow.inward { outer } else { 0.5 };
                let end = start
                    + (if flow.inward {
                        0.5 - outer
                    } else {
                        outer - 0.5
                    }) * progress;
                let mut lo = Vec3::splat(5.0 / 16.0);
                let mut hi = Vec3::splat(11.0 / 16.0);
                lo[pipe_axis] = start.min(end);
                hi[pipe_axis] = start.max(end);
                let mut part = volume(&flow.fluid, lo, hi);
                part.stream = Some(Stream {
                    axis: pipe_axis,
                    positive: positive != flow.inward,
                    start_v: if flow.inward { 0.0 } else { 0.5 },
                });
                let opposite = flows[i ^ 1].as_ref();
                if let Some(other) = opposite
                    && other.fluid.id == flow.fluid.id
                    && other.fluid.tint == flow.fluid.tint
                    && reaches_pipe_center(flow.inward, progress)
                    && reaches_pipe_center(other.inward, other.progress.get(alpha))
                {
                    part.hidden |= 1 << (i ^ 1);
                }
                emit(part);
            }
        }
        _ => {}
    }
}

pub(super) fn sprites(atlas: &HashMap<String, Rect>, id: &str) -> Option<[Rect; 2]> {
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let path = path
        .strip_prefix("flowing_")
        .or_else(|| path.strip_suffix("_flowing"))
        .unwrap_or(path);
    let mut roots =
        ["fluid/", "block/fluid/", "block/fluids/"].map(|p| format!("{namespace}:{p}{path}"));
    if namespace == "minecraft" {
        roots[0] = if path == "milk" {
            "create:fluid/milk".into()
        } else {
            format!("block/{path}")
        };
    }
    for root in roots {
        if let Some(still) = atlas.get(&format!("{root}_still")) {
            let flowing = atlas
                .get(&format!("{root}_flow"))
                .or_else(|| atlas.get(&format!("{root}_flowing")));
            return Some([*still, flowing.copied().unwrap_or(*still)]);
        }
    }
    None
}

pub(super) fn emit_volume(
    volume: Volume<'_>,
    origin: Vec3,
    textures: [Rect; 2],
    light: u32,
    out: &mut Vec<Vertex>,
) {
    let rgb = volume.fluid.tint;
    let color = 0xff000000 | ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
    let light = if matches!(
        volume.fluid.id.as_str(),
        "minecraft:lava" | "minecraft:flowing_lava"
    ) {
        light | (0xf0 << 8)
    } else {
        light
    };
    // Face axes in offset order down, up, north, south, west, east. The winding
    // is outward so only the side nearest the camera blends with the casing.
    let tangents = [(0, 2), (2, 0), (1, 0), (0, 1), (2, 1), (1, 2)];
    for (i, (u, v)) in tangents.into_iter().enumerate() {
        if volume.hidden & (1 << i) != 0 {
            continue;
        }
        let n = AXES[i];
        let mut at = volume.min;
        if i % 2 == 1 {
            at[n] = volume.max[n];
        }
        let stream = volume.stream.filter(|stream| stream.axis != n);
        let along_u = stream.is_some_and(|stream| stream.axis == u);
        let rect = textures[usize::from(stream.is_some())];
        let width = volume.max[u] - volume.min[u];
        let height = volume.max[v] - volume.min[v];
        for x in 0..(width.ceil() as usize) {
            let du = (width - x as f32).min(1.0);
            for y in 0..(height.ceil() as usize) {
                let dv = (height - y as f32).min(1.0);
                let mut base = at;
                base[u] += x as f32;
                base[v] += y as f32;
                if let Some(Stream {
                    axis,
                    positive: false,
                    ..
                }) = stream
                {
                    // Anchor each tile at the start of the stream without changing face winding.
                    let (tile, size) = if along_u { (x, du) } else { (y, dv) };
                    base[axis] = volume.max[axis] - tile as f32 - size;
                }
                for [s, t] in [[0.0, 0.0], [du, 0.0], [du, dv], [0.0, dv]] {
                    let mut p = base;
                    p[u] += s;
                    p[v] += t;
                    let [sprite_u, sprite_v] = if let Some(stream) = stream {
                        let (across, along, length) = if along_u { (t, s, du) } else { (s, t, dv) };
                        let along = if stream.positive {
                            along
                        } else {
                            length - along
                        };
                        [across, stream.start_v + along]
                    } else {
                        [s, t]
                    };
                    out.push(Vertex {
                        pos: (origin + p).to_array(),
                        uv: [
                            rect.u0 + sprite_u * (rect.u1 - rect.u0),
                            rect.v0 + sprite_v * (rect.v1 - rect.v0),
                        ],
                        layer: TRANSLUCENT_ITEM_OVERLAY,
                        color,
                        light,
                        normal: CULL_BACK,
                        glint_uv: [0.0; 2],
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::update;
    use super::*;
    use horizon_client::world::{CreateFluidFlow, LerpedValue};
    use horizon_world::nbt::parse_snbt;

    fn tag(text: &str) -> horizon_world::nbt::Tag {
        parse_snbt(text).unwrap().0
    }

    #[test]
    fn tank_is_tiled_at_controller_and_opens_to_the_right_height() {
        let t = tag(
            r#"{Window:1b,Size:3,Height:4,TankContent:{Fluid:{id:"minecraft:water",amount:144000}}}"#,
        );
        let state = update("create:fluid_tank", None, &t).unwrap();
        let mut count = 0;
        volumes("create:fluid_tank", None, &state, 0.5, |part| {
            assert_eq!(part.min, Vec3::new(9.0 / 128.0, 5.0 / 16.0, 9.0 / 128.0));
            assert_eq!(part.max.y, 5.0 / 16.0 + (4.0 - 9.0 / 16.0) / 2.0);
            count += 1;
        });
        assert_eq!(count, 1);
        assert!(state.matches_block("create:fluid_tank"));
        assert!(!state.matches_block("create:basin"));
        assert!(!state.matches_block("create:creative_fluid_tank"));
        let follower = update(
            "create:fluid_tank",
            None,
            &tag("{Controller:[I;2,3,4],Window:1b}"),
        )
        .unwrap();
        volumes("create:fluid_tank", None, &follower, 0.5, |_| {
            panic!("follower drew a second tank")
        });
        let empty = update(
            "create:fluid_tank",
            Some(&state),
            &tag("{Window:1b,TankContent:{}}"),
        )
        .unwrap();
        volumes("create:fluid_tank", None, &empty, 0.5, |_| {
            panic!("drained tank still has liquid")
        });
    }

    #[test]
    fn small_visible_depth_in_a_tall_tank_requires_the_matching_block() {
        let tank = update(
            "create:fluid_tank",
            None,
            &tag(r#"{Window:1b,Size:3,Height:32,TankContent:{Fluid:{id:"minecraft:water",amount:200}}}"#),
        )
        .unwrap();
        let mut count = 0;
        volumes("create:fluid_tank", None, &tank, 1.0, |part| {
            assert!(part.max.y - part.min.y > 1.0 / 512.0);
            count += 1;
        });
        assert_eq!(count, 1);
        volumes("minecraft:air", None, &tank, 1.0, |_| {
            panic!("stale tank drew through a changed block")
        });
    }

    #[test]
    fn fluid_tank_update_keeps_its_geometry_through_the_wire_packet() {
        use horizon_protocol::buf::Reader;
        use horizon_protocol::packets::play::{BlockEntityData, Clientbound};

        let packet = Clientbound::BlockEntityData(BlockEntityData {
            pos: [-3, 4, 5],
            kind: 614,
            tag: tag(
                r#"{Window:1b,Size:1,Height:1,TankContent:{Fluid:{id:"create:honey",amount:4000}}}"#,
            ),
        });
        let encoded = packet.encode();
        let mut wire = Reader::new(&encoded);
        let decoded = Clientbound::decode(wire.varint().unwrap(), wire.rest()).unwrap();
        let Clientbound::BlockEntityData(data) = decoded else {
            panic!("wrong packet");
        };
        assert_eq!(data.pos, [-3, 4, 5]);
        assert_eq!(data.kind, 614);
        let state = update("create:fluid_tank", None, &data.tag).unwrap();
        let mut tops = Vec::new();
        volumes("create:fluid_tank", None, &state, 0.5, |part| {
            assert_eq!(part.fluid.id, "create:honey");
            tops.push(part.max.y);
        });
        assert_eq!(tops, [17.0 / 32.0]);
    }

    #[test]
    fn basin_has_proportional_layers_and_pipe_uses_the_named_axis() {
        let tag_basin = tag(
            r#"{InputTanks:[{TankContent:{Fluid:{id:"minecraft:water",amount:250}}},{TankContent:{}}],OutputTanks:[{TankContent:{Fluid:{id:"create:honey",amount:750}}}]}"#,
        );
        let basin = update("create:basin", None, &tag_basin).unwrap();
        let mut segments = Vec::new();
        volumes("create:basin", None, &basin, 1.0, |part| {
            segments.push((part.max - part.min, part.hidden))
        });
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].0.x * 3.0, segments[1].0.x);
        assert_eq!(segments[0].0.y, segments[1].0.y);
        assert_eq!(segments[0].1, 32);
        assert_eq!(segments[1].1, 16);
        let pipe = update("create:glass_fluid_pipe", None, &tag(r#"{west:{Flow:{Fluid:{id:"minecraft:water",amount:1},In:1b,Progress:{Value:0.5f}}}}"#)).unwrap();
        let mut results = Vec::new();
        volumes("create:glass_fluid_pipe", Some("x"), &pipe, 1.0, |part| {
            results.push((part.min.x, part.max.x))
        });
        assert_eq!(results, [(0.0, 0.25)]);
        volumes("create:glass_fluid_pipe", Some("z"), &pipe, 1.0, |_| {
            panic!("flow crossed the pipe axis")
        });
    }

    fn pipe_flow(inward: bool, progress: f32, tint: u32) -> CreateFluidFlow {
        CreateFluidFlow {
            fluid: CreateFluid {
                id: "create:potion".into(),
                amount: 1,
                tint,
            },
            inward,
            progress: LerpedValue::start_with(progress),
        }
    }

    #[test]
    fn pipe_side_uvs_follow_flow_direction_on_each_axis() {
        let rect = Rect {
            u0: 0.0,
            v0: 0.0,
            u1: 0.25,
            v1: 0.5,
            w: 16,
            h: 16,
        };
        let flowing = Rect {
            u0: 0.5,
            u1: 0.75,
            ..rect
        };
        for (axis, coord, low_side) in [("x", 0, 4), ("y", 1, 0), ("z", 2, 2)] {
            for (side, inward) in [
                (low_side, true),
                (low_side, false),
                (low_side + 1, true),
                (low_side + 1, false),
            ] {
                let mut flows = Box::new(std::array::from_fn(|_| None));
                flows[side] = Some(pipe_flow(inward, 0.5, 0xffffff));
                let pipe = CreateFluidState::Pipe(flows);
                let mut out = Vec::new();
                volumes("create:glass_fluid_pipe", Some(axis), &pipe, 1.0, |part| {
                    emit_volume(part, Vec3::ZERO, [rect, flowing], 0, &mut out);
                });
                let (start, sign) = match (inward, side == low_side) {
                    (true, true) => (0.0, 1.0),
                    (true, false) => (1.0, -1.0),
                    (false, true) => (0.5, -1.0),
                    (false, false) => (0.5, 1.0),
                };
                let start_v = if inward { 0.0 } else { 0.5 };
                let mut side_vertices = 0;
                for vertex in out.iter().filter(|v| v.uv[0] >= flowing.u0) {
                    let distance = (vertex.pos[coord] - start) * sign;
                    assert!((0.0..=0.25).contains(&distance));
                    assert_eq!(vertex.uv[1], (start_v + distance) * flowing.v1);
                    side_vertices += 1;
                }
                assert_eq!(side_vertices, 16);
            }
        }
    }

    #[test]
    fn pipe_center_caps_hide_only_when_matching_volumes_meet() {
        for (incoming, outgoing, tint, expected) in [
            (1.0, 0.0, 0x123456, vec![0]),
            (0.9995, 0.5, 0x123456, vec![0, 0]),
            (1.0, 0.5, 0xabcdef, vec![0, 0]),
            (1.0, 0.5, 0x123456, vec![1 << 5, 1 << 4]),
        ] {
            let mut flows = Box::new(std::array::from_fn(|_| None));
            flows[4] = Some(pipe_flow(true, incoming, 0x123456));
            flows[5] = Some(pipe_flow(false, outgoing, tint));
            let mut hidden = Vec::new();
            volumes(
                "create:glass_fluid_pipe",
                Some("x"),
                &CreateFluidState::Pipe(flows),
                1.0,
                |part| {
                    hidden.push(part.hidden);
                },
            );
            assert_eq!(hidden, expected);
        }
    }
}
