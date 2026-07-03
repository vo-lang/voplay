#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderResourceKind {
    SurfaceColor,
    MainColor,
    Depth,
    ReceiverMask,
    SurfaceProps,
    ShadowMap,
    PostColor,
    WaterColor,
    Overlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderResource {
    pub(crate) name: &'static str,
    pub(crate) kind: RenderResourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderView {
    pub(crate) id: u32,
    pub(crate) enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderPassKind {
    DepthPrepass,
    Shadow,
    MainOpaque,
    MainTransparent,
    Water,
    Post,
    Overlay,
    BackendSubmit,
}

impl RenderPassKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            RenderPassKind::DepthPrepass => "depth",
            RenderPassKind::Shadow => "shadow",
            RenderPassKind::MainOpaque => "main-opaque",
            RenderPassKind::MainTransparent => "main-transparent",
            RenderPassKind::Water => "water",
            RenderPassKind::Post => "post",
            RenderPassKind::Overlay => "overlay",
            RenderPassKind::BackendSubmit => "backend-submit",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderTargetStatus {
    pub(crate) resource: RenderResource,
    pub(crate) ready: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassDiagnostic {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) elapsed_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameGraphPassPlan {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassNode {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) enabled: bool,
    pub(crate) diagnostics: Vec<RenderPassDiagnostic>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderFrameSnapshot {
    pub(crate) frame_id: u32,
    pub(crate) views: Vec<RenderView>,
    pub(crate) diagnostic_flags: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameGraphReport {
    pub(crate) frame_id: u32,
    pub(crate) planned_pass_count: u32,
    pub(crate) pass_count: u32,
    pub(crate) resource_count: u32,
    pub(crate) target_count: u32,
    pub(crate) ready_target_count: u32,
    pub(crate) slowest_pass: &'static str,
    pub(crate) slowest_pass_ms: f64,
    pub(crate) total_pass_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameGraph {
    frame: RenderFrameSnapshot,
    targets: Vec<RenderTargetStatus>,
    planned_passes: Vec<FrameGraphPassPlan>,
    passes: Vec<RenderPassDiagnostic>,
}

pub(crate) struct FrameGraphExecutor<'a> {
    graph: &'a mut FrameGraph,
}

impl FrameGraph {
    pub(crate) fn single_view(frame_id: u32, diagnostic_flags: u32) -> Self {
        Self {
            frame: RenderFrameSnapshot {
                frame_id,
                views: vec![RenderView {
                    id: 0,
                    enabled: true,
                }],
                diagnostic_flags,
            },
            targets: Vec::new(),
            planned_passes: Vec::new(),
            passes: Vec::new(),
        }
    }

    pub(crate) fn declare_target(&mut self, resource: RenderResource, ready: bool) {
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            existing.ready = existing.ready || ready;
            return;
        }
        self.targets.push(RenderTargetStatus { resource, ready });
    }

    pub(crate) fn plan_pass(
        &mut self,
        kind: RenderPassKind,
        reads: &[RenderResource],
        writes: &[RenderResource],
        enabled: bool,
    ) {
        self.planned_passes.push(FrameGraphPassPlan {
            kind,
            name: kind.name(),
            reads: reads.to_vec(),
            writes: writes.to_vec(),
            enabled,
        });
    }

    pub(crate) fn executor(&mut self) -> FrameGraphExecutor<'_> {
        FrameGraphExecutor { graph: self }
    }

    pub(crate) fn nodes(&self) -> Vec<RenderPassNode> {
        self.planned_passes
            .iter()
            .map(|plan| RenderPassNode {
                kind: plan.kind,
                name: plan.name,
                reads: plan.reads.clone(),
                writes: plan.writes.clone(),
                enabled: plan.enabled,
                diagnostics: self
                    .passes
                    .iter()
                    .filter(|pass| pass.kind == plan.kind)
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    pub(crate) fn has_pass(&self, kind: RenderPassKind) -> bool {
        self.planned_passes
            .iter()
            .any(|pass| pass.kind == kind && pass.enabled)
    }

    pub(crate) fn record_pass(&mut self, kind: RenderPassKind, elapsed_ms: f64) {
        if let Some(plan) = self
            .planned_passes
            .iter()
            .find(|pass| pass.kind == kind && pass.enabled)
        {
            self.passes.push(RenderPassDiagnostic {
                kind,
                name: plan.name,
                reads: plan.reads.clone(),
                writes: plan.writes.clone(),
                elapsed_ms: elapsed_ms.max(0.0),
            });
            let writes = plan.writes.clone();
            for resource in writes {
                self.declare_target(resource, true);
            }
        }
    }

    pub(crate) fn report(&self) -> FrameGraphReport {
        let mut resources = Vec::<RenderResource>::new();
        let mut slowest_pass = "";
        let mut slowest_pass_ms = 0.0;
        let mut total_pass_ms = 0.0;
        for target in &self.targets {
            if !resources.contains(&target.resource) {
                resources.push(target.resource);
            }
        }
        for pass in &self.passes {
            total_pass_ms += pass.elapsed_ms;
            if pass.elapsed_ms >= slowest_pass_ms {
                slowest_pass_ms = pass.elapsed_ms;
                slowest_pass = pass.name;
            }
            for resource in pass.reads.iter().chain(pass.writes.iter()) {
                if !resources.contains(resource) {
                    resources.push(*resource);
                }
            }
        }
        FrameGraphReport {
            frame_id: self.frame.frame_id,
            planned_pass_count: self.nodes().len().min(u32::MAX as usize) as u32,
            pass_count: self.passes.len().min(u32::MAX as usize) as u32,
            resource_count: resources.len().min(u32::MAX as usize) as u32,
            target_count: self.targets.len().min(u32::MAX as usize) as u32,
            ready_target_count: self
                .targets
                .iter()
                .filter(|target| target.ready)
                .count()
                .min(u32::MAX as usize) as u32,
            slowest_pass,
            slowest_pass_ms,
            total_pass_ms,
        }
    }
}

impl FrameGraphExecutor<'_> {
    pub(crate) fn execute_recorded(&mut self, kind: RenderPassKind, elapsed_ms: f64) -> bool {
        if !self.graph.has_pass(kind) {
            return false;
        }
        self.graph.record_pass(kind, elapsed_ms);
        true
    }
}

pub(crate) const RES_SURFACE_COLOR: RenderResource = RenderResource {
    name: "surface-color",
    kind: RenderResourceKind::SurfaceColor,
};
pub(crate) const RES_MAIN_COLOR: RenderResource = RenderResource {
    name: "main-color",
    kind: RenderResourceKind::MainColor,
};
pub(crate) const RES_DEPTH: RenderResource = RenderResource {
    name: "depth",
    kind: RenderResourceKind::Depth,
};
pub(crate) const RES_RECEIVER_MASK: RenderResource = RenderResource {
    name: "receiver-mask",
    kind: RenderResourceKind::ReceiverMask,
};
pub(crate) const RES_SURFACE_PROPS: RenderResource = RenderResource {
    name: "surface-props",
    kind: RenderResourceKind::SurfaceProps,
};
pub(crate) const RES_SHADOW_MAP: RenderResource = RenderResource {
    name: "shadow-map",
    kind: RenderResourceKind::ShadowMap,
};
pub(crate) const RES_POST_COLOR: RenderResource = RenderResource {
    name: "post-color",
    kind: RenderResourceKind::PostColor,
};
pub(crate) const RES_WATER_COLOR: RenderResource = RenderResource {
    name: "water-color",
    kind: RenderResourceKind::WaterColor,
};
pub(crate) const RES_OVERLAY: RenderResource = RenderResource {
    name: "overlay",
    kind: RenderResourceKind::Overlay,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_graph_reports_passes_resources_and_slowest_pass() {
        let mut graph = FrameGraph::single_view(42, 0);
        graph.declare_target(RES_DEPTH, true);
        graph.declare_target(RES_SHADOW_MAP, true);
        graph.declare_target(RES_MAIN_COLOR, true);
        graph.declare_target(RES_SURFACE_COLOR, true);
        graph.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            true,
        );
        graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_SHADOW_MAP],
            &[RES_MAIN_COLOR, RES_DEPTH, RES_RECEIVER_MASK],
            true,
        );
        graph.plan_pass(
            RenderPassKind::Post,
            &[RES_MAIN_COLOR],
            &[RES_SURFACE_COLOR],
            true,
        );
        {
            let mut executor = graph.executor();
            assert!(executor.execute_recorded(RenderPassKind::Shadow, 0.8));
            assert!(executor.execute_recorded(RenderPassKind::MainOpaque, 2.4));
            assert!(executor.execute_recorded(RenderPassKind::Post, 1.1));
        }
        let report = graph.report();
        assert_eq!(report.frame_id, 42);
        assert_eq!(report.pass_count, 3);
        assert_eq!(report.resource_count, 5);
        assert_eq!(report.target_count, 5);
        assert_eq!(report.ready_target_count, 5);
        assert_eq!(report.slowest_pass, "main-opaque");
        assert_eq!(report.slowest_pass_ms, 2.4);
    }

    #[test]
    fn frame_graph_pass_plan_controls_execution() {
        let mut graph = FrameGraph::single_view(7, 0);
        graph.plan_pass(RenderPassKind::DepthPrepass, &[], &[RES_DEPTH], false);
        graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_DEPTH],
            &[RES_MAIN_COLOR],
            true,
        );
        assert!(!graph.has_pass(RenderPassKind::DepthPrepass));
        assert!(graph.has_pass(RenderPassKind::MainOpaque));
        graph.record_pass(RenderPassKind::DepthPrepass, 4.0);
        graph.record_pass(RenderPassKind::MainOpaque, 1.25);
        let report = graph.report();
        assert_eq!(report.pass_count, 1);
        assert_eq!(report.slowest_pass, "main-opaque");
    }

    #[test]
    fn frame_graph_executor_records_nodes_and_marks_targets_ready() {
        let mut graph = FrameGraph::single_view(8, 0);
        graph.declare_target(RES_MAIN_COLOR, false);
        graph.plan_pass(
            RenderPassKind::MainTransparent,
            &[RES_MAIN_COLOR],
            &[RES_SURFACE_COLOR],
            false,
        );
        graph.plan_pass(
            RenderPassKind::Water,
            &[RES_DEPTH],
            &[RES_WATER_COLOR],
            true,
        );
        {
            let mut executor = graph.executor();
            assert!(!executor.execute_recorded(RenderPassKind::MainTransparent, 0.5));
            assert!(executor.execute_recorded(RenderPassKind::Water, 0.7));
        }
        let nodes = graph.nodes();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].diagnostics.len(), 1);
        let report = graph.report();
        assert_eq!(report.pass_count, 1);
        assert_eq!(report.ready_target_count, 1);
    }
}
