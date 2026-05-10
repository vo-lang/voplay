use crate::texture::GpuCubemap;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct InvVPUniform {
    pub inv_vp: [[f32; 4]; 4],
}

pub struct PipelineSkybox {
    pipeline: wgpu::RenderPipeline,
    pipeline_color: wgpu::RenderPipeline,
    inv_vp_buffer: wgpu::Buffer,
    inv_vp_bind_group: wgpu::BindGroup,
}

impl PipelineSkybox {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        receiver_mask_format: wgpu::TextureFormat,
        surface_props_format: wgpu::TextureFormat,
        cubemap_bgl: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_skybox"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/skybox.wgsl").into()),
        });

        let inv_vp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_skybox_inv_vp_bgl"),
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
            label: Some("voplay_skybox_layout"),
            bind_group_layouts: &[&inv_vp_bgl, cubemap_bgl],
            push_constant_ranges: &[],
        });

        let targets = [
            Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: receiver_mask_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: surface_props_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_skybox_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &targets,
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
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..wgpu::MultisampleState::default()
            },
            multiview: None,
            cache: None,
        });
        let color_targets = [
            Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            None,
            None,
        ];
        let pipeline_color = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_skybox_pipeline_color"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main_color"),
                targets: &color_targets,
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
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..wgpu::MultisampleState::default()
            },
            multiview: None,
            cache: None,
        });

        let inv_vp_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_skybox_inv_vp_ub"),
            size: std::mem::size_of::<InvVPUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let inv_vp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_skybox_inv_vp_bg"),
            layout: &inv_vp_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: inv_vp_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            pipeline_color,
            inv_vp_buffer,
            inv_vp_bind_group,
        }
    }

    pub fn set_camera(&self, queue: &wgpu::Queue, inv_vp: &[[f32; 4]; 4]) {
        let uniform = InvVPUniform { inv_vp: *inv_vp };
        queue.write_buffer(&self.inv_vp_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        cubemap: &'a GpuCubemap,
        aux_targets_enabled: bool,
    ) {
        pass.set_pipeline(if aux_targets_enabled {
            &self.pipeline
        } else {
            &self.pipeline_color
        });
        pass.set_bind_group(0, &self.inv_vp_bind_group, &[]);
        pass.set_bind_group(1, &cubemap.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
