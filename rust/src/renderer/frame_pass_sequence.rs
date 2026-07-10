use super::frame_decode::{
    FramePostSettings, FrameScenePayload, FrameShadowSettings, FrameViewState,
};
use super::frame_workload_plan::FrameWorkloadPlan;
use super::pass_dispatch::{
    FrameExecutorResources, FramePassDispatcher, FramePassRuntimeState, FramePassStats,
};
use super::*;
use crate::draw_list::Frame2D;

pub(super) struct FramePassSequenceContext<'a> {
    pub(super) frame_graph: &'a mut FrameGraph,
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FramePassTimings {
    pub(super) depth_pass_ms: f64,
    pub(super) shadow_pass_ms: f64,
    pub(super) main_pass_ms: f64,
    pub(super) water_pass_ms: f64,
    pub(super) post_pass_ms: f64,
    pub(super) overlay_pass_ms: f64,
    pub(super) shadow_active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FramePassSequenceResult {
    pub(super) timings: FramePassTimings,
    pub(super) stats: FramePassStats,
}

impl Renderer {
    pub(super) fn execute_frame_pass_sequence(
        &mut self,
        context: FramePassSequenceContext<'_>,
    ) -> Result<FramePassSequenceResult, String> {
        let mut dispatcher = FramePassDispatcher {
            resources: FrameExecutorResources {
                gpu: super::pass_dispatch::RenderGpuScope {
                    gpu_device: &self.device,
                    gpu_queue: &self.queue,
                    surface: &self.surface_config,
                },
                target_registry: &self.resources,
                post_bindings: super::pass_dispatch::RenderPostBindings {
                    uniform_buffer: &self.post_uniform_buffer,
                    decal_uniform_buffer: &self.post_decal_uniform_buffer,
                    bind_group: &self.post_bind_group,
                },
                camera_bind_group: &self.camera_bind_group,
                pipelines: super::pass_dispatch::RenderPipelineScope {
                    two_d: &mut self.pipeline2d,
                    sprite: &mut self.pipeline_sprite,
                    mesh3d: &mut self.pipeline3d,
                    primitive: &mut self.primitive_pipeline,
                    depth: &mut self.pipeline_depth,
                    shadow: &mut self.pipeline_shadow,
                    skybox: &mut self.pipeline_skybox,
                    post: &self.pipeline_post,
                },
                assets: super::pass_dispatch::RenderAssetScope {
                    models: &self.model_manager,
                    textures: &self.texture_manager,
                    world: &self.render_world,
                },
            },
            encoder: context.encoder,
            output: context.output,
            surface_view: context.surface_view,
            frame: context.frame,
            camera_alignment: context.camera_alignment,
            aspect: context.aspect,
            perf_enabled: context.perf_enabled,
            perf: context.perf,
            view: context.view,
            shadow: context.shadow,
            post: context.post,
            scene: context.scene,
            workload: context.workload,
            stats: FramePassStats::default(),
            runtime: FramePassRuntimeState::default(),
        };
        let diagnostics = context
            .frame_graph
            .executor()
            .execute_all(&mut dispatcher)?;
        let stats = dispatcher.stats;
        let shadow_active = dispatcher.runtime.shadow_active;
        drop(dispatcher);
        let elapsed = |kind| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.kind == kind)
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0)
        };
        Ok(FramePassSequenceResult {
            timings: FramePassTimings {
                depth_pass_ms: elapsed(RenderPassKind::DepthPrepass),
                shadow_pass_ms: elapsed(RenderPassKind::Shadow),
                main_pass_ms: elapsed(RenderPassKind::MainOpaque),
                water_pass_ms: elapsed(RenderPassKind::Water),
                post_pass_ms: elapsed(RenderPassKind::Post),
                overlay_pass_ms: elapsed(RenderPassKind::Overlay),
                shadow_active,
            },
            stats,
        })
    }
}
