//! Moving plot terrain uses the regular terrain materials and a separate vertex stream.

use super::{Frustum, section_instance, upload::Uploads};
use crate::mesher::{SectionMesh, TerrainVertex};
use glam::{DMat4, Mat4, Vec3};
use wgpu::util::DeviceExt;

pub struct PlotSection<'a> {
    /// Section-local coordinates to camera-relative world coordinates.
    pub model: DMat4,
    pub mesh: &'a SectionMesh,
}

pub(super) struct PlotTerrain {
    origin: wgpu::Buffer,
    vertices: Option<wgpu::Buffer>,
    capacity: u64,
    solid_count: u32,
    translucent_count: u32,
    transformed: Vec<TerrainVertex>,
    translucent: Vec<(f32, [TerrainVertex; 4])>,
}

impl PlotTerrain {
    pub fn new(device: &wgpu::Device) -> Self {
        let origin = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Plot terrain origin"),
            contents: bytemuck::bytes_of(&section_instance([0.0; 3])),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            origin,
            vertices: None,
            capacity: 0,
            solid_count: 0,
            translucent_count: 0,
            transformed: Vec::new(),
            translucent: Vec::new(),
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        uploads: &mut Uploads,
        encoder: &mut wgpu::CommandEncoder,
        sections: &[PlotSection<'_>],
        view: &Frustum,
    ) -> u32 {
        self.transformed.clear();
        self.translucent.clear();
        let stride = std::mem::size_of::<TerrainVertex>() as u64;
        let limit = device.limits().max_buffer_size;
        for section in sections {
            let model = section.model.as_mat4();
            if !visible(view, model) {
                continue;
            }
            let vertices = self.transformed.len()
                + self.translucent.len() * 4
                + section.mesh.solid.len()
                + section.mesh.translucent.len();
            if vertices as u64 * stride > limit {
                continue;
            }
            let transform = |mut vertex: TerrainVertex| {
                vertex.pos = model.transform_point3(Vec3::from(vertex.pos)).to_array();
                vertex
            };
            self.transformed
                .extend(section.mesh.solid.iter().copied().map(transform));
            for quad in section.mesh.translucent.chunks_exact(4) {
                let quad = std::array::from_fn(|i| transform(quad[i]));
                let center = quad.iter().map(|v| Vec3::from(v.pos)).sum::<Vec3>() * 0.25;
                self.translucent.push((center.length_squared(), quad));
            }
        }
        self.solid_count = (self.transformed.len() / 4) as u32;
        self.translucent_count = self.translucent.len() as u32;
        self.translucent
            .sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        self.transformed
            .extend(self.translucent.iter().flat_map(|&(_, quad)| quad));
        let bytes = bytemuck::cast_slice(&self.transformed);
        if bytes.is_empty() {
            return 0;
        }
        if bytes.len() as u64 > self.capacity {
            self.capacity = (bytes.len() as u64).next_power_of_two().min(limit);
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Plot terrain vertices"),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.vertices {
            uploads.buffer(encoder, buffer, 0, bytes);
        }
        self.solid_count.max(self.translucent_count)
    }

    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        indices: &wgpu::Buffer,
        translucent: bool,
    ) {
        let (count, base) = if translucent {
            (self.translucent_count, self.solid_count * 4)
        } else {
            (self.solid_count, 0)
        };
        if count != 0
            && let Some(vertices) = &self.vertices
        {
            pass.set_pipeline(pipeline);
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_vertex_buffer(1, self.origin.slice(..));
            pass.draw_indexed(0..count * 6, base as i32, 0..1);
        }
    }
}

fn visible(view: &Frustum, model: Mat4) -> bool {
    let center = model.transform_point3(Vec3::splat(8.0));
    // Enclose the rotated section and model overhangs.
    let extent = (model.x_axis.truncate().abs()
        + model.y_axis.truncate().abs()
        + model.z_axis.truncate().abs())
        * 9.0;
    view.intersects(center - extent, center + extent)
}
