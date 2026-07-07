use super::*;

pub(super) struct FrameGraphPlanDesc {
    pub(super) frame_id: u32,
    pub(super) diagnostic_flags: u32,
    pub(super) perf_enabled: bool,
    pub(super) post_depth_active: bool,
    pub(super) depth_prepass_active: bool,
    pub(super) shadow_enabled: bool,
    pub(super) camera3d_enabled: bool,
    pub(super) model_draws_empty: bool,
    pub(super) primitive_shadow_draws_empty: bool,
    pub(super) primitive_shadow_chunks_empty: bool,
    pub(super) transparent_pass_active: bool,
    pub(super) water_pass_active: bool,
}

pub(super) struct FrameGraphPlanOutput {
    pub(super) frame_graph: FrameGraph,
    pub(super) build_ms: f64,
}

impl Renderer {
    pub(super) fn build_frame_graph_plan(
        &self,
        desc: FrameGraphPlanDesc,
    ) -> Result<FrameGraphPlanOutput, String> {
        let graph_build_start = if desc.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let mut frame_graph = FrameGraph::single_view(desc.frame_id, desc.diagnostic_flags);
        frame_graph.declare_external_target(RES_SURFACE_COLOR, true);
        frame_graph.declare_target(
            RES_MAIN_COLOR,
            self.resources.validate_backing_generation(RES_MAIN_COLOR),
        );
        frame_graph.declare_target(
            RES_DEPTH,
            self.resources.validate_backing_generation(RES_DEPTH),
        );
        frame_graph.declare_target(RES_SHADOW_MAP, true);
        frame_graph.declare_target(
            RES_POST_COLOR,
            self.resources.validate_backing_generation(RES_POST_COLOR),
        );
        frame_graph.declare_external_target(RES_OVERLAY, true);
        frame_graph.declare_transient_target(RES_CAPTURE, false);
        frame_graph.declare_external_target(RES_READBACK, false);
        frame_graph.declare_target(
            RES_RECEIVER_MASK,
            !desc.post_depth_active
                || self
                    .resources
                    .validate_backing_generation(RES_RECEIVER_MASK),
        );
        frame_graph.declare_target(
            RES_SURFACE_PROPS,
            !desc.post_depth_active
                || self
                    .resources
                    .validate_backing_generation(RES_SURFACE_PROPS),
        );
        frame_graph.plan_pass(
            RenderPassKind::DepthPrepass,
            &[],
            &[RES_DEPTH],
            desc.depth_prepass_active,
        );
        frame_graph.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            desc.shadow_enabled
                && desc.camera3d_enabled
                && (!desc.model_draws_empty
                    || !desc.primitive_shadow_draws_empty
                    || !desc.primitive_shadow_chunks_empty),
        );
        frame_graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_SHADOW_MAP],
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::MainTransparent,
            &[RES_MAIN_COLOR, RES_DEPTH],
            &[RES_MAIN_COLOR],
            desc.transparent_pass_active,
        );
        frame_graph.plan_transient_pass(
            RenderPassKind::Water,
            &[RES_DEPTH, RES_MAIN_COLOR],
            &[RES_WATER_COLOR, RES_MAIN_COLOR],
            desc.water_pass_active,
        );
        frame_graph.plan_pass(
            RenderPassKind::Post,
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            &[RES_POST_COLOR, RES_SURFACE_COLOR],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::Overlay,
            &[RES_SURFACE_COLOR],
            &[RES_OVERLAY],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::BackendSubmit,
            &[RES_OVERLAY],
            &[RES_SURFACE_COLOR],
            true,
        );
        let build_ms = elapsed_ms_opt(graph_build_start);
        Ok(FrameGraphPlanOutput {
            frame_graph,
            build_ms,
        })
    }
}
