use crate::renderer_targets::{
    create_depth_view, create_msaa_color_view, create_msaa_receiver_mask_view,
    create_msaa_surface_props_view, create_post_color_view, create_receiver_mask_view,
    create_surface_props_view, MAIN_SAMPLE_COUNT,
};

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
pub(crate) enum RenderResourceLifetime {
    External,
    Persistent,
    Transient,
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
    pub(crate) lifetime: RenderResourceLifetime,
    pub(crate) revision: u32,
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
    pub(crate) transient: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassNode {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) transient_writes: Vec<RenderResource>,
    pub(crate) enabled: bool,
    pub(crate) diagnostics: Vec<RenderPassDiagnostic>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderPassWorkload {
    pub(crate) draw_calls: u32,
    pub(crate) batches: u32,
    pub(crate) instances: u32,
    pub(crate) triangles: u32,
    pub(crate) upload_bytes: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassNodeDiagnostic {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) elapsed_ms: f64,
    pub(crate) workload: RenderPassWorkload,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderFrameSnapshot {
    pub(crate) frame_id: u32,
    pub(crate) views: Vec<RenderView>,
    pub(crate) diagnostic_flags: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RenderFrameDecode {
    pub(crate) frame_id: u32,
    pub(crate) command_count: u32,
    pub(crate) scene_mutation_count: u32,
    pub(crate) overlay_command_count: u32,
    pub(crate) elapsed_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RenderSceneSnapshot {
    pub(crate) frame_id: u32,
    pub(crate) view_count: u32,
    pub(crate) visible_object_count: u32,
    pub(crate) visible_chunk_count: u32,
    pub(crate) material_group_count: u32,
    pub(crate) diagnostic_flags: u32,
    pub(crate) immutable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameGraphBuild {
    pub(crate) frame_id: u32,
    pub(crate) planned_pass_count: u32,
    pub(crate) resource_count: u32,
    pub(crate) target_count: u32,
    pub(crate) elapsed_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FrameGraphExecute {
    pub(crate) frame_id: u32,
    pub(crate) executed_pass_count: u32,
    pub(crate) slowest_pass: &'static str,
    pub(crate) slowest_pass_ms: f64,
    pub(crate) elapsed_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PerfPacketEncode {
    pub(crate) frame_id: u32,
    pub(crate) payload_version: u32,
    pub(crate) elapsed_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RenderFramePipeline {
    pub(crate) decode: RenderFrameDecode,
    pub(crate) snapshot: RenderSceneSnapshot,
    pub(crate) graph_build: FrameGraphBuild,
    pub(crate) graph_execute: FrameGraphExecute,
    pub(crate) perf_packet: PerfPacketEncode,
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
    pub(crate) transient_target_count: u32,
    pub(crate) persistent_target_count: u32,
    pub(crate) external_target_count: u32,
    pub(crate) missing_read_count: u32,
    pub(crate) resize_generation: u32,
    pub(crate) resource_churn: RenderResourceChurn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderResourceChurn {
    pub(crate) target_creates: u32,
    pub(crate) target_reuses: u32,
    pub(crate) target_recreates: u32,
    pub(crate) alias_reuses: u32,
}

#[derive(Default)]
struct RenderTargetStore {
    depth_view: Option<wgpu::TextureView>,
    msaa_color_view: Option<wgpu::TextureView>,
    post_color_view: Option<wgpu::TextureView>,
    msaa_receiver_mask_view: Option<wgpu::TextureView>,
    receiver_mask_view: Option<wgpu::TextureView>,
    msaa_surface_props_view: Option<wgpu::TextureView>,
    surface_props_view: Option<wgpu::TextureView>,
}

pub(crate) struct RenderResourceRegistry {
    targets: Vec<RenderTargetStatus>,
    churn: RenderResourceChurn,
    resize_generation: u32,
    store: RenderTargetStore,
}

impl Default for RenderResourceRegistry {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            churn: RenderResourceChurn::default(),
            resize_generation: 0,
            store: RenderTargetStore::default(),
        }
    }
}

pub(crate) struct FrameGraph {
    frame: RenderFrameSnapshot,
    registry: RenderResourceRegistry,
    planned_passes: Vec<FrameGraphPassPlan>,
    passes: Vec<RenderPassDiagnostic>,
    missing_reads: u32,
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
            registry: RenderResourceRegistry::default(),
            planned_passes: Vec::new(),
            passes: Vec::new(),
            missing_reads: 0,
        }
    }

    pub(crate) fn declare_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::Persistent);
    }

    pub(crate) fn declare_external_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::External);
    }

    pub(crate) fn declare_transient_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::Transient);
    }

    pub(crate) fn declare_target_with_lifetime(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
    ) {
        self.registry.declare_target(resource, ready, lifetime);
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
            transient: false,
        });
    }

    pub(crate) fn plan_transient_pass(
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
            transient: true,
        });
        if enabled {
            for resource in writes {
                self.declare_transient_target(*resource, false);
            }
        }
    }

    pub(crate) fn mark_resize_generation(&mut self, generation: u32) {
        self.registry.mark_resize_generation(generation);
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
                transient_writes: if plan.transient {
                    plan.writes.clone()
                } else {
                    Vec::new()
                },
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

    pub(crate) fn node(&self, kind: RenderPassKind) -> Option<RenderPassNode> {
        self.nodes().into_iter().find(|node| node.kind == kind)
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
            for resource in &plan.reads {
                if !self.registry.is_ready(*resource) {
                    self.missing_reads = self.missing_reads.saturating_add(1);
                }
            }
            let writes = plan.writes.clone();
            for resource in writes {
                self.registry.mark_ready(resource);
            }
        }
    }

    pub(crate) fn report(&self) -> FrameGraphReport {
        let mut resources = Vec::<RenderResource>::new();
        let mut active_targets = Vec::<RenderResource>::new();
        let mut slowest_pass = "";
        let mut slowest_pass_ms = 0.0;
        let mut total_pass_ms = 0.0;
        for target in self.registry.targets() {
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
                if self
                    .registry
                    .targets()
                    .iter()
                    .any(|target| target.resource == *resource)
                    && !active_targets.contains(resource)
                {
                    active_targets.push(*resource);
                }
            }
        }
        FrameGraphReport {
            frame_id: self.frame.frame_id,
            planned_pass_count: self.nodes().len().min(u32::MAX as usize) as u32,
            pass_count: self.passes.len().min(u32::MAX as usize) as u32,
            resource_count: resources.len().min(u32::MAX as usize) as u32,
            target_count: active_targets.len().min(u32::MAX as usize) as u32,
            ready_target_count: active_targets
                .iter()
                .filter(|resource| self.registry.is_ready(**resource))
                .count()
                .min(u32::MAX as usize) as u32,
            slowest_pass,
            slowest_pass_ms,
            total_pass_ms,
            transient_target_count: self
                .registry
                .count_lifetime(RenderResourceLifetime::Transient),
            persistent_target_count: self
                .registry
                .count_lifetime(RenderResourceLifetime::Persistent),
            external_target_count: self
                .registry
                .count_lifetime(RenderResourceLifetime::External),
            missing_read_count: self.missing_reads,
            resize_generation: self.registry.resize_generation,
            resource_churn: self.registry.churn,
        }
    }
}

impl FrameGraphExecutor<'_> {
    pub(crate) fn execute_node<F, W>(
        &mut self,
        node: &RenderPassNode,
        enabled: bool,
        execute: F,
        workload: W,
    ) -> Result<Option<RenderPassNodeDiagnostic>, String>
    where
        F: FnOnce() -> Result<f64, String>,
        W: FnOnce() -> RenderPassWorkload,
    {
        if !node.enabled || !enabled || !self.graph.has_pass(node.kind) {
            return Ok(None);
        }
        let elapsed_ms = execute()?.max(0.0);
        let workload = workload();
        self.graph.record_pass(node.kind, elapsed_ms);
        Ok(Some(RenderPassNodeDiagnostic {
            kind: node.kind,
            name: node.name,
            elapsed_ms,
            workload,
        }))
    }

    pub(crate) fn execute_pass<F>(
        &mut self,
        kind: RenderPassKind,
        execute: F,
    ) -> Result<Option<f64>, String>
    where
        F: FnOnce() -> Result<f64, String>,
    {
        if !self.graph.has_pass(kind) {
            return Ok(None);
        }
        let elapsed_ms = execute()?.max(0.0);
        self.graph.record_pass(kind, elapsed_ms);
        Ok(Some(elapsed_ms))
    }
}

impl RenderResourceRegistry {
    pub(crate) fn new_render_targets(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let mut registry = Self::default();
        registry.recreate_render_targets(device, width, height, surface_format);
        registry
    }

    pub(crate) fn recreate_render_targets(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) {
        self.store = RenderTargetStore {
            depth_view: Some(create_depth_view(device, width, height, MAIN_SAMPLE_COUNT)),
            msaa_color_view: create_msaa_color_view(
                device,
                width,
                height,
                surface_format,
                MAIN_SAMPLE_COUNT,
            ),
            post_color_view: Some(create_post_color_view(
                device,
                width,
                height,
                surface_format,
            )),
            msaa_receiver_mask_view: create_msaa_receiver_mask_view(
                device,
                width,
                height,
                MAIN_SAMPLE_COUNT,
            ),
            receiver_mask_view: Some(create_receiver_mask_view(
                device,
                width,
                height,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                "voplay_receiver_mask",
            )),
            msaa_surface_props_view: create_msaa_surface_props_view(
                device,
                width,
                height,
                MAIN_SAMPLE_COUNT,
            ),
            surface_props_view: Some(create_surface_props_view(
                device,
                width,
                height,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                "voplay_surface_props",
            )),
        };
        let next_generation = self.resize_generation.saturating_add(1);
        self.mark_resize_generation(next_generation);
        self.declare_target(
            RES_DEPTH,
            self.store.depth_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_MAIN_COLOR,
            self.main_color_ready(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_POST_COLOR,
            self.store.post_color_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_RECEIVER_MASK,
            self.store.receiver_mask_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_SURFACE_PROPS,
            self.store.surface_props_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
    }

    pub(crate) fn main_color_ready(&self) -> bool {
        self.store.post_color_view.is_some() || self.store.msaa_color_view.is_some()
    }

    pub(crate) fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.store.depth_view.as_ref()
    }

    pub(crate) fn msaa_color_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_color_view.as_ref()
    }

    pub(crate) fn post_color_view(&self) -> Option<&wgpu::TextureView> {
        self.store.post_color_view.as_ref()
    }

    pub(crate) fn msaa_receiver_mask_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_receiver_mask_view.as_ref()
    }

    pub(crate) fn receiver_mask_view(&self) -> Option<&wgpu::TextureView> {
        self.store.receiver_mask_view.as_ref()
    }

    pub(crate) fn msaa_surface_props_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_surface_props_view.as_ref()
    }

    pub(crate) fn surface_props_view(&self) -> Option<&wgpu::TextureView> {
        self.store.surface_props_view.as_ref()
    }

    pub(crate) fn resize_generation(&self) -> u32 {
        self.resize_generation
    }

    pub(crate) fn declare_target(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
    ) {
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            existing.ready = existing.ready || ready;
            if existing.lifetime == RenderResourceLifetime::Transient
                && lifetime == RenderResourceLifetime::Transient
            {
                self.churn.target_reuses = self.churn.target_reuses.saturating_add(1);
            } else if existing.lifetime != lifetime {
                existing.lifetime = lifetime;
                existing.revision = existing.revision.saturating_add(1);
                self.churn.target_recreates = self.churn.target_recreates.saturating_add(1);
            } else {
                self.churn.alias_reuses = self.churn.alias_reuses.saturating_add(1);
            }
            return;
        }
        self.targets.push(RenderTargetStatus {
            resource,
            ready,
            lifetime,
            revision: self.resize_generation,
        });
        self.churn.target_creates = self.churn.target_creates.saturating_add(1);
    }

    pub(crate) fn mark_ready(&mut self, resource: RenderResource) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            target.ready = true;
            return;
        }
        self.declare_target(resource, true, RenderResourceLifetime::Persistent);
    }

    pub(crate) fn is_ready(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource && target.ready)
    }

    pub(crate) fn targets(&self) -> &[RenderTargetStatus] {
        &self.targets
    }

    pub(crate) fn count_lifetime(&self, lifetime: RenderResourceLifetime) -> u32 {
        self.targets
            .iter()
            .filter(|target| target.lifetime == lifetime)
            .count()
            .min(u32::MAX as usize) as u32
    }

    pub(crate) fn mark_resize_generation(&mut self, generation: u32) {
        if generation == self.resize_generation {
            return;
        }
        self.resize_generation = generation;
        for target in &mut self.targets {
            target.revision = generation;
            target.ready = false;
        }
        self.churn.target_recreates = self
            .churn
            .target_recreates
            .saturating_add(self.targets.len().min(u32::MAX as usize) as u32);
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
            assert_eq!(
                executor
                    .execute_pass(RenderPassKind::Shadow, || Ok(0.8))
                    .unwrap(),
                Some(0.8)
            );
            assert_eq!(
                executor
                    .execute_pass(RenderPassKind::MainOpaque, || Ok(2.4))
                    .unwrap(),
                Some(2.4)
            );
            assert_eq!(
                executor
                    .execute_pass(RenderPassKind::Post, || Ok(1.1))
                    .unwrap(),
                Some(1.1)
            );
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
        graph.plan_transient_pass(
            RenderPassKind::Water,
            &[RES_DEPTH],
            &[RES_WATER_COLOR],
            false,
        );
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
        assert_eq!(report.transient_target_count, 0);
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
        graph.plan_transient_pass(
            RenderPassKind::Water,
            &[RES_DEPTH],
            &[RES_WATER_COLOR],
            true,
        );
        {
            let mut executor = graph.executor();
            assert_eq!(
                executor
                    .execute_pass(RenderPassKind::MainTransparent, || Ok(0.5))
                    .unwrap(),
                None
            );
            assert_eq!(
                executor
                    .execute_pass(RenderPassKind::Water, || Ok(0.7))
                    .unwrap(),
                Some(0.7)
            );
        }
        let nodes = graph.nodes();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].transient_writes, vec![RES_WATER_COLOR]);
        assert_eq!(nodes[1].diagnostics.len(), 1);
        let report = graph.report();
        assert_eq!(report.pass_count, 1);
        assert_eq!(report.ready_target_count, 1);
        assert_eq!(report.transient_target_count, 1);
        assert_eq!(report.missing_read_count, 1);
    }

    #[test]
    fn frame_graph_target_readiness_counts_executed_pass_targets() {
        let mut graph = FrameGraph::single_view(10, 0);
        graph.declare_target(RES_POST_COLOR, false);
        graph.declare_target(RES_MAIN_COLOR, true);
        graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_MAIN_COLOR],
            &[RES_SURFACE_COLOR],
            true,
        );
        graph.record_pass(RenderPassKind::MainOpaque, 0.3);
        let report = graph.report();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.ready_target_count, 2);
        assert_eq!(report.persistent_target_count, 3);
    }

    #[test]
    fn frame_graph_registry_tracks_resize_and_lifetime_churn() {
        let mut graph = FrameGraph::single_view(9, 0);
        graph.declare_external_target(RES_SURFACE_COLOR, true);
        graph.declare_transient_target(RES_POST_COLOR, false);
        graph.declare_transient_target(RES_POST_COLOR, true);
        graph.mark_resize_generation(2);
        let report = graph.report();
        assert_eq!(report.external_target_count, 1);
        assert_eq!(report.transient_target_count, 1);
        assert_eq!(report.resize_generation, 2);
        assert!(report.resource_churn.target_creates >= 2);
        assert!(report.resource_churn.target_reuses >= 1);
        assert!(report.resource_churn.target_recreates >= 2);
    }

    #[test]
    fn frame_graph_executor_runs_node_closure_and_reports_workload() {
        let mut graph = FrameGraph::single_view(12, 0);
        graph.declare_target(RES_MAIN_COLOR, true);
        graph.plan_pass(
            RenderPassKind::Overlay,
            &[RES_MAIN_COLOR],
            &[RES_OVERLAY],
            true,
        );
        let node = graph
            .nodes()
            .into_iter()
            .find(|node| node.kind == RenderPassKind::Overlay)
            .unwrap();
        let diagnostic = graph
            .executor()
            .execute_node(
                &node,
                true,
                || Ok(0.42),
                || RenderPassWorkload {
                    draw_calls: 3,
                    batches: 2,
                    instances: 8,
                    triangles: 96,
                    upload_bytes: 128,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(diagnostic.name, "overlay");
        assert_eq!(diagnostic.workload.draw_calls, 3);
        assert_eq!(diagnostic.workload.instances, 8);
        let report = graph.report();
        assert_eq!(report.pass_count, 1);
        assert_eq!(report.ready_target_count, 2);
    }
}
