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
        self.resources
            .validate_backing(RES_MAIN_COLOR)
            .map_err(|failure| failure.structured_message())?;
        self.resources
            .validate_backing(RES_DEPTH)
            .map_err(|failure| failure.structured_message())?;
        if desc.post_depth_active {
            self.resources
                .validate_backing(RES_RECEIVER_MASK)
                .map_err(|failure| failure.structured_message())?;
            self.resources
                .validate_backing(RES_SURFACE_PROPS)
                .map_err(|failure| failure.structured_message())?;
        }
        let mut frame_graph = FrameGraph::single_view(desc.frame_id, desc.diagnostic_flags);
        frame_graph.declare_external_target(RES_SURFACE_COLOR, true);
        frame_graph.declare_target(RES_MAIN_COLOR, true);
        frame_graph.declare_target(RES_DEPTH, true);
        frame_graph.declare_target(RES_SHADOW_MAP, true);
        frame_graph.declare_target(RES_RECEIVER_MASK, true);
        frame_graph.declare_target(RES_SURFACE_PROPS, true);
        frame_graph.plan_standard_passes(FrameGraphPassOptions {
            depth_prepass: desc.depth_prepass_active,
            shadow: desc.shadow_enabled
                && desc.camera3d_enabled
                && (!desc.model_draws_empty
                    || !desc.primitive_shadow_draws_empty
                    || !desc.primitive_shadow_chunks_empty),
            transparent: desc.transparent_pass_active,
            water: desc.water_pass_active,
        });
        let build_ms = elapsed_ms_opt(graph_build_start);
        Ok(FrameGraphPlanOutput {
            frame_graph,
            build_ms,
        })
    }
}
