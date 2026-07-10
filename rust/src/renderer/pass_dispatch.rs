use super::backend_submit_pass::{BackendSubmitPassContext, BackendSubmitPassExecutor};
use super::depth_pass::{DepthPassContext, DepthPassExecutor};
use super::frame_decode::{
    FramePostSettings, FrameScenePayload, FrameShadowSettings, FrameViewState,
};
use super::frame_workload_plan::FrameWorkloadPlan;
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

pub(super) struct FrameExecutorResources<'a> {
    pub(super) gpu: RenderGpuScope<'a>,
    pub(super) target_registry: &'a RenderResourceRegistry,
    pub(super) post_bindings: RenderPostBindings<'a>,
    pub(super) camera_bind_group: &'a wgpu::BindGroup,
    pub(super) pipelines: RenderPipelineScope<'a>,
    pub(super) assets: RenderAssetScope<'a>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FramePassStats {
    pub(super) primitive_depth_draw_calls: u32,
    pub(super) primitive_shadow_draw_calls: u32,
    pub(super) mesh_main: MeshDrawStats,
    pub(super) primitive_main: PrimitiveDrawStats,
    pub(super) primitive_transparent: PrimitiveDrawStats,
    pub(super) primitive_water: PrimitiveDrawStats,
    pub(super) primitive_main_submitted: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FramePassRuntimeState {
    post_fallback_path_count: u32,
    post_rejected_decal_count: u32,
    post_upload_bytes: u32,
    overlay_missing_texture_count: u32,
    pub(super) shadow_active: bool,
    post_uniforms_uploaded: bool,
}

pub(super) struct FramePassDispatcher<'a> {
    pub(super) resources: FrameExecutorResources<'a>,
    pub(super) encoder: Option<wgpu::CommandEncoder>,
    pub(super) output: Option<wgpu::SurfaceTexture>,
    pub(super) surface_view: &'a wgpu::TextureView,
    pub(super) frame: &'a Frame2D,
    pub(super) camera_alignment: u32,
    pub(super) aspect: f32,
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
    pub(super) view: &'a FrameViewState,
    pub(super) shadow: &'a FrameShadowSettings,
    pub(super) post: &'a FramePostSettings,
    pub(super) scene: &'a mut FrameScenePayload,
    pub(super) workload: &'a mut FrameWorkloadPlan,
    pub(super) stats: FramePassStats,
    pub(super) runtime: FramePassRuntimeState,
}

impl FramePassDispatcher<'_> {
    fn ensure_post_uniforms_uploaded(&mut self) {
        if self.runtime.post_uniforms_uploaded {
            return;
        }
        if !self.runtime.shadow_active {
            self.scene.light_uniform.shadow_vp = math3d::MAT4_IDENTITY;
            self.scene.light_uniform.shadow_cascade_vp = [math3d::MAT4_IDENTITY; 4];
            self.scene.light_uniform.shadow_cascade_splits = [0.0; 4];
            self.scene.light_uniform.shadow_params = [
                0.0,
                0.002,
                self.shadow.shadow_softness,
                self.shadow.shadow_strength,
            ];
            self.scene.light_uniform.shadow_params2 = [
                self.shadow.shadow_distance,
                self.shadow.shadow_fade,
                self.shadow.shadow_quality as f32,
                0.0,
            ];
        }
        let mut post_setup = PostPassSetupContext {
            queue: self.resources.gpu.gpu_queue,
            surface_width: self.resources.gpu.surface.width,
            surface_height: self.resources.gpu.surface.height,
            uniform_buffer: self.resources.post_bindings.uniform_buffer,
            decal_uniform_buffer: self.resources.post_bindings.decal_uniform_buffer,
            camera3d_uniform: self.view.camera3d_uniform.as_ref(),
            camera3d_state: self.view.camera3d_state,
            light_uniform: &mut self.scene.light_uniform,
            projected_decals: &self.workload.planned_projected_decals,
            projected_decal_atlas_binding_count: self.scene.projected_decal_atlas_bindings.len()
                as u32,
            bloom_threshold: self.post.post_bloom_threshold,
            bloom_strength: self.post.post_bloom_strength,
            sharpen_strength: self.post.post_sharpen_strength,
            fxaa_strength: self.post.post_fxaa_strength,
            contact_ao_strength: self.post.post_contact_ao_strength,
            contact_ao_radius: self.post.post_contact_ao_radius,
            contact_ao_depth_scale: self.post.post_contact_ao_depth_scale,
            contact_ao_detail_strength: self.post.post_contact_ao_detail_strength,
            contact_ao_detail_radius: self.post.post_contact_ao_detail_radius,
            contact_ao_normal_bias: self.post.post_contact_ao_normal_bias,
            contact_ao_quality: self.post.post_contact_ao_quality,
        };
        let decal_report = PostPassSetup::upload_uniforms(&mut post_setup);
        self.runtime.post_rejected_decal_count =
            decal_report.rejected_count.min(u32::MAX as usize) as u32;
        self.runtime.post_upload_bytes = decal_report
            .upload_bytes
            .saturating_add(std::mem::size_of::<PostUniform>().min(u32::MAX as usize) as u32);
        self.runtime.post_uniforms_uploaded = true;
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
                let camera3d_uniform = self.view.camera3d_uniform.as_ref();
                let model_draws = &self.workload.planned_model_draws;
                let primitive_depth_chunks = &self.workload.primitive_depth_chunks;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = DepthPassContext {
                    device: self.resources.gpu.gpu_device,
                    queue: self.resources.gpu.gpu_queue,
                    depth_pipeline: &mut *self.resources.pipelines.depth,
                    primitive_pipeline: &mut *self.resources.pipelines.primitive,
                    models: self.resources.assets.models,
                    encoder,
                    camera3d_uniform,
                    model_draws,
                    primitive_depth_draws: &mut self.workload.primitive_depth_draws,
                    primitive_depth_chunks,
                    perf_enabled,
                };
                let result = DepthPassExecutor::execute(&mut context)?;
                self.stats.primitive_depth_draw_calls = result.primitive_draw_calls;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Shadow => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.view.camera3d_uniform.as_ref();
                let camera3d_state = self.view.camera3d_state;
                let model_draws = &self.workload.planned_model_draws;
                let primitive_shadow_chunks = &self.workload.primitive_shadow_chunks;
                let retained_scene_draws = &self.scene.retained_scene_draws;
                let shadow_resolution = self.shadow.shadow_resolution;
                let shadow_quality = self.shadow.shadow_quality;
                let shadow_distance = self.shadow.shadow_distance;
                let shadow_fade = self.shadow.shadow_fade;
                let shadow_softness = self.shadow.shadow_softness;
                let shadow_strength = self.shadow.shadow_strength;
                let aspect = self.aspect;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = ShadowPassContext {
                    device: self.resources.gpu.gpu_device,
                    queue: self.resources.gpu.gpu_queue,
                    mesh_pipeline: &mut *self.resources.pipelines.mesh3d,
                    primitive_pipeline: &mut *self.resources.pipelines.primitive,
                    shadow_pipeline: &mut *self.resources.pipelines.shadow,
                    world: self.resources.assets.world,
                    models: self.resources.assets.models,
                    encoder,
                    camera3d_uniform,
                    camera3d_state,
                    light_uniform: &mut self.scene.light_uniform,
                    model_draws,
                    primitive_shadow_draws: &mut self.workload.primitive_shadow_draws,
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
                self.stats.primitive_shadow_draw_calls = result.primitive_draw_calls;
                self.runtime.shadow_active = result.active;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::MainOpaque => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.view.camera3d_uniform.as_ref();
                let camera3d_state = self.view.camera3d_state;
                let skybox_cubemap_id = self.view.skybox_cubemap_id;
                let clear_color = self.view.clear_color;
                let light_uniform = &self.scene.light_uniform;
                let planned_model_draws = &self.workload.planned_model_draws;
                let planned_primitive_draws = &self.workload.planned_primitive_draws;
                let planned_primitive_chunks = &self.workload.planned_primitive_chunks;
                let main_aux_targets_enabled = self.workload.post_depth_active;
                let aspect = self.aspect;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = MainOpaquePassContext {
                    device: self.resources.gpu.gpu_device,
                    queue: self.resources.gpu.gpu_queue,
                    targets: self.resources.target_registry,
                    mesh_pipeline: &mut *self.resources.pipelines.mesh3d,
                    primitive_pipeline: &mut *self.resources.pipelines.primitive,
                    shadow_pipeline: &*self.resources.pipelines.shadow,
                    skybox_pipeline: &mut *self.resources.pipelines.skybox,
                    models: self.resources.assets.models,
                    textures: self.resources.assets.textures,
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
                self.stats.mesh_main = result.mesh_stats;
                self.stats.primitive_main = result.primitive_stats;
                self.stats.primitive_main_submitted = self.stats.primitive_main.batch_count > 0;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::MainTransparent => {
                let camera3d_uniform = self.view.camera3d_uniform.as_ref();
                let light_uniform = &self.scene.light_uniform;
                let planned_primitive_draws = &self.workload.planned_primitive_draws;
                let planned_primitive_chunks = &self.workload.planned_primitive_chunks;
                let perf_enabled = self.perf_enabled;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = MainTransparentPassContext {
                    device: self.resources.gpu.gpu_device,
                    queue: self.resources.gpu.gpu_queue,
                    targets: self.resources.target_registry,
                    primitive_pipeline: &mut *self.resources.pipelines.primitive,
                    shadow_pipeline: &*self.resources.pipelines.shadow,
                    models: self.resources.assets.models,
                    textures: self.resources.assets.textures,
                    encoder,
                    camera3d_uniform,
                    light_uniform,
                    primitive_draws: planned_primitive_draws,
                    primitive_chunks: planned_primitive_chunks,
                    perf_enabled,
                };
                let result = MainTransparentPassExecutor::execute(&mut context)?;
                self.stats.primitive_transparent = result.primitive_stats;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Water => {
                let perf_enabled = self.perf_enabled;
                let camera3d_uniform = self.view.camera3d_uniform.as_ref();
                let light_uniform = &self.scene.light_uniform;
                let planned_water_draws = &self.workload.planned_water_draws;
                let planned_water_chunks = &self.workload.planned_water_chunks;
                let main_aux_targets_enabled = self.workload.post_depth_active;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = WaterPassContext {
                    device: self.resources.gpu.gpu_device,
                    queue: self.resources.gpu.gpu_queue,
                    targets: self.resources.target_registry,
                    primitive_pipeline: &mut *self.resources.pipelines.primitive,
                    shadow_pipeline: &*self.resources.pipelines.shadow,
                    models: self.resources.assets.models,
                    textures: self.resources.assets.textures,
                    encoder,
                    camera3d_uniform,
                    light_uniform,
                    primitive_draws: planned_water_draws,
                    primitive_chunks: planned_water_chunks,
                    main_aux_targets_enabled,
                    perf_enabled,
                };
                let result = WaterPassExecutor::execute(&mut context)?;
                self.stats.primitive_water = result.stats;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::Post => {
                let perf_enabled = self.perf_enabled;
                let surface_view = self.surface_view;
                let projected_decal_atlas_bindings = &self.scene.projected_decal_atlas_bindings;
                let encoder = self.encoder.as_mut().ok_or_else(|| {
                    "voplay: frame pass dispatcher missing command encoder".to_string()
                })?;
                let mut context = PostPassContext {
                    device: self.resources.gpu.gpu_device,
                    targets: self.resources.target_registry,
                    post_pipeline: self.resources.pipelines.post,
                    depth_pipeline: &*self.resources.pipelines.depth,
                    uniform_buffer: self.resources.post_bindings.uniform_buffer,
                    decal_uniform_buffer: self.resources.post_bindings.decal_uniform_buffer,
                    default_bind_group: self.resources.post_bindings.bind_group,
                    textures: self.resources.assets.textures,
                    encoder,
                    surface_view,
                    projected_decal_atlas_bindings,
                    perf_enabled,
                };
                let result = PostPassExecutor::execute(&mut context)?;
                self.runtime.post_fallback_path_count = result.fallback_path_count;
                Ok(result.elapsed_ms)
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
                    two_d_pipeline: &mut *self.resources.pipelines.two_d,
                    sprite_pipeline: &mut *self.resources.pipelines.sprite,
                    camera_bind_group: self.resources.camera_bind_group,
                    textures: self.resources.assets.textures,
                    encoder,
                    surface_view,
                    frame,
                    camera_alignment,
                    perf_enabled,
                };
                let result = OverlayPassExecutor::execute(&mut context)?;
                self.runtime.overlay_missing_texture_count = result.missing_texture_count;
                Ok(result.elapsed_ms)
            }
            RenderPassKind::BackendSubmit => {
                let mut context = BackendSubmitPassContext {
                    queue: self.resources.gpu.gpu_queue,
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
            RenderPassKind::MainOpaque => {
                MainOpaquePassExecutor::workload(self.stats.mesh_main, self.stats.primitive_main)
            }
            RenderPassKind::MainTransparent => {
                MainTransparentPassExecutor::workload(self.stats.primitive_transparent)
            }
            RenderPassKind::Water => WaterPassExecutor::workload(self.stats.primitive_water),
            RenderPassKind::Post => PostPassExecutor::workload(
                self.runtime.post_fallback_path_count,
                self.runtime.post_rejected_decal_count,
                self.runtime.post_upload_bytes,
            ),
            RenderPassKind::Overlay => OverlayPassExecutor::workload(
                self.frame,
                self.runtime.overlay_missing_texture_count,
            ),
            RenderPassKind::BackendSubmit => BackendSubmitPassExecutor::workload(),
        }
    }
}
