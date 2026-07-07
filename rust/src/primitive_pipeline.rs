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

mod runtime;

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
    Translucent,
    Water,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimitiveDrawStats {
    pub batch_count: u32,
    pub instance_count: u32,
    pub triangle_count: u32,
}

#[allow(dead_code)]
// owner: voplay/render; expiry: 2026-07-12; retained for combined main/water submitter regression coverage.
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

#[derive(Clone, Copy)]
struct PrimitiveChunkDirtyRange {
    chunk_ref: PrimitiveChunkRef,
    dirty_start: u32,
    dirty_count: u32,
    requires_full_rebuild: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResidentRebuildPolicy {
    pub(crate) dirty_upload_bytes: u64,
    pub(crate) full_rebuild_count: u32,
    pub(crate) rebuild_reason: &'static str,
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
    rebuild_queue: Vec<PrimitiveChunkDirtyRange>,
    staging_instances: Vec<ResidentPrimitiveInstance>,
    rebuild_queue_peak: u32,
    last_resident_chunk_rebuilds: u32,
    last_resident_rebuild_policy: ResidentRebuildPolicy,
    texture_bind_groups: HashMap<PrimitiveTextureKey, wgpu::BindGroup>,
    last_main_batch_count: u32,
    last_main_instance_count: u32,
    last_main_triangle_count: u32,
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
        PrimitiveRenderFilter::Translucent => !draw.is_water_surface(),
        PrimitiveRenderFilter::Water => draw.is_water_surface(),
    }
}

fn primitive_mode_matches_filter(mode: PrimitiveRenderMode, filter: PrimitiveRenderFilter) -> bool {
    match filter {
        PrimitiveRenderFilter::Main => mode != PrimitiveRenderMode::Translucent,
        PrimitiveRenderFilter::Translucent => mode == PrimitiveRenderMode::Translucent,
        PrimitiveRenderFilter::Water => true,
    }
}

fn sort_primitive_batches(batches: &mut [PrimitiveBatch]) {
    batches.sort_by(|a, b| compare_primitive_batch_key(a.key, b.key));
}

#[allow(dead_code)] // owner: voplay/render; expiry: 2026-07-12; helper for retained combined submitter path.
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
mod tests;
