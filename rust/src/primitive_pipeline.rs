use std::cmp::Ordering;
use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::material::{MaterialSamplerKey, MATERIAL_SAMPLER_KEYS};
use crate::model_loader::{MeshMaterial, MeshVertex, ModelId, ModelManager};
use crate::pipeline3d::{Camera3DUniform, LightUniform, MaterialOverride, ModelUniform};
use crate::primitive_scene::{
    PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate, PRIMITIVE_FLAG_ATLAS_UV,
    PRIMITIVE_FLAG_BILLBOARD, PRIMITIVE_FLAG_NO_SHADOW, PRIMITIVE_FLAG_WATER_SURFACE,
    PRIMITIVE_FLAG_Y_BILLBOARD,
};
use crate::renderer_perf::{elapsed_ms, perf_now};
use crate::texture::TextureManager;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PrimitiveInstanceGpu {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
    base_color: [f32; 4],
    material_params: [f32; 4],
    emissive_color: [f32; 4],
    texture_flags: [f32; 4],
    instance_params: [f32; 4],
    instance_params2: [f32; 4],
}

impl PrimitiveInstanceGpu {
    const ATTRIBS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Float32x4,
        11 => Float32x4,
        12 => Float32x4,
        13 => Float32x4,
        14 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }

    fn from_draw(draw: &PrimitiveDraw, uniform: &ModelUniform) -> Self {
        Self {
            model_0: uniform.model[0],
            model_1: uniform.model[1],
            model_2: uniform.model[2],
            model_3: uniform.model[3],
            base_color: uniform.base_color,
            material_params: uniform.material_params,
            emissive_color: uniform.emissive_color,
            texture_flags: uniform.texture_flags,
            instance_params: draw.instance_params,
            instance_params2: draw.instance_params2,
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveTextureKey {
    albedo: u32,
    normal: u32,
    metallic_roughness: u32,
    emissive: u32,
    toon_ramp: u32,
    sampler: MaterialSamplerKey,
}

impl PrimitiveTextureKey {
    fn has_albedo(self) -> bool {
        self.albedo != 0
    }

    fn texture_flags(self, normal_scale: f32) -> [f32; 4] {
        [
            if self.normal != 0 {
                normal_scale.max(0.0)
            } else {
                0.0
            },
            if self.metallic_roughness != 0 {
                1.0
            } else {
                0.0
            },
            if self.emissive != 0 { 1.0 } else { 0.0 },
            if self.toon_ramp != 0 { 1.0 } else { 0.0 },
        ]
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum PrimitiveRenderMode {
    Opaque,
    Cutout,
    Translucent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveRenderFilter {
    Main,
    Water,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimitiveDrawStats {
    pub batch_count: u32,
    pub instance_count: u32,
    pub triangle_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PrimitiveLayeredDrawStats {
    pub main: PrimitiveDrawStats,
    pub water: PrimitiveDrawStats,
    pub main_cpu_ms: f64,
    pub water_cpu_ms: f64,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveBatchKey {
    model_id: ModelId,
    mesh_index: usize,
    textures: PrimitiveTextureKey,
    mode: PrimitiveRenderMode,
}

struct PrimitiveBatch {
    key: PrimitiveBatchKey,
    instances: Vec<PrimitiveInstanceGpu>,
}

struct PrimitiveBatchDraw {
    key: PrimitiveBatchKey,
    start: u32,
    count: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct PrimitivePassInstanceGpu {
    model_0: [f32; 4],
    model_1: [f32; 4],
    model_2: [f32; 4],
    model_3: [f32; 4],
}

impl PrimitivePassInstanceGpu {
    fn from_model(model: [[f32; 4]; 4]) -> Self {
        Self {
            model_0: model[0],
            model_1: model[1],
            model_2: model[2],
            model_3: model[3],
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct PrimitivePassBatchKey {
    pub model_id: ModelId,
    pub mesh_index: usize,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct PrimitiveObjectKey {
    scene_id: u32,
    layer_id: u32,
    object_id: u32,
}

#[derive(Clone, Copy)]
struct ResidentPrimitiveInstance {
    object_id: u32,
    draw: PrimitiveDraw,
}

struct ResidentPrimitiveChunk {
    instances: Vec<ResidentPrimitiveInstance>,
    depth_batches: Vec<ResidentPrimitivePassBatch>,
    shadow_batches: Vec<ResidentPrimitivePassBatch>,
}

struct ResidentPrimitivePassBatch {
    key: PrimitivePassBatchKey,
    buffer: wgpu::Buffer,
    count: u32,
}

struct ResidentPrimitivePassBatchRef<'a> {
    batch: &'a ResidentPrimitivePassBatch,
}

fn color_targets(
    surface_format: wgpu::TextureFormat,
    receiver_mask_format: wgpu::TextureFormat,
    surface_props_format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> [Option<wgpu::ColorTargetState>; 3] {
    [
        Some(wgpu::ColorTargetState {
            format: surface_format,
            blend,
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
    ]
}

fn color_only_targets(
    surface_format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> [Option<wgpu::ColorTargetState>; 3] {
    [
        Some(wgpu::ColorTargetState {
            format: surface_format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        }),
        None,
        None,
    ]
}

fn primitive_state_for_mode(mode: PrimitiveRenderMode) -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        strip_index_format: None,
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: if mode == PrimitiveRenderMode::Opaque {
            Some(wgpu::Face::Back)
        } else {
            None
        },
        polygon_mode: wgpu::PolygonMode::Fill,
        unclipped_depth: false,
        conservative: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn create_primitive_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex: wgpu::VertexState<'_>,
    surface_format: wgpu::TextureFormat,
    receiver_mask_format: wgpu::TextureFormat,
    surface_props_format: wgpu::TextureFormat,
    depth_stencil: Option<wgpu::DepthStencilState>,
    multisample: wgpu::MultisampleState,
    mode: PrimitiveRenderMode,
    textured: bool,
    aux_targets_enabled: bool,
) -> wgpu::RenderPipeline {
    let blend = if mode == PrimitiveRenderMode::Translucent {
        Some(wgpu::BlendState::ALPHA_BLENDING)
    } else {
        None
    };
    let targets = if aux_targets_enabled {
        color_targets(
            surface_format,
            receiver_mask_format,
            surface_props_format,
            blend,
        )
    } else {
        color_only_targets(surface_format, blend)
    };
    let texture_suffix = if textured { "textured" } else { "untextured" };
    let mode_suffix = match mode {
        PrimitiveRenderMode::Opaque => "opaque",
        PrimitiveRenderMode::Cutout => "cutout",
        PrimitiveRenderMode::Translucent => "translucent",
    };
    let target_suffix = if aux_targets_enabled { "full" } else { "color" };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!(
            "voplay_primitive_instanced_{texture_suffix}_{mode_suffix}_{target_suffix}"
        )),
        layout: Some(layout),
        vertex,
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(match (textured, aux_targets_enabled) {
                (true, true) => "fs_instanced",
                (true, false) => "fs_instanced_color",
                (false, true) => "fs_instanced_no_tex",
                (false, false) => "fs_instanced_no_tex_color",
            }),
            targets: &targets,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: primitive_state_for_mode(mode),
        depth_stencil,
        multisample,
        multiview: None,
        cache: None,
    })
}

pub struct PrimitivePipeline {
    pipeline_textured_opaque: wgpu::RenderPipeline,
    pipeline_textured_opaque_color: wgpu::RenderPipeline,
    pipeline_untextured_opaque: wgpu::RenderPipeline,
    pipeline_untextured_opaque_color: wgpu::RenderPipeline,
    pipeline_textured_cutout: wgpu::RenderPipeline,
    pipeline_textured_cutout_color: wgpu::RenderPipeline,
    pipeline_untextured_cutout: wgpu::RenderPipeline,
    pipeline_untextured_cutout_color: wgpu::RenderPipeline,
    pipeline_textured_translucent: wgpu::RenderPipeline,
    pipeline_textured_translucent_color: wgpu::RenderPipeline,
    pipeline_untextured_translucent: wgpu::RenderPipeline,
    pipeline_untextured_translucent_color: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    model_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: u32,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    main_texture_bind_group_layout: wgpu::BindGroupLayout,
    material_samplers: Vec<wgpu::Sampler>,
    white_texture_view: wgpu::TextureView,
    resident_chunks: HashMap<PrimitiveChunkRef, ResidentPrimitiveChunk>,
    object_chunks: HashMap<PrimitiveObjectKey, PrimitiveChunkRef>,
    texture_bind_groups: HashMap<PrimitiveTextureKey, wgpu::BindGroup>,
    last_main_batch_count: u32,
    last_main_instance_count: u32,
    last_main_triangle_count: u32,
}

impl PrimitivePipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        receiver_mask_format: wgpu::TextureFormat,
        surface_props_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay_primitive_mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_camera_bgl"),
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
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_model_bgl"),
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
        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_primitive_light_bgl"),
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
        let main_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay_primitive_texture_bgl"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
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
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voplay_primitive_layout"),
            bind_group_layouts: &[
                &camera_bgl,
                &model_bgl,
                &light_bgl,
                &main_texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let multisample = wgpu::MultisampleState {
            count: sample_count,
            ..wgpu::MultisampleState::default()
        };
        let vertex = wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_instanced_primitive"),
            buffers: &[MeshVertex::layout(), PrimitiveInstanceGpu::layout()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        };
        let pipeline_textured_opaque = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            true,
            true,
        );
        let pipeline_textured_opaque_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            true,
            false,
        );
        let pipeline_untextured_opaque = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            false,
            true,
        );
        let pipeline_untextured_opaque_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Opaque,
            false,
            false,
        );
        let pipeline_textured_cutout = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            true,
            true,
        );
        let pipeline_textured_cutout_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            true,
            false,
        );
        let pipeline_untextured_cutout = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            false,
            true,
        );
        let pipeline_untextured_cutout_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Cutout,
            false,
            false,
        );
        let pipeline_textured_translucent = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            true,
            true,
        );
        let pipeline_textured_translucent_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            true,
            false,
        );
        let pipeline_untextured_translucent = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex.clone(),
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil.clone(),
            multisample,
            PrimitiveRenderMode::Translucent,
            false,
            true,
        );
        let pipeline_untextured_translucent_color = create_primitive_render_pipeline(
            device,
            &pipeline_layout,
            &shader,
            vertex,
            surface_format,
            receiver_mask_format,
            surface_props_format,
            depth_stencil,
            multisample,
            PrimitiveRenderMode::Translucent,
            false,
            false,
        );

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_camera_ub"),
            size: std::mem::size_of::<Camera3DUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_camera_bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let model_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let model_size = align_up(std::mem::size_of::<ModelUniform>() as u32, model_alignment);
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_dummy_model_ub"),
            size: model_size as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_dummy_model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ModelUniform>() as u64),
                }),
            }],
        });
        queue.write_buffer(
            &model_buffer,
            0,
            bytemuck::bytes_of(&ModelUniform::zeroed()),
        );

        let instance_buffer_capacity: u32 = 1024;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_instance_vb"),
            size: std::mem::size_of::<PrimitiveInstanceGpu>() as u64
                * instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_light_ub"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_light_bg"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
        });
        let material_samplers = MATERIAL_SAMPLER_KEYS
            .iter()
            .map(|key| create_material_sampler(device, *key))
            .collect();
        let white_data = [255u8; 4];
        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_primitive_white_1x1"),
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
        let white_texture_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            pipeline_textured_opaque,
            pipeline_textured_opaque_color,
            pipeline_untextured_opaque,
            pipeline_untextured_opaque_color,
            pipeline_textured_cutout,
            pipeline_textured_cutout_color,
            pipeline_untextured_cutout,
            pipeline_untextured_cutout_color,
            pipeline_textured_translucent,
            pipeline_textured_translucent_color,
            pipeline_untextured_translucent,
            pipeline_untextured_translucent_color,
            camera_buffer,
            camera_bind_group,
            model_bind_group,
            instance_buffer,
            instance_buffer_capacity,
            light_buffer,
            light_bind_group,
            main_texture_bind_group_layout,
            material_samplers,
            white_texture_view,
            resident_chunks: HashMap::new(),
            object_chunks: HashMap::new(),
            texture_bind_groups: HashMap::new(),
            last_main_batch_count: 0,
            last_main_instance_count: 0,
            last_main_triangle_count: 0,
        }
    }

    pub fn clear_texture_bind_group_cache(&mut self) {
        self.texture_bind_groups.clear();
    }

    pub fn set_camera_and_lights(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera3DUniform,
        lights: &LightUniform,
    ) {
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(camera));
        queue.write_buffer(&self.light_buffer, 0, bytemuck::bytes_of(lights));
    }

    pub fn last_main_batch_count(&self) -> u32 {
        self.last_main_batch_count
    }

    pub fn last_main_instance_count(&self) -> u32 {
        self.last_main_instance_count
    }

    pub fn last_main_triangle_count(&self) -> u32 {
        self.last_main_triangle_count
    }

    pub fn append_resident_depth_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
    ) {
        self.append_resident_pass_draws(chunk_refs, out, false);
    }

    pub fn append_resident_shadow_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
    ) {
        self.append_resident_pass_draws(chunk_refs, out, true);
    }

    fn append_resident_pass_draws(
        &self,
        chunk_refs: &[PrimitiveChunkRef],
        out: &mut Vec<PrimitiveDraw>,
        shadow_only: bool,
    ) {
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for instance in &chunk.instances {
                if primitive_participates_in_depth_pass(&instance.draw, shadow_only) {
                    out.push(instance.draw);
                }
            }
        }
    }

    pub fn replace_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: &[PrimitiveObjectUpdate],
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let chunk_ref = PrimitiveChunkRef {
            scene_id,
            layer_id,
            chunk_id,
        };
        if let Some(previous) = self.resident_chunks.remove(&chunk_ref) {
            for instance in previous.instances {
                self.object_chunks.remove(&PrimitiveObjectKey {
                    scene_id,
                    layer_id,
                    object_id: instance.object_id,
                });
            }
        }
        for update in updates {
            let object_key = PrimitiveObjectKey {
                scene_id,
                layer_id,
                object_id: update.object_id,
            };
            if let Some(previous_chunk) = self.object_chunks.get(&object_key).copied() {
                if previous_chunk != chunk_ref {
                    self.remove_object_from_resident_chunk(
                        device,
                        queue,
                        object_key,
                        previous_chunk,
                        models,
                    );
                }
            }
        }
        let instances = updates
            .iter()
            .filter(|update| update.visible)
            .map(|update| ResidentPrimitiveInstance {
                object_id: update.object_id,
                draw: PrimitiveDraw::from_update(*update),
            })
            .collect::<Vec<_>>();
        for instance in &instances {
            self.object_chunks.insert(
                PrimitiveObjectKey {
                    scene_id,
                    layer_id,
                    object_id: instance.object_id,
                },
                chunk_ref,
            );
        }
        let depth_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, false);
        let shadow_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, true);
        self.resident_chunks.insert(
            chunk_ref,
            ResidentPrimitiveChunk {
                instances,
                depth_batches,
                shadow_batches,
            },
        );
    }

    pub fn upsert_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        update: PrimitiveObjectUpdate,
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let object_key = PrimitiveObjectKey {
            scene_id: update.scene_id,
            layer_id: update.layer_id,
            object_id: update.object_id,
        };
        let Some(chunk_ref) = self.object_chunks.get(&object_key).copied() else {
            return;
        };
        if !update.visible {
            return;
        }
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            self.object_chunks.remove(&object_key);
            return;
        };
        if let Some(instance) = chunk
            .instances
            .iter_mut()
            .find(|instance| instance.object_id == update.object_id)
        {
            instance.draw = PrimitiveDraw::from_update(update);
        } else {
            chunk.instances.push(ResidentPrimitiveInstance {
                object_id: update.object_id,
                draw: PrimitiveDraw::from_update(update),
            });
        }
        self.rebuild_resident_chunk(device, queue, chunk_ref, models);
    }

    pub fn destroy_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
        models: &ModelManager,
        _textures: &TextureManager,
    ) {
        let object_key = PrimitiveObjectKey {
            scene_id,
            layer_id,
            object_id,
        };
        let Some(chunk_ref) = self.object_chunks.get(&object_key).copied() else {
            return;
        };
        self.remove_object_from_resident_chunk(device, queue, object_key, chunk_ref, models);
    }

    pub fn clear_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.resident_chunks.retain(|chunk_ref, _| {
            !(chunk_ref.scene_id == scene_id && chunk_ref.layer_id == layer_id)
        });
        self.object_chunks.retain(|object_key, _| {
            !(object_key.scene_id == scene_id && object_key.layer_id == layer_id)
        });
    }

    pub fn clear_scene(&mut self, scene_id: u32) {
        self.resident_chunks
            .retain(|chunk_ref, _| chunk_ref.scene_id != scene_id);
        self.object_chunks
            .retain(|object_key, _| object_key.scene_id != scene_id);
    }

    pub fn draw<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
        filter: PrimitiveRenderFilter,
    ) -> PrimitiveDrawStats {
        let mut stats = PrimitiveDrawStats::default();
        self.last_main_batch_count = 0;
        self.last_main_instance_count = 0;
        self.last_main_triangle_count = 0;
        if draws.is_empty() && chunk_refs.is_empty() {
            return stats;
        }
        let mut batch_draws: Vec<PrimitiveBatchDraw> = Vec::new();
        let mut batches: Vec<PrimitiveBatch> = Vec::new();
        let mut batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
        let mut instance_count = 0u32;
        for draw in draws {
            instance_count += self.push_draw_batches(
                draw,
                models,
                textures,
                &mut batches,
                &mut batch_index,
                filter,
            );
        }
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for instance in &chunk.instances {
                instance_count += self.push_draw_batches(
                    &instance.draw,
                    models,
                    textures,
                    &mut batches,
                    &mut batch_index,
                    filter,
                );
            }
        }
        if instance_count > 0 {
            self.ensure_instance_capacity(device, instance_count);
            let mut instance_data = Vec::with_capacity(instance_count as usize);
            batch_draws = Vec::with_capacity(batches.len());
            sort_primitive_batches(&mut batches);
            for batch in &batches {
                let start = instance_data.len() as u32;
                instance_data.extend_from_slice(&batch.instances);
                batch_draws.push(PrimitiveBatchDraw {
                    key: batch.key,
                    start,
                    count: batch.instances.len() as u32,
                });
            }
            stats.batch_count = batch_draws.len() as u32;
            stats.instance_count = instance_count;
            stats.triangle_count = primitive_batch_triangle_count(&batch_draws, models);
            self.last_main_batch_count = stats.batch_count;
            self.last_main_instance_count = stats.instance_count;
            self.last_main_triangle_count = stats.triangle_count;
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instance_data),
            );
        }
        for batch in &batch_draws {
            self.ensure_texture_bind_group(device, textures, batch.key.textures, shadow_view);
        }

        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.model_bind_group, &[0]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);
        let instance_stride = std::mem::size_of::<PrimitiveInstanceGpu>() as u64;
        for batch in &batch_draws {
            let Some(gpu_model) = models.get(batch.key.model_id) else {
                continue;
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                continue;
            };
            let texture_key = batch.key.textures;
            let texture_bind_group = self
                .texture_bind_groups
                .get(&texture_key)
                .expect("primitive texture bind group cache missing");
            pass.set_pipeline(self.pipeline_for_batch(batch.key, aux_targets_enabled));
            pass.set_bind_group(3, texture_bind_group, &[]);
            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
        }
        stats
    }

    pub fn draw_main_and_water<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
        models: &'a ModelManager,
        textures: &'a TextureManager,
        shadow_view: &'a wgpu::TextureView,
        aux_targets_enabled: bool,
    ) -> PrimitiveLayeredDrawStats {
        self.last_main_batch_count = 0;
        self.last_main_instance_count = 0;
        self.last_main_triangle_count = 0;
        if draws.is_empty() && chunk_refs.is_empty() {
            return PrimitiveLayeredDrawStats::default();
        }

        let mut main_batches: Vec<PrimitiveBatch> = Vec::new();
        let mut main_batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
        let mut water_batches: Vec<PrimitiveBatch> = Vec::new();
        let mut water_batch_index: HashMap<PrimitiveBatchKey, usize> = HashMap::new();
        let mut main_instance_count = 0u32;
        let mut water_instance_count = 0u32;

        for draw in draws {
            main_instance_count += self.push_draw_batches(
                draw,
                models,
                textures,
                &mut main_batches,
                &mut main_batch_index,
                PrimitiveRenderFilter::Main,
            );
            water_instance_count += self.push_draw_batches(
                draw,
                models,
                textures,
                &mut water_batches,
                &mut water_batch_index,
                PrimitiveRenderFilter::Water,
            );
        }
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for instance in &chunk.instances {
                main_instance_count += self.push_draw_batches(
                    &instance.draw,
                    models,
                    textures,
                    &mut main_batches,
                    &mut main_batch_index,
                    PrimitiveRenderFilter::Main,
                );
                water_instance_count += self.push_draw_batches(
                    &instance.draw,
                    models,
                    textures,
                    &mut water_batches,
                    &mut water_batch_index,
                    PrimitiveRenderFilter::Water,
                );
            }
        }

        let total_instance_count = main_instance_count.saturating_add(water_instance_count);
        if total_instance_count == 0 {
            return PrimitiveLayeredDrawStats::default();
        }

        self.ensure_instance_capacity(device, total_instance_count);
        let mut instance_data = Vec::with_capacity(total_instance_count as usize);
        let main_batch_draws = append_primitive_batch_draws(&mut main_batches, &mut instance_data);
        let water_batch_draws =
            append_primitive_batch_draws(&mut water_batches, &mut instance_data);
        let main = PrimitiveDrawStats {
            batch_count: main_batch_draws.len() as u32,
            instance_count: main_instance_count,
            triangle_count: primitive_batch_triangle_count(&main_batch_draws, models),
        };
        let water = PrimitiveDrawStats {
            batch_count: water_batch_draws.len() as u32,
            instance_count: water_instance_count,
            triangle_count: primitive_batch_triangle_count(&water_batch_draws, models),
        };
        self.last_main_batch_count = main.batch_count;
        self.last_main_instance_count = main.instance_count;
        self.last_main_triangle_count = main.triangle_count;
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&instance_data),
        );

        for batch in main_batch_draws.iter().chain(water_batch_draws.iter()) {
            self.ensure_texture_bind_group(device, textures, batch.key.textures, shadow_view);
        }

        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.model_bind_group, &[0]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);

        let main_start = perf_now();
        self.draw_batch_draws(pass, &main_batch_draws, models, aux_targets_enabled);
        let main_cpu_ms = elapsed_ms(main_start);
        let water_start = perf_now();
        self.draw_batch_draws(pass, &water_batch_draws, models, aux_targets_enabled);
        let water_cpu_ms = elapsed_ms(water_start);
        PrimitiveLayeredDrawStats {
            main,
            water,
            main_cpu_ms,
            water_cpu_ms,
        }
    }

    pub fn has_water_surface(
        &self,
        draws: &[PrimitiveDraw],
        chunk_refs: &[PrimitiveChunkRef],
    ) -> bool {
        if draws.iter().any(PrimitiveDraw::is_water_surface) {
            return true;
        }
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            if chunk
                .instances
                .iter()
                .any(|instance| instance.draw.is_water_surface())
            {
                return true;
            }
        }
        false
    }

    fn pipeline_for_batch(
        &self,
        key: PrimitiveBatchKey,
        aux_targets_enabled: bool,
    ) -> &wgpu::RenderPipeline {
        match (key.textures.has_albedo(), key.mode, aux_targets_enabled) {
            (true, PrimitiveRenderMode::Opaque, true) => &self.pipeline_textured_opaque,
            (true, PrimitiveRenderMode::Opaque, false) => &self.pipeline_textured_opaque_color,
            (false, PrimitiveRenderMode::Opaque, true) => &self.pipeline_untextured_opaque,
            (false, PrimitiveRenderMode::Opaque, false) => &self.pipeline_untextured_opaque_color,
            (true, PrimitiveRenderMode::Cutout, true) => &self.pipeline_textured_cutout,
            (true, PrimitiveRenderMode::Cutout, false) => &self.pipeline_textured_cutout_color,
            (false, PrimitiveRenderMode::Cutout, true) => &self.pipeline_untextured_cutout,
            (false, PrimitiveRenderMode::Cutout, false) => &self.pipeline_untextured_cutout_color,
            (true, PrimitiveRenderMode::Translucent, true) => &self.pipeline_textured_translucent,
            (true, PrimitiveRenderMode::Translucent, false) => {
                &self.pipeline_textured_translucent_color
            }
            (false, PrimitiveRenderMode::Translucent, true) => {
                &self.pipeline_untextured_translucent
            }
            (false, PrimitiveRenderMode::Translucent, false) => {
                &self.pipeline_untextured_translucent_color
            }
        }
    }

    fn draw_batch_draws<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        batch_draws: &[PrimitiveBatchDraw],
        models: &'a ModelManager,
        aux_targets_enabled: bool,
    ) {
        let instance_stride = std::mem::size_of::<PrimitiveInstanceGpu>() as u64;
        for batch in batch_draws {
            let Some(gpu_model) = models.get(batch.key.model_id) else {
                continue;
            };
            let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
                continue;
            };
            let texture_key = batch.key.textures;
            let texture_bind_group = self
                .texture_bind_groups
                .get(&texture_key)
                .expect("primitive texture bind group cache missing");
            pass.set_pipeline(self.pipeline_for_batch(batch.key, aux_targets_enabled));
            pass.set_bind_group(3, texture_bind_group, &[]);
            let start = batch.start as u64 * instance_stride;
            let end = start + batch.count as u64 * instance_stride;
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.slice(start..end));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..batch.count);
        }
    }

    fn push_draw_batches(
        &self,
        draw: &PrimitiveDraw,
        models: &ModelManager,
        textures: &TextureManager,
        batches: &mut Vec<PrimitiveBatch>,
        batch_index: &mut HashMap<PrimitiveBatchKey, usize>,
        filter: PrimitiveRenderFilter,
    ) -> u32 {
        if !primitive_draw_matches_filter(draw, filter) {
            return 0;
        }
        let Some(gpu_model) = models.get(draw.model_id) else {
            return 0;
        };
        let mut pushed = 0u32;
        for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
            if mesh.skinned || mesh.material.control_texture_id.is_some() {
                continue;
            }
            let texture_key = resolve_texture_key(&draw.material, &mesh.material, textures);
            let mut uniform = draw.model_uniform;
            uniform.base_color = combined_base_color(&draw.material, &mesh.material.base_color);
            let (material_params, emissive_color, texture_flags) =
                mesh_uniform_values(&draw.material, &mesh.material, texture_key);
            uniform.material_params = material_params;
            uniform.emissive_color = emissive_color;
            uniform.texture_flags = texture_flags;
            let key = PrimitiveBatchKey {
                model_id: draw.model_id,
                mesh_index,
                textures: texture_key,
                mode: primitive_render_mode(draw, uniform.base_color),
            };
            let index = if let Some(index) = batch_index.get(&key) {
                *index
            } else {
                let index = batches.len();
                batches.push(PrimitiveBatch {
                    key,
                    instances: Vec::new(),
                });
                batch_index.insert(key, index);
                index
            };
            batches[index]
                .instances
                .push(PrimitiveInstanceGpu::from_draw(draw, &uniform));
            pushed += 1;
        }
        pushed
    }

    fn remove_object_from_resident_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        object_key: PrimitiveObjectKey,
        chunk_ref: PrimitiveChunkRef,
        models: &ModelManager,
    ) {
        self.object_chunks.remove(&object_key);
        let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) else {
            return;
        };
        chunk
            .instances
            .retain(|instance| instance.object_id != object_key.object_id);
        self.rebuild_resident_chunk(device, queue, chunk_ref, models);
    }

    fn rebuild_resident_chunk(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunk_ref: PrimitiveChunkRef,
        models: &ModelManager,
    ) {
        let Some(instances) = self.resident_chunks.get(&chunk_ref).map(|chunk| {
            chunk
                .instances
                .iter()
                .map(|instance| ResidentPrimitiveInstance {
                    object_id: instance.object_id,
                    draw: instance.draw,
                })
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let depth_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, false);
        let shadow_batches =
            self.build_resident_pass_batches(device, queue, &instances, models, true);
        if instances.is_empty() {
            self.resident_chunks.remove(&chunk_ref);
        } else if let Some(chunk) = self.resident_chunks.get_mut(&chunk_ref) {
            chunk.depth_batches = depth_batches;
            chunk.shadow_batches = shadow_batches;
        }
    }

    fn build_resident_pass_batches(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[ResidentPrimitiveInstance],
        models: &ModelManager,
        shadow_only: bool,
    ) -> Vec<ResidentPrimitivePassBatch> {
        let mut batches: HashMap<PrimitivePassBatchKey, Vec<PrimitivePassInstanceGpu>> =
            HashMap::new();
        for instance in instances {
            if !primitive_participates_in_depth_pass(&instance.draw, shadow_only) {
                continue;
            }
            let Some(gpu_model) = models.get(instance.draw.model_id) else {
                continue;
            };
            for (mesh_index, mesh) in gpu_model.meshes.iter().enumerate() {
                if mesh.skinned {
                    continue;
                }
                let key = PrimitivePassBatchKey {
                    model_id: instance.draw.model_id,
                    mesh_index,
                };
                batches
                    .entry(key)
                    .or_default()
                    .push(PrimitivePassInstanceGpu::from_model(
                        instance.draw.model_uniform.model,
                    ));
            }
        }
        batches
            .into_iter()
            .filter_map(|(key, instances)| {
                if instances.is_empty() {
                    return None;
                }
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("voplay_primitive_chunk_pass_instance_vb"),
                    size: std::mem::size_of::<PrimitivePassInstanceGpu>() as u64
                        * instances.len() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&instances));
                Some(ResidentPrimitivePassBatch {
                    key,
                    buffer,
                    count: instances.len() as u32,
                })
            })
            .collect()
    }

    pub fn for_each_resident_depth_batch<'a, F>(
        &'a self,
        chunk_refs: &[PrimitiveChunkRef],
        mut f: F,
    ) where
        F: FnMut(PrimitivePassBatchKey, &'a wgpu::Buffer, u32),
    {
        let mut visible_batches = Vec::new();
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for batch in &chunk.depth_batches {
                visible_batches.push(ResidentPrimitivePassBatchRef { batch });
            }
        }
        visible_batches.sort_by(compare_resident_primitive_pass_batch_refs);
        for batch_ref in visible_batches {
            let batch = batch_ref.batch;
            f(batch.key, &batch.buffer, batch.count);
        }
    }

    pub fn for_each_resident_shadow_batch<'a, F>(
        &'a self,
        chunk_refs: &[PrimitiveChunkRef],
        mut f: F,
    ) where
        F: FnMut(PrimitivePassBatchKey, &'a wgpu::Buffer, u32),
    {
        let mut visible_batches = Vec::new();
        for chunk_ref in chunk_refs {
            let Some(chunk) = self.resident_chunks.get(chunk_ref) else {
                continue;
            };
            for batch in &chunk.shadow_batches {
                visible_batches.push(ResidentPrimitivePassBatchRef { batch });
            }
        }
        visible_batches.sort_by(compare_resident_primitive_pass_batch_refs);
        for batch_ref in visible_batches {
            let batch = batch_ref.batch;
            f(batch.key, &batch.buffer, batch.count);
        }
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_buffer_capacity {
            return;
        }
        let new_count = needed.next_power_of_two().max(1024);
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_primitive_instance_vb"),
            size: std::mem::size_of::<PrimitiveInstanceGpu>() as u64 * new_count as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer_capacity = new_count;
    }

    fn create_texture_bind_group(
        &self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: PrimitiveTextureKey,
        shadow_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let albedo_view = self.texture_view(textures, key.albedo);
        let normal_view = self.texture_view(textures, key.normal);
        let metallic_roughness_view = self.texture_view(textures, key.metallic_roughness);
        let emissive_view = self.texture_view(textures, key.emissive);
        let toon_ramp_view = self.texture_view(textures, key.toon_ramp);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_primitive_texture_bg"),
            layout: &self.main_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(
                        &self.material_samplers[key.sampler.sampler_index()],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(metallic_roughness_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(emissive_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(toon_ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self.white_texture_view),
                },
            ],
        })
    }

    fn texture_view<'a>(
        &'a self,
        textures: &'a TextureManager,
        texture_id: u32,
    ) -> &'a wgpu::TextureView {
        textures
            .get(texture_id)
            .map(|texture| &texture.view)
            .unwrap_or(&self.white_texture_view)
    }

    fn ensure_texture_bind_group(
        &mut self,
        device: &wgpu::Device,
        textures: &TextureManager,
        key: PrimitiveTextureKey,
        shadow_view: &wgpu::TextureView,
    ) {
        if self.texture_bind_groups.contains_key(&key) {
            return;
        }
        let bind_group = self.create_texture_bind_group(device, textures, key, shadow_view);
        self.texture_bind_groups.insert(key, bind_group);
    }
}

fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}

fn create_material_sampler(device: &wgpu::Device, key: MaterialSamplerKey) -> wgpu::Sampler {
    let address_mode = match key.wrap_mode {
        crate::material::MATERIAL_WRAP_CLAMP => wgpu::AddressMode::ClampToEdge,
        crate::material::MATERIAL_WRAP_MIRROR => wgpu::AddressMode::MirrorRepeat,
        _ => wgpu::AddressMode::Repeat,
    };
    let filter = match key.filter_mode {
        crate::material::MATERIAL_FILTER_NEAREST => wgpu::FilterMode::Nearest,
        _ => wgpu::FilterMode::Linear,
    };
    let anisotropy_clamp = if filter == wgpu::FilterMode::Linear {
        8
    } else {
        1
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("voplay_primitive_material_sampler"),
        address_mode_u: address_mode,
        address_mode_v: address_mode,
        address_mode_w: address_mode,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        anisotropy_clamp,
        ..Default::default()
    })
}

fn valid_texture_id(textures: &TextureManager, texture_id: Option<u32>) -> u32 {
    texture_id
        .filter(|id| *id != 0 && textures.get(*id).is_some())
        .unwrap_or(0)
}

fn resolve_texture_key(
    material: &MaterialOverride,
    mesh_material: &MeshMaterial,
    textures: &TextureManager,
) -> PrimitiveTextureKey {
    let albedo =
        if material.albedo_texture_id != 0 && textures.get(material.albedo_texture_id).is_some() {
            material.albedo_texture_id
        } else {
            valid_texture_id(textures, mesh_material.texture_id)
        };
    let normal =
        if material.normal_texture_id != 0 && textures.get(material.normal_texture_id).is_some() {
            material.normal_texture_id
        } else {
            valid_texture_id(textures, mesh_material.normal_texture_id)
        };
    let metallic_roughness = if material.metallic_roughness_texture_id != 0
        && textures
            .get(material.metallic_roughness_texture_id)
            .is_some()
    {
        material.metallic_roughness_texture_id
    } else {
        valid_texture_id(textures, mesh_material.metallic_roughness_texture_id)
    };
    let emissive = if material.emissive_texture_id != 0
        && textures.get(material.emissive_texture_id).is_some()
    {
        material.emissive_texture_id
    } else {
        valid_texture_id(textures, mesh_material.emissive_texture_id)
    };
    let toon_ramp = if material.toon_ramp_texture_id != 0
        && textures.get(material.toon_ramp_texture_id).is_some()
    {
        material.toon_ramp_texture_id
    } else {
        valid_texture_id(textures, mesh_material.toon_ramp_texture_id)
    };
    PrimitiveTextureKey {
        albedo,
        normal,
        metallic_roughness,
        emissive,
        toon_ramp,
        sampler: MaterialSamplerKey::resolve(
            material.wrap_mode,
            material.filter_mode,
            mesh_material.sampler,
        ),
    }
}

fn combined_base_color(material: &MaterialOverride, mesh_color: &[f32; 4]) -> [f32; 4] {
    let color = material.base_color_multiplier();
    [
        mesh_color[0] * color[0],
        mesh_color[1] * color[1],
        mesh_color[2] * color[2],
        mesh_color[3] * color[3],
    ]
}

fn mesh_uniform_values(
    material: &MaterialOverride,
    mesh_material: &MeshMaterial,
    key: PrimitiveTextureKey,
) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let override_emissive_texture_active =
        material.emissive_texture_id != 0 && key.emissive == material.emissive_texture_id;
    (
        [
            material.uv_scale_or(mesh_material.uv_scales[0]),
            roughness_value(material, mesh_material.roughness),
            metallic_value(material, mesh_material.metallic),
            if material.shading_mode == 0 {
                0.0
            } else {
                material.shading_mode as f32
            },
        ],
        emissive_color_value(
            material,
            mesh_material.emissive_factor,
            override_emissive_texture_active,
        ),
        key.texture_flags(normal_scale_value(material, mesh_material.normal_scale)),
    )
}

fn roughness_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.roughness > 0.0 {
        material.roughness.clamp(0.04, 1.0)
    } else {
        fallback.clamp(0.04, 1.0)
    }
}

fn metallic_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.metallic != 0.0 {
        material.metallic.clamp(0.0, 1.0)
    } else {
        fallback.clamp(0.0, 1.0)
    }
}

fn normal_scale_value(material: &MaterialOverride, fallback: f32) -> f32 {
    if material.normal_scale > 0.0 {
        material.normal_scale
    } else if material.normal_texture_id != 0 {
        1.0
    } else {
        fallback.max(0.0)
    }
}

fn primitive_participates_in_depth_pass(draw: &PrimitiveDraw, shadow_only: bool) -> bool {
    let flags = primitive_draw_flags(draw);
    if (flags & PRIMITIVE_FLAG_WATER_SURFACE) != 0 {
        return false;
    }
    if (flags & (PRIMITIVE_FLAG_BILLBOARD | PRIMITIVE_FLAG_Y_BILLBOARD | PRIMITIVE_FLAG_ATLAS_UV))
        != 0
    {
        return false;
    }
    if shadow_only && (flags & PRIMITIVE_FLAG_NO_SHADOW) != 0 {
        return false;
    }
    true
}

fn primitive_render_mode(draw: &PrimitiveDraw, base_color: [f32; 4]) -> PrimitiveRenderMode {
    let flags = primitive_draw_flags(draw);
    if (flags & (PRIMITIVE_FLAG_BILLBOARD | PRIMITIVE_FLAG_Y_BILLBOARD | PRIMITIVE_FLAG_ATLAS_UV))
        != 0
    {
        return PrimitiveRenderMode::Cutout;
    }
    if base_color[3] < 0.999 {
        return PrimitiveRenderMode::Translucent;
    }
    PrimitiveRenderMode::Opaque
}

fn primitive_draw_matches_filter(draw: &PrimitiveDraw, filter: PrimitiveRenderFilter) -> bool {
    match filter {
        PrimitiveRenderFilter::Main => !draw.is_water_surface(),
        PrimitiveRenderFilter::Water => draw.is_water_surface(),
    }
}

fn sort_primitive_batches(batches: &mut [PrimitiveBatch]) {
    batches.sort_by(|a, b| compare_primitive_batch_key(a.key, b.key));
}

fn append_primitive_batch_draws(
    batches: &mut [PrimitiveBatch],
    instance_data: &mut Vec<PrimitiveInstanceGpu>,
) -> Vec<PrimitiveBatchDraw> {
    if batches.is_empty() {
        return Vec::new();
    }
    sort_primitive_batches(batches);
    let mut batch_draws = Vec::with_capacity(batches.len());
    for batch in batches {
        let start = instance_data.len() as u32;
        instance_data.extend_from_slice(&batch.instances);
        batch_draws.push(PrimitiveBatchDraw {
            key: batch.key,
            start,
            count: batch.instances.len() as u32,
        });
    }
    batch_draws
}

fn primitive_batch_triangle_count(batches: &[PrimitiveBatchDraw], models: &ModelManager) -> u32 {
    let mut triangles = 0u32;
    for batch in batches {
        let Some(gpu_model) = models.get(batch.key.model_id) else {
            continue;
        };
        let Some(mesh) = gpu_model.meshes.get(batch.key.mesh_index) else {
            continue;
        };
        triangles = triangles.saturating_add((mesh.index_count / 3).saturating_mul(batch.count));
    }
    triangles
}

fn compare_resident_primitive_pass_batch_refs(
    a: &ResidentPrimitivePassBatchRef<'_>,
    b: &ResidentPrimitivePassBatchRef<'_>,
) -> Ordering {
    compare_primitive_pass_batch_key(a.batch.key, b.batch.key)
}

fn compare_primitive_batch_key(a: PrimitiveBatchKey, b: PrimitiveBatchKey) -> Ordering {
    (
        primitive_render_mode_order(a.mode),
        a.model_id,
        a.mesh_index,
        primitive_texture_sort_key(a.textures),
    )
        .cmp(&(
            primitive_render_mode_order(b.mode),
            b.model_id,
            b.mesh_index,
            primitive_texture_sort_key(b.textures),
        ))
}

fn compare_primitive_pass_batch_key(
    a: PrimitivePassBatchKey,
    b: PrimitivePassBatchKey,
) -> Ordering {
    (a.model_id, a.mesh_index).cmp(&(b.model_id, b.mesh_index))
}

fn primitive_texture_sort_key(key: PrimitiveTextureKey) -> (u32, u32, u32, u32, u32, usize) {
    (
        key.albedo,
        key.normal,
        key.metallic_roughness,
        key.emissive,
        key.toon_ramp,
        key.sampler.sampler_index(),
    )
}

fn primitive_render_mode_order(mode: PrimitiveRenderMode) -> u8 {
    match mode {
        PrimitiveRenderMode::Opaque => 0,
        PrimitiveRenderMode::Cutout => 1,
        PrimitiveRenderMode::Translucent => 2,
    }
}

fn primitive_draw_flags(draw: &PrimitiveDraw) -> u32 {
    draw.flags()
}

fn emissive_color_value(
    material: &MaterialOverride,
    source_factor: [f32; 3],
    override_texture_active: bool,
) -> [f32; 4] {
    let e = material.emissive_color;
    if e[0] == 0.0 && e[1] == 0.0 && e[2] == 0.0 && e[3] == 0.0 {
        if source_factor != [0.0, 0.0, 0.0] {
            [source_factor[0], source_factor[1], source_factor[2], 1.0]
        } else if override_texture_active {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::{
        primitive_render_mode, sort_primitive_batches, PrimitiveBatch, PrimitiveBatchKey,
        PrimitivePipeline, PrimitiveRenderMode, PrimitiveTextureKey,
    };
    use crate::material::MaterialSamplerKey;
    use crate::math3d::{Quat, Vec3};
    use crate::model_loader::ModelManager;
    use crate::pipeline3d::{MaterialOverride, ModelUniform};
    use crate::primitive_scene::{
        PrimitiveDraw, PrimitiveObjectUpdate, PRIMITIVE_FLAG_ATLAS_UV, PRIMITIVE_FLAG_BILLBOARD,
    };
    use crate::texture::TextureManager;
    use bytemuck::Zeroable;

    #[test]
    fn primitive_pipeline_creates_with_current_shader_layouts() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            return;
        };
        let adapter_limits = adapter.limits();
        if adapter_limits.max_inter_stage_shader_components < 44 {
            return;
        }
        let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
        limits.max_inter_stage_shader_components = adapter_limits
            .max_inter_stage_shader_components
            .min(wgpu::Limits::default().max_inter_stage_shader_components);
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voplay_primitive_pipeline_test"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let _pipeline = PrimitivePipeline::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        );
    }

    #[test]
    fn primitive_pipeline_tracks_resident_chunks() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            return;
        };
        let adapter_limits = adapter.limits();
        if adapter_limits.max_inter_stage_shader_components < 44 {
            return;
        }
        let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
        limits.max_inter_stage_shader_components = adapter_limits
            .max_inter_stage_shader_components
            .min(wgpu::Limits::default().max_inter_stage_shader_components);
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("voplay_primitive_resident_chunk_test"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");
        let mut pipeline = PrimitivePipeline::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        );
        let models = ModelManager::new();
        let textures = TextureManager::new(&device);
        let update = PrimitiveObjectUpdate {
            scene_id: 1,
            layer_id: 2,
            object_id: 3,
            model_id: 4,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
        };
        pipeline.replace_chunk(&device, &queue, 1, 2, 5, &[update], &models, &textures);
        assert_eq!(pipeline.resident_chunks.len(), 1);
        assert_eq!(pipeline.object_chunks.len(), 1);

        pipeline.destroy_instance(&device, &queue, 1, 2, 3, &models, &textures);
        assert!(pipeline.resident_chunks.is_empty());
        assert!(pipeline.object_chunks.is_empty());
    }

    #[test]
    fn primitive_pipeline_classifies_render_modes() {
        let mut draw = PrimitiveDraw {
            model_id: 1,
            model_uniform: ModelUniform::zeroed(),
            material: MaterialOverride::default(),
            instance_params: [0.0; 4],
            instance_params2: [0.0, 0.0, 1.0, 1.0],
        };
        assert_eq!(
            primitive_render_mode(&draw, [1.0, 1.0, 1.0, 1.0]),
            PrimitiveRenderMode::Opaque
        );
        assert_eq!(
            primitive_render_mode(&draw, [1.0, 1.0, 1.0, 0.5]),
            PrimitiveRenderMode::Translucent
        );
        draw.instance_params[0] = PRIMITIVE_FLAG_ATLAS_UV as f32;
        assert_eq!(
            primitive_render_mode(&draw, [1.0, 1.0, 1.0, 1.0]),
            PrimitiveRenderMode::Cutout
        );
        draw.instance_params[0] = PRIMITIVE_FLAG_BILLBOARD as f32;
        assert_eq!(
            primitive_render_mode(&draw, [1.0, 1.0, 1.0, 0.5]),
            PrimitiveRenderMode::Cutout
        );
    }

    #[test]
    fn primitive_pipeline_sorts_batches_by_state_then_model() {
        let texture = PrimitiveTextureKey {
            albedo: 0,
            normal: 0,
            metallic_roughness: 0,
            emissive: 0,
            toon_ramp: 0,
            sampler: MaterialSamplerKey::REPEAT_LINEAR,
        };
        let mut batches = vec![
            PrimitiveBatch {
                key: PrimitiveBatchKey {
                    model_id: 8,
                    mesh_index: 0,
                    textures: texture,
                    mode: PrimitiveRenderMode::Translucent,
                },
                instances: Vec::new(),
            },
            PrimitiveBatch {
                key: PrimitiveBatchKey {
                    model_id: 4,
                    mesh_index: 0,
                    textures: texture,
                    mode: PrimitiveRenderMode::Cutout,
                },
                instances: Vec::new(),
            },
            PrimitiveBatch {
                key: PrimitiveBatchKey {
                    model_id: 2,
                    mesh_index: 0,
                    textures: texture,
                    mode: PrimitiveRenderMode::Opaque,
                },
                instances: Vec::new(),
            },
        ];
        sort_primitive_batches(&mut batches);
        assert_eq!(batches[0].key.mode, PrimitiveRenderMode::Opaque);
        assert_eq!(batches[0].key.model_id, 2);
        assert_eq!(batches[1].key.mode, PrimitiveRenderMode::Cutout);
        assert_eq!(batches[1].key.model_id, 4);
        assert_eq!(batches[2].key.mode, PrimitiveRenderMode::Translucent);
        assert_eq!(batches[2].key.model_id, 8);
    }
}
