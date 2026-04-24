//! 3D mesh rendering pipeline — Blinn-Phong forward renderer.
//!
//! Renders loaded glTF models with per-node transforms, directional/point lights,
//! and optional albedo textures. Uses depth testing for proper occlusion.

use crate::animation;
use crate::model_loader::{MeshVertex, ModelId, ModelManager, SkinnedMeshVertex};
use crate::texture::TextureManager;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

/// Maximum number of lights per frame.
const MAX_LIGHTS: usize = 8;
const MAX_JOINTS: usize = animation::MAX_JOINTS;

/// Camera uniform for 3D rendering (group 0).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct Camera3DUniform {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub _pad: f32,
}

/// Per-model transform uniform (group 1).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ModelUniform {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub material_params: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SkinnedModelUniform {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub material_params: [f32; 4],
    pub joint_count: [u32; 4],
    pub joints: [[[f32; 4]; 4]; MAX_JOINTS],
}

/// Single light data matching the shader struct.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct LightData {
    pub position_or_dir: [f32; 4], // xyz = pos/dir, w = type (0=dir, 1=point)
    pub color_intensity: [f32; 4], // rgb = color, a = intensity
}

/// Light uniform buffer (group 2).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct LightUniform {
    pub ambient: [f32; 4], // rgb = ambient color, a = unused
    pub count: [u32; 4],   // x = number of lights, y = fog mode
    pub lights: [LightData; MAX_LIGHTS],
    pub fog_color: [f32; 4],
    pub fog_params: [f32; 4],
    pub shadow_vp: [[f32; 4]; 4],
    pub shadow_params: [f32; 4],
}

/// A pending 3D model draw.
pub struct ModelDraw {
    pub model_id: ModelId,
    pub model_uniform: ModelUniform,
    pub tint: [f32; 4],
    pub animation_world_id: u32,
    pub animation_target_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct InstanceData {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
    normal_0: [f32; 4],
    normal_1: [f32; 4],
    normal_2: [f32; 4],
    normal_3: [f32; 4],
    base_color: [f32; 4],
    material_params: [f32; 4],
}

impl InstanceData {
    const ATTRIBS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Float32x4,
        11 => Float32x4,
        12 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }

    fn from_uniform(uniform: &ModelUniform) -> Self {
        Self {
            model_0: uniform.model[0],
            model_1: uniform.model[1],
            model_2: uniform.model[2],
            model_3: uniform.model[3],
            normal_0: uniform.normal_matrix[0],
            normal_1: uniform.normal_matrix[1],
            normal_2: uniform.normal_matrix[2],
            normal_3: uniform.normal_matrix[3],
            base_color: uniform.base_color,
            material_params: uniform.material_params,
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct InstanceBatchKey {
    model_id: ModelId,
    mesh_index: usize,
    texture_id: u32,
    textured: bool,
}

struct InstanceBatch {
    key: InstanceBatchKey,
    instances: Vec<InstanceData>,
}

struct InstanceBatchDraw {
    key: InstanceBatchKey,
    start: u32,
    count: u32,
}

/// The 3D mesh rendering pipeline.
pub struct Pipeline3D {
    pipeline_textured: wgpu::RenderPipeline,
    pipeline_untextured: wgpu::RenderPipeline,
    pipeline_instanced_textured: wgpu::RenderPipeline,
    pipeline_instanced_untextured: wgpu::RenderPipeline,
    pipeline_terrain_splat: wgpu::RenderPipeline,
    pipeline_skinned_textured: wgpu::RenderPipeline,
    pipeline_skinned_untextured: wgpu::RenderPipeline,
    // GPU buffers
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    model_bgl: wgpu::BindGroupLayout,
    model_buffer: wgpu::Buffer,
    model_bind_group: wgpu::BindGroup,
    model_buffer_alignment: u32,
    model_buffer_slot_count: u32,
    skinned_model_buffer: wgpu::Buffer,
    skinned_model_bind_group: wgpu::BindGroup,
    skinned_model_buffer_slot_count: u32,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: u32,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    main_texture_bind_group_layout: wgpu::BindGroupLayout,
    terrain_texture_bind_group_layout: wgpu::BindGroupLayout,
    // 1x1 white fallback texture for untextured meshes
    white_texture_view: wgpu::TextureView,
    white_sampler: wgpu::Sampler,
}

impl Pipeline3D {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let static_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });
        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh_terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh_terrain.wgsl").into()),
        });
        let skinned_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_mesh_skinned"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh_skinned.wgsl").into()),
        });

        // Group 0: Camera
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Group 1: Model transform (dynamic offset for per-draw uniforms)
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let model_buffer_alignment = device.limits().min_uniform_buffer_offset_alignment;

        // Group 2: Lights
        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_mesh_light_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let main_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay_mesh_main_texture_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
        let terrain_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay_mesh_terrain_texture_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 8,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 9,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 10,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // Group 3: Main texture + shadow map
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_mesh_layout"),
            bind_group_layouts: &[
                &camera_bgl,
                &model_bgl,
                &light_bgl,
                &main_texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let terrain_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("voplay_mesh_terrain_layout"),
                bind_group_layouts: &[
                    &camera_bgl,
                    &model_bgl,
                    &light_bgl,
                    &terrain_texture_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });

        let depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });

        let vertex_state = wgpu::VertexState {
            module: &static_shader,
            entry_point: Some("vs_main"),
            buffers: &[MeshVertex::layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        };
        let instanced_vertex_state = wgpu::VertexState {
            module: &static_shader,
            entry_point: Some("vs_instanced"),
            buffers: &[MeshVertex::layout(), InstanceData::layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        };

        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        };

        // Textured pipeline
        let pipeline_textured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_mesh_textured"),
            layout: Some(&pipeline_layout),
            vertex: vertex_state.clone(),
            fragment: Some(wgpu::FragmentState {
                module: &static_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let pipeline_instanced_textured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_textured"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state.clone(),
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let pipeline_terrain_splat =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_terrain_splat"),
                layout: Some(&terrain_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &terrain_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &terrain_shader,
                    entry_point: Some("fs_main_terrain"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let pipeline_skinned_textured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_textured"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let pipeline_skinned_untextured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_skinned_untextured"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &skinned_shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedMeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &skinned_shader,
                    entry_point: Some("fs_skinned_no_tex"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Untextured pipeline (uses fs_main_no_tex)
        let pipeline_untextured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voplay_mesh_untextured"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &static_shader,
                entry_point: Some("vs_main"),
                buffers: &[MeshVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &static_shader,
                entry_point: Some("fs_main_no_tex"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let pipeline_instanced_untextured =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("voplay_mesh_instanced_untextured"),
                layout: Some(&pipeline_layout),
                vertex: instanced_vertex_state,
                fragment: Some(wgpu::FragmentState {
                    module: &static_shader,
                    entry_point: Some("fs_instanced_no_tex"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive,
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Create uniform buffers
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_camera_ub"),
            size: std::mem::size_of::<Camera3DUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let model_buffer_slot_count: u32 = 256;
        let aligned_model_size = Self::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            model_buffer_alignment,
        );
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_model_ub"),
            size: aligned_model_size as u64 * model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = Self::create_model_bind_group(
            device,
            &model_bgl,
            &model_buffer,
            std::mem::size_of::<ModelUniform>() as u64,
            "voplay_mesh_model_bg",
        );

        let skinned_model_buffer_slot_count: u32 = 32;
        let aligned_skinned_size = Self::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            model_buffer_alignment,
        );
        let skinned_model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_skinned_model_ub"),
            size: aligned_skinned_size as u64 * skinned_model_buffer_slot_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skinned_model_bind_group = Self::create_model_bind_group(
            device,
            &model_bgl,
            &skinned_model_buffer,
            std::mem::size_of::<SkinnedModelUniform>() as u64,
            "voplay_mesh_skinned_model_bg",
        );

        let instance_buffer_capacity: u32 = 1024;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_instance_vb"),
            size: std::mem::size_of::<InstanceData>() as u64 * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_light_ub"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_light_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
        });

        // 1x1 white texture for untextured meshes
        let white_data = [255u8; 4]; // RGBA white
        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_white_1x1"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &white_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let white_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_white_sampler"),
            ..Default::default()
        });

        Self {
            pipeline_textured,
            pipeline_untextured,
            pipeline_instanced_textured,
            pipeline_instanced_untextured,
            pipeline_terrain_splat,
            pipeline_skinned_textured,
            pipeline_skinned_untextured,
            camera_buffer,
            camera_bind_group,
            model_bgl,
            model_buffer,
            model_bind_group,
            model_buffer_alignment,
            model_buffer_slot_count,
            skinned_model_buffer,
            skinned_model_bind_group,
            skinned_model_buffer_slot_count,
            instance_buffer,
            instance_buffer_capacity,
            light_buffer,
            light_bind_group,
            main_texture_bind_group_layout,
            terrain_texture_bind_group_layout,
            white_texture_view: white_view,
            white_sampler,
        }
    }

    fn align_up(value: u32, alignment: u32) -> u32 {
        (value + alignment - 1) & !(alignment - 1)
    }

    fn create_model_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        binding_size: u64,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(binding_size),
                }),
            }],
        })
    }

    fn ensure_model_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.model_buffer_slot_count {
            return;
        }
        let new_count = needed.next_power_of_two().max(256);
        let aligned = Self::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.model_buffer,
            std::mem::size_of::<ModelUniform>() as u64,
            "voplay_mesh_model_bg",
        );
        self.model_buffer_slot_count = new_count;
    }

    fn ensure_skinned_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.skinned_model_buffer_slot_count {
            return;
        }
        let new_count = needed.next_power_of_two().max(32);
        let aligned = Self::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        self.skinned_model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_skinned_model_ub"),
            size: aligned as u64 * new_count as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.skinned_model_bind_group = Self::create_model_bind_group(
            device,
            &self.model_bgl,
            &self.skinned_model_buffer,
            std::mem::size_of::<SkinnedModelUniform>() as u64,
            "voplay_mesh_skinned_model_bg",
        );
        self.skinned_model_buffer_slot_count = new_count;
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_instance_vb"),
            size: std::mem::size_of::<InstanceData>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    fn create_main_texture_bind_group(
        &self,
        device: &wgpu::Device,
        texture_view: &wgpu::TextureView,
        texture_sampler: &wgpu::Sampler,
        shadow_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_main_texture_bg"),
            layout: &self.main_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(texture_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
            ],
        })
    }

    fn create_terrain_texture_bind_group(
        &self,
        device: &wgpu::Device,
        control_view: &wgpu::TextureView,
        control_sampler: &wgpu::Sampler,
        shadow_view: &wgpu::TextureView,
        layer_views: [&wgpu::TextureView; 4],
        layer_sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_mesh_terrain_texture_bg"),
            layout: &self.terrain_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(control_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(control_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(layer_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(layer_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(layer_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(layer_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(layer_views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(layer_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(layer_views[3]),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(layer_sampler),
                },
            ],
        })
    }

    /// Upload camera and light uniforms for this frame.
    pub fn set_camera_and_lights(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera3DUniform,
        lights: &LightUniform,
    ) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
        queue.write_buffer(&self.light_buffer, 0, bytemuck::bytes_of(lights));
    }

    /// Draw a list of models within an active render pass.
    pub fn draw_models<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[ModelDraw],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
    ) {
        if draws.is_empty() {
            return;
        }

        let aligned_model_stride = Self::align_up(
            std::mem::size_of::<ModelUniform>() as u32,
            self.model_buffer_alignment,
        );
        let aligned_skinned_stride = Self::align_up(
            std::mem::size_of::<SkinnedModelUniform>() as u32,
            self.model_buffer_alignment,
        );

        let mut instance_batches: Vec<InstanceBatch> = Vec::new();
        let mut instance_batch_index: HashMap<InstanceBatchKey, usize> = HashMap::new();
        let mut instance_count: u32 = 0;
        let mut static_slot: u32 = 0;
        let mut skinned_slot: u32 = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                let base_color = [
                    mesh.material.base_color[0] * draw.tint[0],
                    mesh.material.base_color[1] * draw.tint[1],
                    mesh.material.base_color[2] * draw.tint[2],
                    mesh.material.base_color[3] * draw.tint[3],
                ];
                if mesh.skinned {
                    skinned_slot += 1;
                    continue;
                }
                if mesh.material.control_texture_id.is_some() {
                    static_slot += 1;
                    continue;
                }

                let texture_id = mesh
                    .material
                    .texture_id
                    .and_then(|tex_id| textures.get(tex_id).map(|_| tex_id))
                    .unwrap_or(0);
                let key = InstanceBatchKey {
                    model_id: draw.model_id,
                    mesh_index,
                    texture_id,
                    textured: texture_id != 0,
                };
                let batch_index = if let Some(index) = instance_batch_index.get(&key) {
                    *index
                } else {
                    let index = instance_batches.len();
                    instance_batches.push(InstanceBatch {
                        key,
                        instances: Vec::new(),
                    });
                    instance_batch_index.insert(key, index);
                    index
                };
                let mut model_uniform = draw.model_uniform;
                model_uniform.base_color = base_color;
                model_uniform.material_params = mesh.material.uv_scales;
                instance_batches[batch_index]
                    .instances
                    .push(InstanceData::from_uniform(&model_uniform));
                instance_count += 1;
            }
        }

        self.ensure_model_capacity(device, static_slot);
        self.ensure_skinned_capacity(device, skinned_slot);

        let mut instance_data = Vec::with_capacity(instance_count as usize);
        let mut instance_batch_draws = Vec::with_capacity(instance_batches.len());
        for batch in &instance_batches {
            let start = instance_data.len() as u32;
            instance_data.extend_from_slice(&batch.instances);
            instance_batch_draws.push(InstanceBatchDraw {
                key: batch.key,
                start,
                count: batch.instances.len() as u32,
            });
        }
        if !instance_data.is_empty() {
            self.ensure_instance_capacity(device, instance_data.len() as u32);
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instance_data),
            );
        }

        static_slot = 0;
        skinned_slot = 0;
        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };
            for mesh in &gpu_model.meshes {
                let base_color = [
                    mesh.material.base_color[0] * draw.tint[0],
                    mesh.material.base_color[1] * draw.tint[1],
                    mesh.material.base_color[2] * draw.tint[2],
                    mesh.material.base_color[3] * draw.tint[3],
                ];
                if mesh.skinned {
                    let mut skinned_uniform = SkinnedModelUniform {
                        model: draw.model_uniform.model,
                        normal_matrix: draw.model_uniform.normal_matrix,
                        base_color,
                        material_params: mesh.material.uv_scales,
                        joint_count: [0, 0, 0, 0],
                        joints: [[[0.0; 4]; 4]; MAX_JOINTS],
                    };
                    let palette = if draw.animation_world_id != 0 && draw.animation_target_id != 0 {
                        animation::get_palette(draw.animation_world_id, draw.animation_target_id)
                    } else {
                        None
                    };
                    let joint_palette = palette.as_ref().unwrap_or(&gpu_model.rest_joint_palette);
                    assert!(
                        joint_palette.len() <= MAX_JOINTS,
                        "voplay: joint palette exceeds MAX_JOINTS"
                    );
                    skinned_uniform.joint_count[0] = joint_palette.len() as u32;
                    for (index, matrix) in joint_palette.iter().enumerate() {
                        skinned_uniform.joints[index] = *matrix;
                    }
                    let offset = skinned_slot as u64 * aligned_skinned_stride as u64;
                    queue.write_buffer(
                        &self.skinned_model_buffer,
                        offset,
                        bytemuck::bytes_of(&skinned_uniform),
                    );
                    skinned_slot += 1;
                } else {
                    if mesh.material.control_texture_id.is_none() {
                        continue;
                    }
                    let mut model_uniform = draw.model_uniform;
                    model_uniform.base_color = base_color;
                    model_uniform.material_params = mesh.material.uv_scales;
                    let offset = static_slot as u64 * aligned_model_stride as u64;
                    queue.write_buffer(
                        &self.model_buffer,
                        offset,
                        bytemuck::bytes_of(&model_uniform),
                    );
                    static_slot += 1;
                }
            }
        }

        let mut main_texture_bind_groups: HashMap<u32, wgpu::BindGroup> = HashMap::new();
        let mut terrain_texture_bind_groups: HashMap<[u32; 5], wgpu::BindGroup> = HashMap::new();

        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);

        static_slot = 0;
        skinned_slot = 0;

        for draw in draws {
            let gpu_model = match models.get(draw.model_id) {
                Some(m) => m,
                None => continue,
            };

            for mesh in &gpu_model.meshes {
                if mesh.skinned {
                    let dyn_offset = skinned_slot * aligned_skinned_stride;
                    pass.set_bind_group(1, &self.skinned_model_bind_group, &[dyn_offset]);
                    skinned_slot += 1;

                    let use_textured_pipeline = mesh
                        .material
                        .texture_id
                        .and_then(|tex_id| textures.get(tex_id))
                        .is_some();
                    let (texture_key, texture_view, texture_sampler) =
                        if let Some(tex_id) = mesh.material.texture_id {
                            if let Some(gpu_tex) = textures.get(tex_id) {
                                (tex_id, &gpu_tex.view, textures.sampler())
                            } else {
                                (0, &self.white_texture_view, &self.white_sampler)
                            }
                        } else {
                            (0, &self.white_texture_view, &self.white_sampler)
                        };
                    let main_texture_bind_group = main_texture_bind_groups
                        .entry(texture_key)
                        .or_insert_with(|| {
                            self.create_main_texture_bind_group(
                                device,
                                texture_view,
                                texture_sampler,
                                shadow_view,
                            )
                        });
                    if use_textured_pipeline {
                        pass.set_pipeline(&self.pipeline_skinned_textured);
                    } else {
                        pass.set_pipeline(&self.pipeline_skinned_untextured);
                    }
                    pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                } else {
                    if mesh.material.control_texture_id.is_none() {
                        continue;
                    }
                    let dyn_offset = static_slot * aligned_model_stride;
                    pass.set_bind_group(1, &self.model_bind_group, &[dyn_offset]);
                    static_slot += 1;

                    if let Some(control_id) = mesh.material.control_texture_id {
                        pass.set_pipeline(&self.pipeline_terrain_splat);
                        let (texture_key, texture_view, texture_sampler) =
                            if let Some(gpu_tex) = textures.get(control_id) {
                                (control_id, &gpu_tex.view, textures.sampler())
                            } else {
                                (0, &self.white_texture_view, &self.white_sampler)
                            };
                        let mut layer_texture_ids = [0u32; 4];
                        let mut layer_texture_views = [&self.white_texture_view; 4];
                        for (index, tex_id) in mesh.material.layer_texture_ids.iter().enumerate() {
                            if let Some(id) = *tex_id {
                                if let Some(gpu_tex) = textures.get(id) {
                                    layer_texture_ids[index] = id;
                                    layer_texture_views[index] = &gpu_tex.view;
                                }
                            }
                        }
                        let terrain_key = [
                            texture_key,
                            layer_texture_ids[0],
                            layer_texture_ids[1],
                            layer_texture_ids[2],
                            layer_texture_ids[3],
                        ];
                        let terrain_texture_bind_group = terrain_texture_bind_groups
                            .entry(terrain_key)
                            .or_insert_with(|| {
                                self.create_terrain_texture_bind_group(
                                    device,
                                    texture_view,
                                    texture_sampler,
                                    shadow_view,
                                    layer_texture_views,
                                    textures.sampler(),
                                )
                            });
                        pass.set_bind_group(3, &*terrain_texture_bind_group, &[]);
                    } else if let Some(tex_id) = mesh.material.texture_id {
                        if let Some(gpu_tex) = textures.get(tex_id) {
                            let main_texture_bind_group =
                                main_texture_bind_groups.entry(tex_id).or_insert_with(|| {
                                    self.create_main_texture_bind_group(
                                        device,
                                        &gpu_tex.view,
                                        textures.sampler(),
                                        shadow_view,
                                    )
                                });
                            pass.set_pipeline(&self.pipeline_textured);
                            pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                        } else {
                            let main_texture_bind_group =
                                main_texture_bind_groups.entry(0).or_insert_with(|| {
                                    self.create_main_texture_bind_group(
                                        device,
                                        &self.white_texture_view,
                                        &self.white_sampler,
                                        shadow_view,
                                    )
                                });
                            pass.set_pipeline(&self.pipeline_untextured);
                            pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                        }
                    } else {
                        let main_texture_bind_group =
                            main_texture_bind_groups.entry(0).or_insert_with(|| {
                                self.create_main_texture_bind_group(
                                    device,
                                    &self.white_texture_view,
                                    &self.white_sampler,
                                    shadow_view,
                                )
                            });
                        pass.set_pipeline(&self.pipeline_untextured);
                        pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                    }
                }

                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        if instance_batch_draws.is_empty() {
            return;
        }

        pass.set_bind_group(1, &self.model_bind_group, &[0]);
        let instance_stride = std::mem::size_of::<InstanceData>() as u64;

        for batch in &instance_batch_draws {
            let gpu_model = match models.get(batch.key.model_id) {
                Some(m) => m,
                None => continue,
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                continue;
            };
            if batch.key.textured {
                if let Some(gpu_tex) = textures.get(batch.key.texture_id) {
                    let main_texture_bind_group = main_texture_bind_groups
                        .entry(batch.key.texture_id)
                        .or_insert_with(|| {
                            self.create_main_texture_bind_group(
                                device,
                                &gpu_tex.view,
                                textures.sampler(),
                                shadow_view,
                            )
                        });
                    pass.set_pipeline(&self.pipeline_instanced_textured);
                    pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                } else {
                    let main_texture_bind_group =
                        main_texture_bind_groups.entry(0).or_insert_with(|| {
                            self.create_main_texture_bind_group(
                                device,
                                &self.white_texture_view,
                                &self.white_sampler,
                                shadow_view,
                            )
                        });
                    pass.set_pipeline(&self.pipeline_instanced_untextured);
                    pass.set_bind_group(3, &*main_texture_bind_group, &[]);
                }
            } else {
                let main_texture_bind_group =
                    main_texture_bind_groups.entry(0).or_insert_with(|| {
                        self.create_main_texture_bind_group(
                            device,
                            &self.white_texture_view,
                            &self.white_sampler,
                            shadow_view,
                        )
                    });
                pass.set_pipeline(&self.pipeline_instanced_untextured);
                pass.set_bind_group(3, &*main_texture_bind_group, &[]);
            }

            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
        }
    }
}
