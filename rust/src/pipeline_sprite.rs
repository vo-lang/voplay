//! Sprite rendering pipeline — instanced textured quads.
//!
//! Uses the same unit quad as shape2d but with texture sampling.
//! Sprites are batched per texture: each texture change flushes the current batch.

use bytemuck::{Pod, Zeroable};
use crate::pipeline2d::{QuadVertex, QUAD_VERTICES};
use crate::texture::{TextureId, TextureManager};

/// Per-instance data for a sprite.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SpriteInstance {
    pub dst_rect: [f32; 4],  // x, y, w, h in world/screen coords
    pub src_rect: [f32; 4],  // u0, v0, u1, v1 (normalized UV)
    pub color: [f32; 4],     // tint RGBA
    pub params: [f32; 4],    // rotation, flipX (0/1), flipY (0/1), _unused
}

impl SpriteInstance {
    const ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        1 => Float32x4,  // dst_rect
        2 => Float32x4,  // src_rect
        3 => Float32x4,  // color
        4 => Float32x4,  // params
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// A pending sprite draw, tagged with its texture ID for batching.
#[derive(Clone)]
pub struct SpriteDraw {
    pub texture_id: TextureId,
    pub instance: SpriteInstance,
}

/// Collects sprite draws per frame, then flushes in texture-batched order.
pub struct SpriteBatch {
    draws: Vec<SpriteDraw>,
}

impl SpriteBatch {
    pub fn new() -> Self {
        Self {
            draws: Vec::with_capacity(256),
        }
    }

    pub fn clear(&mut self) {
        self.draws.clear();
    }

    pub fn push(&mut self, draw: SpriteDraw) {
        self.draws.push(draw);
    }

    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Sort by texture ID so we can batch draws per texture.
    pub fn sort_by_texture(&mut self) {
        self.draws.sort_by_key(|d| d.texture_id);
    }

    pub fn draws(&self) -> &[SpriteDraw] {
        &self.draws
    }
}

const INITIAL_SPRITE_CAPACITY: usize = 1024;

/// The sprite rendering pipeline.
pub struct PipelineSprite {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

impl PipelineSprite {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_sprite_layout"),
            bind_group_layouts: &[camera_bind_group_layout, texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_sprite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[QuadVertex::layout(), SpriteInstance::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Static unit quad vertices (shared with shape2d)
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_sprite_quad_vb"),
            size: std::mem::size_of_val(&QUAD_VERTICES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&QUAD_VERTICES));

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_sprite_ib"),
            size: (INITIAL_SPRITE_CAPACITY * std::mem::size_of::<SpriteInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            instance_buffer,
            instance_capacity: INITIAL_SPRITE_CAPACITY,
        }
    }

    /// Upload a batch of sprite instances and draw them, grouped by texture.
    /// Must be called within an active render pass.
    pub fn draw_batch<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        batch: &mut SpriteBatch,
        textures: &'a TextureManager,
    ) {
        if batch.is_empty() {
            return;
        }

        batch.sort_by_texture();
        let draws = batch.draws();

        // Ensure instance buffer is large enough for the whole batch
        if draws.len() > self.instance_capacity {
            let new_capacity = draws.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("voplay_sprite_ib"),
                size: (new_capacity * std::mem::size_of::<SpriteInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_capacity;
        }

        // Upload all instances at once
        let all_instances: Vec<SpriteInstance> = draws.iter().map(|d| d.instance).collect();
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&all_instances));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));

        // Draw in texture-contiguous batches
        let mut batch_start = 0usize;
        while batch_start < draws.len() {
            let tex_id = draws[batch_start].texture_id;
            let gpu_tex = match textures.get(tex_id) {
                Some(t) => t,
                None => {
                    log::warn!("voplay: sprite references unknown texture {}", tex_id);
                    batch_start += 1;
                    continue;
                }
            };

            // Find end of this texture run
            let mut batch_end = batch_start + 1;
            while batch_end < draws.len() && draws[batch_end].texture_id == tex_id {
                batch_end += 1;
            }

            let count = (batch_end - batch_start) as u32;
            pass.set_bind_group(1, &gpu_tex.bind_group, &[]);
            pass.draw(0..6, batch_start as u32..batch_start as u32 + count);

            batch_start = batch_end;
        }
    }
}
