pub(crate) use crate::renderer_frame_resources::*;
mod resource_registry;
pub(crate) use resource_registry::RenderResourceRegistry;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    Capture,
    Readback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderResourceLifetime {
    External,
    Persistent,
    Transient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    pub(crate) backing_generation: u32,
    pub(crate) backing_owner: &'static str,
    pub(crate) ready_cause: &'static str,
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

impl RenderFramePipeline {
    pub(crate) fn from_frame_metrics(
        frame_id: u32,
        command_count: u32,
        scene_mutation_count: u32,
        overlay_command_count: u32,
        decode_ms: f64,
        visible_object_count: u32,
        visible_chunk_count: u32,
        material_group_count: u32,
        diagnostic_flags: u32,
        graph_report: &FrameGraphReport,
        graph_build_ms: f64,
        graph_execute_ms: f64,
        perf_payload_version: u32,
        perf_packet_ms: f64,
    ) -> Self {
        RenderFramePipeline {
            decode: RenderFrameDecode {
                frame_id,
                command_count,
                scene_mutation_count,
                overlay_command_count,
                elapsed_ms: decode_ms.max(0.0),
            },
            snapshot: RenderSceneSnapshot {
                frame_id,
                view_count: 1,
                visible_object_count,
                visible_chunk_count,
                material_group_count,
                diagnostic_flags,
                immutable: true,
            },
            graph_build: FrameGraphBuild {
                frame_id,
                planned_pass_count: graph_report.planned_pass_count,
                resource_count: graph_report.resource_count,
                target_count: graph_report.target_count,
                elapsed_ms: graph_build_ms.max(0.0),
            },
            graph_execute: FrameGraphExecute {
                frame_id,
                executed_pass_count: graph_report.pass_count,
                slowest_pass: graph_report.slowest_pass,
                slowest_pass_ms: graph_report.slowest_pass_ms,
                elapsed_ms: graph_execute_ms.max(0.0),
            },
            perf_packet: PerfPacketEncode {
                frame_id,
                payload_version: perf_payload_version,
                elapsed_ms: perf_packet_ms.max(0.0),
            },
        }
    }
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

    pub(crate) fn dependency_ordered_nodes(&self) -> Result<Vec<RenderPassNode>, String> {
        let nodes = self.nodes();
        let mut producer_by_resource = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if !node.enabled {
                continue;
            }
            for resource in &node.writes {
                producer_by_resource.insert(*resource, index);
            }
        }
        let mut state = vec![0u8; nodes.len()];
        let mut ordered = Vec::with_capacity(nodes.len());
        for index in 0..nodes.len() {
            self.visit_node(
                index,
                &nodes,
                &producer_by_resource,
                &mut state,
                &mut ordered,
            )?;
        }
        Ok(ordered)
    }

    fn visit_node(
        &self,
        index: usize,
        nodes: &[RenderPassNode],
        producer_by_resource: &HashMap<RenderResource, usize>,
        state: &mut [u8],
        ordered: &mut Vec<RenderPassNode>,
    ) -> Result<(), String> {
        if state[index] == 2 {
            return Ok(());
        }
        if state[index] == 1 {
            return Err(format!(
                "voplay: frame graph dependency cycle at {}",
                nodes[index].name
            ));
        }
        state[index] = 1;
        if nodes[index].enabled {
            for resource in &nodes[index].reads {
                if let Some(producer) = producer_by_resource.get(resource) {
                    if *producer != index {
                        self.visit_node(*producer, nodes, producer_by_resource, state, ordered)?;
                    }
                }
            }
        }
        state[index] = 2;
        ordered.push(nodes[index].clone());
        Ok(())
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
                self.registry
                    .mark_ready_with_cause(resource, "frame-pass-write");
            }
        }
    }

    fn validate_ready_reads(&mut self, node: &RenderPassNode) -> Result<(), String> {
        for resource in &node.reads {
            if !self.registry.is_ready(*resource) {
                self.missing_reads = self.missing_reads.saturating_add(1);
                return Err(format!(
                    "voplay: frame graph missing required read {} for pass {}",
                    resource.name, node.name
                ));
            }
        }
        Ok(())
    }

    fn validate_required_writes(&self, node: &RenderPassNode) -> Result<(), String> {
        for resource in &node.writes {
            if !self.registry.is_declared(*resource) {
                return Err(format!(
                    "voplay: frame graph missing required write {} for pass {}",
                    resource.name, node.name
                ));
            }
        }
        Ok(())
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

pub(crate) trait RenderPassNodeDispatcher {
    fn before_execute(&mut self, _kind: RenderPassKind) -> Result<(), String> {
        Ok(())
    }

    fn execute(&mut self, kind: RenderPassKind) -> Result<f64, String>;
    fn workload(&self, kind: RenderPassKind) -> RenderPassWorkload;
    fn after_execute(
        &mut self,
        _kind: RenderPassKind,
        _diagnostic: &RenderPassNodeDiagnostic,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl FrameGraphExecutor<'_> {
    pub(crate) fn execute_all<D>(
        &mut self,
        dispatcher: &mut D,
    ) -> Result<Vec<RenderPassNodeDiagnostic>, String>
    where
        D: RenderPassNodeDispatcher + ?Sized,
    {
        let nodes = self.graph.dependency_ordered_nodes()?;
        let mut diagnostics = Vec::new();
        for node in nodes {
            if let Some(diagnostic) = self.execute_node(&node, true, dispatcher)? {
                dispatcher.after_execute(node.kind, &diagnostic)?;
                diagnostics.push(diagnostic);
            }
        }
        Ok(diagnostics)
    }

    pub(crate) fn execute_node<D>(
        &mut self,
        node: &RenderPassNode,
        enabled: bool,
        dispatcher: &mut D,
    ) -> Result<Option<RenderPassNodeDiagnostic>, String>
    where
        D: RenderPassNodeDispatcher + ?Sized,
    {
        if !node.enabled || !enabled || !self.graph.has_pass(node.kind) {
            return Ok(None);
        }
        dispatcher.before_execute(node.kind)?;
        self.graph.validate_required_writes(node)?;
        self.graph.validate_ready_reads(node)?;
        let elapsed_ms = dispatcher.execute(node.kind)?.max(0.0);
        let workload = dispatcher.workload(node.kind);
        self.graph.record_pass(node.kind, elapsed_ms);
        Ok(Some(RenderPassNodeDiagnostic {
            kind: node.kind,
            name: node.name,
            elapsed_ms,
            workload,
        }))
    }
}

#[cfg(test)]
mod tests;
