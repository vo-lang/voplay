use super::backend_submit_pass::{BackendSubmitPassContext, BackendSubmitPassExecutor};
use super::depth_pass::{DepthPassContext, DepthPassExecutor};
use super::main_opaque_pass::{MainOpaquePassContext, MainOpaquePassExecutor};
use super::main_transparent_pass::{MainTransparentPassContext, MainTransparentPassExecutor};
use super::overlay_pass::{OverlayPassContext, OverlayPassExecutor};
use super::post_pass::{PostPassContext, PostPassExecutor, PostPassSetup, PostPassSetupContext};
use super::shadow_pass::{ShadowPassContext, ShadowPassExecutor};
use super::water_pass::{WaterPassContext, WaterPassExecutor};
use super::*;
use crate::draw_list::Frame2D;
use crate::renderer_frame::RenderPassNodeDispatcher;

pub(super) struct RenderGpuScope<'a> {
    pub(super) gpu_device: &'a wgpu::Device,
    pub(super) gpu_queue: &'a wgpu::Queue,
    pub(super) surface: &'a wgpu::SurfaceConfiguration,
}

pub(super) struct RenderPostBindings<'a> {
    pub(super) uniform_buffer: &'a wgpu::Buffer,
    pub(super) decal_uniform_buffer: &'a wgpu::Buffer,
    pub(super) bind_group: &'a Option<wgpu::BindGroup>,
}

pub(super) struct RenderPipelineScope<'a> {
    pub(super) two_d: &'a mut Pipeline2D,
    pub(super) sprite: &'a mut PipelineSprite,
    pub(super) mesh3d: &'a mut Pipeline3D,
    pub(super) primitive: &'a mut PrimitivePipeline,
    pub(super) depth: &'a mut PipelineDepth,
    pub(super) shadow: &'a mut PipelineShadow,
    pub(super) skybox: &'a mut PipelineSkybox,
    pub(super) post: &'a PipelinePost,
}

pub(super) struct RenderAssetScope<'a> {
    pub(super) models: &'a ModelManager,
    pub(super) textures: &'a TextureManager,
    pub(super) world: &'a RenderWorld,
}

pub(super) struct RenderPassResources<'a> {
    pub(super) gpu: RenderGpuScope<'a>,
    pub(super) target_registry: &'a RenderResourceRegistry,
    pub(super) post_bindings: RenderPostBindings<'a>,
    pub(super) camera_bind_group: &'a wgpu::BindGroup,
    pub(super) pipelines: RenderPipelineScope<'a>,
    pub(super) assets: RenderAssetScope<'a>,
}

impl RenderPassResources<'_> {
    pub(super) fn clear_texture_bind_group_caches(&mut self) {
        self.pipelines.mesh3d.clear_texture_bind_group_cache();
        self.pipelines.primitive.clear_texture_bind_group_cache();
    }
}

pub(super) struct FramePassDispatcher<'a> {
    pub(super) resources: RenderPassResources<'a>,
    pub(super) encoder: Option<wgpu::CommandEncoder>,
    pub(super) output: Option<wgpu::SurfaceTexture>,
    pub(super) surface_view: &'a wgpu::TextureView,
    pub(super) frame: &'a Frame2D,
    pub(super) camera_alignment: u32,
    pub(super) clear_color: wgpu::Color,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)>,
    pub(super) skybox_cubemap_id: Option<u32>,
    pub(super) light_uniform: &'a mut LightUniform,
    pub(super) planned_model_draws: &'a [ModelDraw],
    pub(super) primitive_depth_draws: &'a mut Vec<PrimitiveDraw>,
    pub(super) primitive_depth_chunks: &'a [PrimitiveChunkRef],
    pub(super) primitive_shadow_draws: &'a mut Vec<PrimitiveDraw>,
    pub(super) primitive_shadow_chunks: &'a [PrimitiveChunkRef],
    pub(super) retained_scene_draws: &'a [u32],
    pub(super) planned_primitive_draws: &'a [PrimitiveDraw],
    pub(super) planned_primitive_chunks: &'a [PrimitiveChunkRef],
    pub(super) planned_water_draws: &'a [PrimitiveDraw],
    pub(super) planned_water_chunks: &'a [PrimitiveChunkRef],
    pub(super) projected_decal_atlas_bindings: &'a [ProjectedDecalAtlasBinding],
    pub(super) projected_decals: &'a [PostDecalGpu],
    pub(super) projected_decal_atlas_binding_count: u32,
    pub(super) shadow_resolution: u32,
    pub(super) shadow_quality: u32,
    pub(super) shadow_distance: f32,
    pub(super) shadow_fade: f32,
    pub(super) shadow_softness: f32,
    pub(super) shadow_strength: f32,
    pub(super) main_aux_targets_enabled: bool,
    pub(super) aspect: f32,
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
    pub(super) primitive_depth_draw_calls: &'a mut u32,
    pub(super) primitive_shadow_draw_calls: &'a mut u32,
    pub(super) primitive_main_stats: &'a mut PrimitiveDrawStats,
    pub(super) primitive_transparent_stats: &'a mut PrimitiveDrawStats,
    pub(super) primitive_main_submitted: &'a mut bool,
    pub(super) primitive_water_stats: &'a mut PrimitiveDrawStats,
    pub(super) shadow_active: &'a mut bool,
    pub(super) post_uniforms_uploaded: bool,
    pub(super) bloom_threshold: f32,
    pub(super) bloom_strength: f32,
    pub(super) sharpen_strength: f32,
    pub(super) fxaa_strength: f32,
    pub(super) contact_ao_strength: f32,
    pub(super) contact_ao_radius: f32,
    pub(super) contact_ao_depth_scale: f32,
    pub(super) contact_ao_detail_strength: f32,
    pub(super) contact_ao_detail_radius: f32,
    pub(super) contact_ao_normal_bias: f32,
    pub(super) contact_ao_quality: u32,
}

impl FramePassDispatcher<'_> {
    fn ensure_post_uniforms_uploaded(&mut self) {
        if self.post_uniforms_uploaded {
            return;
        }
        if !*self.shadow_active {
            self.light_uniform.shadow_vp = math3d::MAT4_IDENTITY;
            self.light_uniform.shadow_cascade_vp = [math3d::MAT4_IDENTITY; 4];
            self.light_uniform.shadow_cascade_splits = [0.0; 4];
            self.light_uniform.shadow_params =
                [0.0, 0.002, self.shadow_softness, self.shadow_strength];
            self.light_uniform.shadow_params2 = [
                self.shadow_distance,
                self.shadow_fade,
                self.shadow_quality as f32,
                0.0,
            ];
        }
        let mut post_setup = PostPassSetupContext {
            resources: &mut self.resources,
            camera3d_uniform: self.camera3d_uniform,
            camera3d_state: self.camera3d_state,
            light_uniform: &mut *self.light_uniform,
            projected_decals: self.projected_decals,
            projected_decal_atlas_binding_count: self.projected_decal_atlas_binding_count,
            bloom_threshold: self.bloom_threshold,
            bloom_strength: self.bloom_strength,
            sharpen_strength: self.sharpen_strength,
            fxaa_strength: self.fxaa_strength,
            contact_ao_strength: self.contact_ao_strength,
            contact_ao_radius: self.contact_ao_radius,
            contact_ao_depth_scale: self.contact_ao_depth_scale,
            contact_ao_detail_strength: self.contact_ao_detail_strength,
            contact_ao_detail_radius: self.contact_ao_detail_radius,
            contact_ao_normal_bias: self.contact_ao_normal_bias,
            contact_ao_quality: self.contact_ao_quality,
        };
        PostPassSetup::upload_uniforms(&mut post_setup);
        self.post_uniforms_uploaded = true;
    }
}

impl RenderPassNodeDispatcher for FramePassDispatcher<'_> {
    fn before_execute(&mut self, kind: RenderPassKind) -> Result<(), String> {
        if matches!(kind, RenderPassKind::MainOpaque) {
            self.ensure_post_uniforms_uploaded();
        }
        Ok(())
    }

    fn execute(&mut self, kind: RenderPassKind) -> Result<f64, String> {
        match kind {
            RenderPassKind::DepthPrepass => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.camera3d_uniform;
                let model_draws = self.planned_model_draws;
                let primitive_depth_chunks = self.primitive_depth_chunks;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = DepthPassContext {
                    resources: &mut self.resources,
                    encoder,
                    camera3d_uniform,
                    model_draws,
                    primitive_depth_draws: &mut *self.primitive_depth_draws,
                    primitive_depth_chunks,
                    perf_enabled,
                };
                let result = DepthPassExecutor::execute(&mut context)?;
                *self.primitive_depth_draw_calls = result.primitive_draw_calls;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Shadow => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.camera3d_uniform;
                let camera3d_state = self.camera3d_state;
                let model_draws = self.planned_model_draws;
                let primitive_shadow_chunks = self.primitive_shadow_chunks;
                let retained_scene_draws = self.retained_scene_draws;
                let shadow_resolution = self.shadow_resolution;
                let shadow_quality = self.shadow_quality;
                let shadow_distance = self.shadow_distance;
                let shadow_fade = self.shadow_fade;
                let shadow_softness = self.shadow_softness;
                let shadow_strength = self.shadow_strength;
                let aspect = self.aspect;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = ShadowPassContext {
                    resources: &mut self.resources,
                    encoder,
                    camera3d_uniform,
                    camera3d_state,
                    light_uniform: &mut *self.light_uniform,
                    model_draws,
                    primitive_shadow_draws: &mut *self.primitive_shadow_draws,
                    primitive_shadow_chunks,
                    retained_scene_draws,
                    shadow_resolution,
                    shadow_quality,
                    shadow_distance,
                    shadow_fade,
                    shadow_softness,
                    shadow_strength,
                    aspect,
                    perf_enabled,
                };
                let result = ShadowPassExecutor::execute(&mut context)?;
                *self.primitive_shadow_draw_calls = result.primitive_draw_calls;
                *self.shadow_active = result.active;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::MainOpaque => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.camera3d_uniform;
                let camera3d_state = self.camera3d_state;
                let skybox_cubemap_id = self.skybox_cubemap_id;
                let clear_color = self.clear_color;
                let light_uniform = &*self.light_uniform;
                let planned_model_draws = self.planned_model_draws;
                let planned_primitive_draws = self.planned_primitive_draws;
                let planned_primitive_chunks = self.planned_primitive_chunks;
                let main_aux_targets_enabled = self.main_aux_targets_enabled;
                let aspect = self.aspect;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = MainOpaquePassContext {
                    resources: &mut self.resources,
                    encoder,
                    clear_color,
                    camera3d_uniform,
                    camera3d_state,
                    skybox_cubemap_id,
                    light_uniform,
                    model_draws: planned_model_draws,
                    primitive_draws: planned_primitive_draws,
                    primitive_chunks: planned_primitive_chunks,
                    main_aux_targets_enabled,
                    aspect,
                    perf_enabled,
                    perf: &mut *self.perf,
                };
                let result = MainOpaquePassExecutor::execute(&mut context)?;
                *self.primitive_main_stats = result.primitive_stats;
                *self.primitive_main_submitted = self.primitive_main_stats.batch_count > 0;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::MainTransparent => {
                let camera3d_uniform = self.camera3d_uniform;
                let light_uniform = &*self.light_uniform;
                let planned_primitive_draws = self.planned_primitive_draws;
                let planned_primitive_chunks = self.planned_primitive_chunks;
                let perf_enabled = self.perf_enabled;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = MainTransparentPassContext {
                    resources: &mut self.resources,
                    encoder,
                    camera3d_uniform,
                    light_uniform,
                    primitive_draws: planned_primitive_draws,
                    primitive_chunks: planned_primitive_chunks,
                    perf_enabled,
                };
                let result = MainTransparentPassExecutor::execute(&mut context)?;
                *self.primitive_transparent_stats = result.primitive_stats;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Water => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.camera3d_uniform;
                let light_uniform = &*self.light_uniform;
                let planned_water_draws = self.planned_water_draws;
                let planned_water_chunks = self.planned_water_chunks;
                let main_aux_targets_enabled = self.main_aux_targets_enabled;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = WaterPassContext {
                    resources: &mut self.resources,
                    encoder,
                    camera3d_uniform,
                    light_uniform,
                    primitive_draws: planned_water_draws,
                    primitive_chunks: planned_water_chunks,
                    main_aux_targets_enabled,
                    perf_enabled,
                };
                let result = WaterPassExecutor::execute(&mut context)?;
                *self.primitive_water_stats = result.stats;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Post => {
                let perf_enabled = self.perf_enabled;
                let surface_view = self.surface_view;
                let projected_decal_atlas_bindings = self.projected_decal_atlas_bindings;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = PostPassContext {
                    resources: &mut self.resources,
                    encoder,
                    surface_view,
                    projected_decal_atlas_bindings,
                    perf_enabled,
                };
                PostPassExecutor::execute(&mut context)
            }
            RenderPassKind::Overlay => {
                let perf_enabled = self.perf_enabled;
                let surface_view = self.surface_view;
                let frame = self.frame;
                let camera_alignment = self.camera_alignment;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = OverlayPassContext {
                    resources: &mut self.resources,
                    encoder,
                    surface_view,
                    frame,
                    camera_alignment,
                    perf_enabled,
                };
                OverlayPassExecutor::execute(&mut context)
            }
            RenderPassKind::BackendSubmit => {
                let mut context = BackendSubmitPassContext {
                    resources: &mut self.resources,
                    encoder: self.encoder.take(),
                    output: self.output.take(),
                    perf_enabled: self.perf_enabled,
                    perf: &mut *self.perf,
                };
                BackendSubmitPassExecutor::execute(&mut context)
            }
        }
    }

    fn workload(&self, kind: RenderPassKind) -> RenderPassWorkload {
        match kind {
            RenderPassKind::DepthPrepass => DepthPassExecutor::workload(),
            RenderPassKind::Shadow => ShadowPassExecutor::workload(),
            RenderPassKind::MainOpaque => MainOpaquePassExecutor::workload(
                self.planned_model_draws.len(),
                self.planned_primitive_draws.len(),
                self.planned_primitive_chunks.len(),
            ),
            RenderPassKind::MainTransparent => {
                MainTransparentPassExecutor::workload(*self.primitive_transparent_stats)
            }
            RenderPassKind::Water => WaterPassExecutor::workload(*self.primitive_water_stats),
            RenderPassKind::Post => PostPassExecutor::workload(),
            RenderPassKind::Overlay => OverlayPassExecutor::workload(self.frame),
            RenderPassKind::BackendSubmit => BackendSubmitPassExecutor::workload(),
        }
    }
}
