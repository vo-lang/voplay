//! 3D mesh rendering pipeline — Blinn-Phong forward renderer.
//!
//! Renders loaded glTF models with per-node transforms, directional/point lights,
//! and optional albedo textures. Uses depth testing for proper occlusion.

use crate::animation;
use crate::material::{MaterialSamplerKey, MATERIAL_SAMPLER_KEYS};
use crate::model_loader::{
    MeshMaterial, MeshVertex, ModelId, ModelManager, SkinnedMeshVertex, TerrainMaterialTuning,
};
use crate::pipeline3d_batches::{
    InstanceBatch, InstanceBatchDraw, InstanceBatchKey, InstanceData, MainTextureKey,
    TerrainBindGroupEntry, TerrainTextureKey,
};
pub use crate::pipeline3d_material::MaterialOverride;
use crate::texture::TextureManager;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

mod decal_submitter;
mod mesh_submitter;
pub(crate) use mesh_submitter::MeshDrawStats;
mod pipeline_cache;
mod pipeline_factory;

pub(crate) use decal_submitter::{DecalSubmitPlan, DecalSubmitter};

const STATIC_MESH_SHADER: &str = include_str!("shaders/mesh.wgsl");
const TERRAIN_MESH_SHADER: &str = include_str!("shaders/mesh_terrain.wgsl");
const SKINNED_MESH_SHADER: &str = include_str!("shaders/mesh_skinned.wgsl");

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
    pub emissive_color: [f32; 4],
    pub texture_flags: [f32; 4],
    pub material_response: [f32; 4],
    pub texture_flags2: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct TerrainMaterialUniform {
    params0: [f32; 4],
    params1: [f32; 4],
    params2: [f32; 4],
    params3: [f32; 4],
}

impl TerrainMaterialUniform {
    fn from_tuning(tuning: TerrainMaterialTuning) -> Self {
        let tuning = tuning.normalized_or_default();
        Self {
            params0: [
                tuning.macro_scale,
                tuning.macro_strength,
                tuning.detail_near,
                tuning.detail_far,
            ],
            params1: [
                tuning.slope_start,
                tuning.slope_end,
                tuning.slope_dirt_strength,
                tuning.slope_rock_strength,
            ],
            params2: [
                tuning.anti_tile_strength,
                tuning.detail_strength,
                tuning.normal_near,
                tuning.normal_far,
            ],
            params3: [
                tuning.height_blend_strength,
                tuning.height_low,
                tuning.height_high,
                tuning.curvature_strength,
            ],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct SkinnedModelUniform {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub material_params: [f32; 4],
    pub emissive_color: [f32; 4],
    pub texture_flags: [f32; 4],
    pub material_response: [f32; 4],
    pub texture_flags2: [f32; 4],
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
    pub ambient_ground: [f32; 4],
    pub count: [u32; 4], // x = number of lights, y = fog mode
    pub lights: [LightData; MAX_LIGHTS],
    pub fog_color: [f32; 4],
    pub fog_params: [f32; 4],
    pub shadow_vp: [[f32; 4]; 4],
    pub shadow_cascade_vp: [[[f32; 4]; 4]; 4],
    pub shadow_cascade_splits: [f32; 4],
    pub shadow_params: [f32; 4],
    pub shadow_params2: [f32; 4],
    pub color_params: [f32; 4],
    pub debug_params: [u32; 4],
}

/// A pending 3D model draw.
#[derive(Clone, Copy)]
pub struct ModelDraw {
    pub model_id: ModelId,
    pub model_uniform: ModelUniform,
    pub material: MaterialOverride,
    pub animation_world_id: u32,
    pub animation_target_id: u32,
}

#[cfg(test)]
mod tests {
    use crate::material::{
        MaterialSamplerKey, MATERIAL_FILTER_LINEAR, MATERIAL_FILTER_NEAREST, MATERIAL_WRAP_CLAMP,
        MATERIAL_WRAP_MIRROR, MATERIAL_WRAP_REPEAT,
    };
    use crate::model_loader::MeshMaterial;

    use super::{MaterialOverride, Pipeline3D};

    #[test]
    fn material_override_packs_render_material_params() {
        let material = MaterialOverride {
            roughness: 1.7,
            metallic: -0.5,
            normal_texture_id: 12,
            normal_scale: 0.35,
            uv_scale: 2.0,
            detail_strength: 1.4,
            macro_blend: 0.6,
            roughness_response: 0.75,
            toon_ramp_response: 0.5,
            shading_mode: 1,
            emissive_color: [0.2, 0.3, 0.4, 1.5],
            ..MaterialOverride::default()
        };
        assert_eq!(
            material.mesh_material_params(0.25, 0.65, 0.8),
            [2.0, 1.0, 0.0, 1.0]
        );
        assert_eq!(material.normal_scale_value(0.8), 0.35);
        assert_eq!(
            material.material_response_values(&MeshMaterial::standard([1.0; 4], None, 1.0)),
            [1.4, 0.6, 0.75, 0.5]
        );
        assert_eq!(
            material.emissive_color_value([0.0, 0.0, 0.0], false),
            [0.2, 0.3, 0.4, 1.5]
        );

        let fallback = MaterialOverride {
            roughness: 0.0,
            uv_scale: 0.0,
            ..MaterialOverride::default()
        };
        assert_eq!(
            fallback.mesh_material_params(0.75, 0.7, 0.25),
            [0.75, 0.7, 0.25, 0.0]
        );
        assert_eq!(fallback.normal_scale_value(0.8), 0.8);
        assert_eq!(
            fallback.material_response_values(&MeshMaterial::standard([1.0; 4], None, 1.0)),
            [1.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            fallback.emissive_color_value([0.4, 0.5, 0.6], false),
            [0.4, 0.5, 0.6, 1.0]
        );
        assert_eq!(
            fallback.emissive_color_value([0.0, 0.0, 0.0], true),
            [1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn material_override_resolves_sampler_config() {
        let source = MaterialSamplerKey {
            wrap_mode: MATERIAL_WRAP_CLAMP,
            filter_mode: MATERIAL_FILTER_NEAREST,
        };
        assert_eq!(MaterialOverride::default().sampler_key(source), source);

        let override_sampler = MaterialOverride {
            wrap_mode: MATERIAL_WRAP_MIRROR,
            filter_mode: MATERIAL_FILTER_LINEAR,
            ..MaterialOverride::default()
        };
        assert_eq!(
            override_sampler.sampler_key(source),
            MaterialSamplerKey {
                wrap_mode: MATERIAL_WRAP_MIRROR,
                filter_mode: MATERIAL_FILTER_LINEAR,
            }
        );

        let invalid_sampler = MaterialOverride {
            wrap_mode: 99,
            filter_mode: 99,
            ..MaterialOverride::default()
        };
        assert_eq!(invalid_sampler.sampler_key(source), source);
        assert_eq!(MaterialSamplerKey::REPEAT_LINEAR.sampler_index(), 0);
        assert_eq!(
            MaterialSamplerKey {
                wrap_mode: MATERIAL_WRAP_REPEAT,
                filter_mode: MATERIAL_FILTER_NEAREST,
            }
            .sampler_index(),
            3
        );
    }

    #[test]
    fn pipeline3d_creates_with_current_shader_layouts() {
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
                label: Some("voplay_pipeline3d_test"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let _pipeline = Pipeline3D::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
        );
    }
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

/// The 3D mesh rendering pipeline.
pub struct Pipeline3D {
    #[allow(dead_code)]
    // owner: voplay/render; expiry: 2026-07-12; legacy direct pipelines retained during cache split.
    pipeline_textured: wgpu::RenderPipeline,
    #[allow(dead_code)]
    // owner: voplay/render; expiry: 2026-07-12; legacy direct pipelines retained during cache split.
    pipeline_untextured: wgpu::RenderPipeline,
    pipeline_instanced_textured: wgpu::RenderPipeline,
    pipeline_instanced_textured_color: wgpu::RenderPipeline,
    pipeline_instanced_untextured: wgpu::RenderPipeline,
    pipeline_instanced_untextured_color: wgpu::RenderPipeline,
    pipeline_terrain_splat: wgpu::RenderPipeline,
    pipeline_terrain_splat_color: wgpu::RenderPipeline,
    pipeline_skinned_textured: wgpu::RenderPipeline,
    pipeline_skinned_textured_color: wgpu::RenderPipeline,
    pipeline_skinned_untextured: wgpu::RenderPipeline,
    pipeline_skinned_untextured_color: wgpu::RenderPipeline,
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
    material_samplers: Vec<wgpu::Sampler>,
    material_clamp_sampler: wgpu::Sampler,
    // 1x1 white fallback texture for untextured meshes
    white_texture_view: wgpu::TextureView,
    main_texture_bind_groups: HashMap<MainTextureKey, wgpu::BindGroup>,
    terrain_texture_bind_groups: HashMap<TerrainTextureKey, TerrainBindGroupEntry>,
}
