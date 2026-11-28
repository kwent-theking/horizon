//! Headless frame capture for renderer and gameplay fixtures.

use super::{
    EntityBatch, Globals, HudAtlasSource, Renderer, copy_frame, entity_shader, padded_row,
    shared_atlas,
};
use crate::{Result, mesher::Vertex};
use bytemuck::Zeroable;
use glam::Mat4;
use wgpu::util::DeviceExt;

/// Draws entity quads against black using a white atlas and lightmap. The
/// projection uses conventional depth (0 near), as in `Mat4::orthographic_rh`.
/// Returns tightly packed RGBA8 rows from top to bottom.
pub fn render(vertices: &[Vertex], view_proj: Mat4, width: u32, height: u32) -> Result<Vec<u8>> {
    render_with_atlas(vertices, view_proj, width, height, [1, 1], &[255; 4])
}

/// Draws entity quads using the caller's tightly packed RGBA8 atlas. Material
/// overlays use the opaque, translucent item, then translucent particle order.
/// The lightmap is white and the projection uses conventional depth (0 near).
pub fn render_with_atlas(
    vertices: &[Vertex],
    view_proj: Mat4,
    width: u32,
    height: u32,
    atlas_size: [u32; 2],
    atlas_rgba: &[u8],
) -> Result<Vec<u8>> {
    if !vertices.len().is_multiple_of(4) {
        return Err("fixture vertices must form complete quads".into());
    }
    let quads = u32::try_from(vertices.len() / 4)?;
    let index_count = quads.checked_mul(6).ok_or("too many fixture quads")?;
    if width == 0 || height == 0 {
        return Err("fixture dimensions must be nonzero".into());
    }
    let [atlas_width, atlas_height] = atlas_size;
    let atlas_pixels = u64::from(atlas_width) * u64::from(atlas_height);
    if atlas_pixels == 0 || atlas_pixels.checked_mul(4) != Some(atlas_rgba.len() as u64) {
        return Err("fixture atlas needs nonzero dimensions and four bytes per pixel".into());
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
    let limits = device.limits();
    if width.max(height) > limits.max_texture_dimension_2d
        || u64::from(padded_row(width)) * u64::from(height) > limits.max_buffer_size
    {
        return Err("fixture dimensions exceed the device limits".into());
    }
    if (std::mem::size_of_val(vertices) as u64).max(u64::from(index_count) * 4)
        > limits.max_buffer_size
    {
        return Err("fixture geometry exceeds the device buffer limit".into());
    }
    let mut batch = EntityBatch::default();
    let [.., particle_quads, item_quads] = batch.update(vertices, &[], &[], &[]);
    let opaque_indices = (quads - item_quads - particle_quads) * 6;
    let particle_end = opaque_indices + particle_quads * 6;

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let atlas = shared_atlas(
        &device,
        &queue,
        HudAtlasSource::Pixels((atlas_rgba, atlas_width, atlas_height)),
        None,
    )?;
    let atlas_view = atlas.create_view(&Default::default());
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let depth_format = wgpu::TextureFormat::Depth32Float;
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = target(
        "fixture color",
        format,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = target(
        "fixture depth",
        depth_format,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fixture entity shader"),
        source: wgpu::ShaderSource::Wgsl(entity_shader().into()),
    });
    let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x2,2=>Uint32,3=>Unorm8x4,4=>Uint8x4,5=>Snorm8x4,6=>Float32x2];
    let make_pipeline = |write_depth| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fixture entity pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_entity"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(write_depth),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_entity"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let pipeline = make_pipeline(true);

    let white = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("fixture white texel"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &[255; 4],
    );
    let white_view = white.create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    let globals = Globals {
        view_proj: view_proj.to_cols_array_2d(),
        fog: [4096.0, 4097.0, 4096.0, 4097.0],
        ..Globals::zeroed()
    };
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fixture globals"),
        contents: bytemuck::bytes_of(&globals),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    use wgpu::BindingResource::{Sampler, TextureView};
    let entries: Vec<_> = [
        uniform.as_entire_binding(),
        TextureView(&atlas_view),
        Sampler(&sampler),
        TextureView(&white_view),
        Sampler(&sampler),
        TextureView(&white_view),
        Sampler(&sampler),
        TextureView(&white_view),
    ]
    .into_iter()
    .enumerate()
    .map(|(binding, resource)| wgpu::BindGroupEntry {
        binding: binding as u32,
        resource,
    })
    .collect();
    let make_group = |pipeline: &wgpu::RenderPipeline| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fixture entity resources"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        })
    };
    let group = make_group(&pipeline);
    let item_draw = (item_quads != 0).then(|| {
        let pipeline = make_pipeline(false);
        let group = make_group(&pipeline);
        (pipeline, group)
    });

    let geometry = (quads != 0).then(|| {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixture vertices"),
            contents: bytemuck::cast_slice(&batch.combined),
            usage: wgpu::BufferUsages::VERTEX,
        });
        (buffer, Renderer::quad_indices(&device, quads))
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some((vertices, indices)) = &geometry {
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..opaque_indices, 0, 0..1);
            if let Some((pipeline, group)) = &item_draw {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.draw_indexed(particle_end..index_count, 0, 0..1);
            }
            if particle_quads != 0 {
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw_indexed(opaque_indices..particle_end, 0, 0..1);
            }
        }
    }
    let readback = copy_frame(&device, &mut encoder, &color, [width, height]);
    queue.submit([encoder.finish()]);
    if let Some(error) = pollster::block_on(scope.pop()) {
        return Err(format!("fixture GPU validation failed: {error}").into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let data = readback.get_mapped_range(..)?;
    let row = width as usize * 4;
    let mut pixels = Vec::with_capacity(row * height as usize);
    for padded in data.chunks_exact(padded_row(width) as usize) {
        pixels.extend_from_slice(&padded[..row]);
    }
    drop(data);
    readback.unmap();
    Ok(pixels)
}
