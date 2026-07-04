use super::backend_submit_pass::{BackendSubmitPassContext, BackendSubmitPassExecutor};
use super::depth_pass::{DepthPassContext, DepthPassExecutor};
use super::main_opaque_pass::{MainOpaquePassContext, MainOpaquePassExecutor};
use super::main_transparent_pass::MainTransparentPassExecutor;
use super::overlay_pass::{OverlayPassContext, OverlayPassExecutor};
use super::post_pass::{PostPassContext, PostPassExecutor};
use super::shadow_pass::{ShadowPassContext, ShadowPassExecutor};
use super::water_pass::{WaterPassContext, WaterPassExecutor};
use super::*;
use crate::draw_list::Frame2D;
use crate::renderer_frame::RenderPassNodeDispatcher;

pub(super) struct FramePassDispatcher<'a> {
    pub(super) renderer: &'a mut Renderer,
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
    pub(super) primitive_main_submitted: &'a mut bool,
    pub(super) primitive_water_stats: &'a mut PrimitiveDrawStats,
    pub(super) shadow_active: &'a mut bool,
}

impl FramePassDispatcher<'_> {
    pub(super) fn into_frame_parts(
        self,
    ) -> Result<(wgpu::CommandEncoder, wgpu::SurfaceTexture), String> {
        let encoder = self
            .encoder
            .ok_or_else(|| "voplay: frame pass dispatcher missing command encoder".to_string())?;
        let output = self
            .output
            .ok_or_else(|| "voplay: frame pass dispatcher missing surface texture".to_string())?;
        Ok((encoder, output))
    }
}

impl RenderPassNodeDispatcher for FramePassDispatcher<'_> {
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
                    renderer: &mut *self.renderer,
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
                    renderer: &mut *self.renderer,
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
                    renderer: &mut *self.renderer,
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
            RenderPassKind::MainTransparent => MainTransparentPassExecutor::execute(),
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
                    renderer: &mut *self.renderer,
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
                    renderer: &mut *self.renderer,
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
                    renderer: &mut *self.renderer,
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
                    renderer: &mut *self.renderer,
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
            RenderPassKind::MainTransparent => MainTransparentPassExecutor::workload(),
            RenderPassKind::Water => WaterPassExecutor::workload(*self.primitive_water_stats),
            RenderPassKind::Post => PostPassExecutor::workload(),
            RenderPassKind::Overlay => OverlayPassExecutor::workload(self.frame),
            RenderPassKind::BackendSubmit => BackendSubmitPassExecutor::workload(),
        }
    }
}
