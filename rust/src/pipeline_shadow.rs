use bytemuck::{Pod, Zeroable};
use crate::animation;
use crate::model_loader::{MeshVertex, ModelManager, SkinnedMeshVertex};
use crate::pipeline3d::ModelDraw;

const MAX_JOINTS: usize = animation::MAX_JOINTS;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct LightVPUniform {
    pub light_vp: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShadowModelUniform {
    pub model: [[f32; 4]; 4],
    pub joint_count: [u32; 4],
    pub joints: [[[f32; 4]; 4]; MAX_JOINTS],
}

pub struct PipelineShadow {
    pipeline_static: wgpu::RenderPipeline,
    pipeline_skinned: wgpu::RenderPipeline,
    light_vp_buffer: wgpu::Buffer,
    light_vp_bind_group: wgpu::BindGroup,
    model_buffer: wgpu::Buffer,
    model_bind_group: wgpu::BindGroup,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    comparison_sampler: wgpu::Sampler,
    size: u32,
}

impl PipelineShadow {
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
        });

        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_shadow_light_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_shadow_model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_shadow_layout"),
            bind_group_layouts: &[&light_bgl, &model_bgl],
            push_constant_ranges: &[],
        });

        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        };

        let depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        });

        let pipeline_static = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shadow_static"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                buffers: &[MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let pipeline_skinned = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_shadow_skinned"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow_skinned"),
                buffers: &[SkinnedMeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let light_vp_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_light_vp_ub"),
            size: std::mem::size_of::<LightVPUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_vp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_shadow_light_vp_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_vp_buffer.as_entire_binding(),
            }],
        });

        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_shadow_model_ub"),
            size: std::mem::size_of::<ShadowModelUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_shadow_model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: model_buffer.as_entire_binding(),
            }],
        });

        let (depth_texture, depth_view) = Self::create_depth_texture(device, size.max(1));
        let comparison_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_shadow_compare_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        Self {
            pipeline_static,
            pipeline_skinned,
            light_vp_buffer,
            light_vp_bind_group,
            model_buffer,
            model_bind_group,
            depth_texture,
            depth_view,
            comparison_sampler,
            size: size.max(1),
        }
    }

    fn create_depth_texture(device: &wgpu::Device, size: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_shadow_depth"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        (depth_texture, depth_view)
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: u32) {
        let size = size.max(1);
        if size == self.size {
            return;
        }
        let (depth_texture, depth_view) = Self::create_depth_texture(device, size);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        self.size = size;
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn shadow_texture_view(&self) -> &wgpu::TextureView {
        &self.depth_view
    }

    pub fn comparison_sampler(&self) -> &wgpu::Sampler {
        &self.comparison_sampler
    }

    pub fn render_shadow_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        light_vp: &[[f32; 4]; 4],
        draws: &[ModelDraw],
        models: &ModelManager,
    ) {
        if draws.is_empty() {
            return;
        }

        let light_uniform = LightVPUniform { light_vp: *light_vp };
        queue.write_buffer(&self.light_vp_buffer, 0, bytemuck::bytes_of(&light_uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_shadow_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_bind_group(0, &self.light_vp_bind_group, &[]);

        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(model) => model,
                None => continue,
            };

            for mesh in &gpu_model.meshes {
                let mut uniform = ShadowModelUniform {
                    model: draw.model_uniform.model,
                    joint_count: [0, 0, 0, 0],
                    joints: [[[0.0; 4]; 4]; MAX_JOINTS],
                };

                if mesh.skinned {
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    assert!(joint_palette.len() <= MAX_JOINTS, "voplay: shadow joint palette exceeds MAX_JOINTS");
                    uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        uniform.joints[index] = *matrix;
                    }
                    pass.set_pipeline(&self.pipeline_skinned);
                } else {
                    pass.set_pipeline(&self.pipeline_static);
                }

                queue.write_buffer(&self.model_buffer, 0, bytemuck::bytes_of(&uniform));
                pass.set_bind_group(1, &self.model_bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
    }
}
