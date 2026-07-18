pub(crate) use crate::renderer_frame_resources::*;
mod frame_resource_ledger;
mod resource_backing;
mod resource_registry;
use crate::render_world::RenderSkipStats;
use frame_resource_ledger::FrameResourceLedger;
pub(crate) use resource_backing::*;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RenderResourceVersion {
    pub(crate) resource: RenderResource,
    pub(crate) version: u32,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderAttachmentStoreContract {
    Store,
    Discard,
}

impl RenderAttachmentStoreContract {
    pub(crate) fn wgpu_store_op(self) -> wgpu::StoreOp {
        match self {
            Self::Store => wgpu::StoreOp::Store,
            Self::Discard => wgpu::StoreOp::Discard,
        }
    }
}

pub(crate) fn depth_attachment_store_contract(
    kind: RenderPassKind,
) -> RenderAttachmentStoreContract {
    match kind {
        RenderPassKind::MainOpaque | RenderPassKind::MainTransparent | RenderPassKind::Water => {
            RenderAttachmentStoreContract::Store
        }
        _ => RenderAttachmentStoreContract::Discard,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassDiagnostic {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) elapsed_ms: f64,
    pub(crate) workload: RenderPassWorkload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderPassFailureDiagnostic {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameGraphPassPlan {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) read_versions: Vec<RenderResourceVersion>,
    pub(crate) write_versions: Vec<RenderResourceVersion>,
    pub(crate) enabled: bool,
    pub(crate) transient: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderPassNode {
    pub(crate) kind: RenderPassKind,
    pub(crate) name: &'static str,
    pub(crate) reads: Vec<RenderResource>,
    pub(crate) writes: Vec<RenderResource>,
    pub(crate) read_versions: Vec<RenderResourceVersion>,
    pub(crate) write_versions: Vec<RenderResourceVersion>,
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
    pub(crate) skips: RenderSkipStats,
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

pub(crate) struct RenderFrameMetrics<'a> {
    pub(crate) frame_id: u32,
    pub(crate) command_count: u32,
    pub(crate) scene_mutation_count: u32,
    pub(crate) overlay_command_count: u32,
    pub(crate) decode_ms: f64,
    pub(crate) visible_object_count: u32,
    pub(crate) visible_chunk_count: u32,
    pub(crate) material_group_count: u32,
    pub(crate) diagnostic_flags: u32,
    pub(crate) graph_report: &'a FrameGraphReport,
    pub(crate) graph_build_ms: f64,
    pub(crate) graph_execute_ms: f64,
    pub(crate) perf_payload_version: u32,
    pub(crate) perf_packet_ms: f64,
}

impl RenderFramePipeline {
    pub(crate) fn from_frame_metrics(metrics: RenderFrameMetrics<'_>) -> Self {
        let RenderFrameMetrics {
            frame_id,
            command_count,
            scene_mutation_count,
            overlay_command_count,
            decode_ms,
            visible_object_count,
            visible_chunk_count,
            material_group_count,
            diagnostic_flags,
            graph_report,
            graph_build_ms,
            graph_execute_ms,
            perf_payload_version,
            perf_packet_ms,
        } = metrics;
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
    pub(crate) skipped_pass_count: u32,
    pub(crate) failure_count: u32,
    pub(crate) skip_stats: RenderSkipStats,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameGraphPassOptions {
    pub(crate) depth_prepass: bool,
    pub(crate) shadow: bool,
    pub(crate) transparent: bool,
    pub(crate) water: bool,
}

pub(crate) struct FrameGraph {
    frame: RenderFrameSnapshot,
    resources: FrameResourceLedger,
    planned_passes: Vec<FrameGraphPassPlan>,
    planned_resource_versions: HashMap<RenderResource, u32>,
    passes: Vec<RenderPassDiagnostic>,
    failures: Vec<RenderPassFailureDiagnostic>,
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
            resources: FrameResourceLedger::default(),
            planned_passes: Vec::new(),
            planned_resource_versions: HashMap::new(),
            passes: Vec::new(),
            failures: Vec::new(),
            missing_reads: 0,
        }
    }

    pub(crate) fn declare_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::Persistent);
    }

    pub(crate) fn declare_external_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::External);
    }

    #[cfg(test)]
    pub(crate) fn declare_transient_target(&mut self, resource: RenderResource, ready: bool) {
        self.declare_target_with_lifetime(resource, ready, RenderResourceLifetime::Transient);
    }

    pub(crate) fn declare_target_with_lifetime(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
    ) {
        self.resources.declare_target(resource, ready, lifetime);
        self.planned_resource_versions.entry(resource).or_insert(0);
    }

    pub(crate) fn plan_pass(
        &mut self,
        kind: RenderPassKind,
        reads: &[RenderResource],
        writes: &[RenderResource],
        enabled: bool,
    ) {
        self.plan_pass_with_lifetime(kind, reads, writes, enabled, false);
    }

    fn plan_pass_with_lifetime(
        &mut self,
        kind: RenderPassKind,
        reads: &[RenderResource],
        writes: &[RenderResource],
        enabled: bool,
        transient: bool,
    ) {
        let read_versions = reads
            .iter()
            .map(|resource| RenderResourceVersion {
                resource: *resource,
                version: *self.planned_resource_versions.get(resource).unwrap_or(&0),
            })
            .collect();
        let mut write_versions = Vec::with_capacity(writes.len());
        for resource in writes {
            let current = *self.planned_resource_versions.get(resource).unwrap_or(&0);
            let version = current.saturating_add(1);
            write_versions.push(RenderResourceVersion {
                resource: *resource,
                version,
            });
            if enabled {
                self.planned_resource_versions.insert(*resource, version);
            }
        }
        self.planned_passes.push(FrameGraphPassPlan {
            kind,
            name: kind.name(),
            reads: reads.to_vec(),
            writes: writes.to_vec(),
            read_versions,
            write_versions,
            enabled,
            transient,
        });
    }

    #[cfg(test)]
    pub(crate) fn plan_transient_pass(
        &mut self,
        kind: RenderPassKind,
        reads: &[RenderResource],
        writes: &[RenderResource],
        enabled: bool,
    ) {
        if enabled {
            for resource in writes {
                self.declare_transient_target(*resource, false);
            }
        }
        self.plan_pass_with_lifetime(kind, reads, writes, enabled, true);
    }

    pub(crate) fn plan_standard_passes(&mut self, options: FrameGraphPassOptions) {
        self.plan_pass(
            RenderPassKind::DepthPrepass,
            &[],
            &[RES_DEPTH],
            options.depth_prepass,
        );
        self.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            options.shadow,
        );
        self.plan_pass(
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
        self.plan_pass(
            RenderPassKind::MainTransparent,
            &[RES_MAIN_COLOR, RES_DEPTH],
            &[RES_MAIN_COLOR],
            options.transparent,
        );
        self.plan_pass(
            RenderPassKind::Water,
            &[RES_DEPTH, RES_MAIN_COLOR],
            &[RES_MAIN_COLOR],
            options.water,
        );
        self.plan_pass(
            RenderPassKind::Post,
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            &[RES_SURFACE_COLOR],
            true,
        );
        self.plan_pass(
            RenderPassKind::Overlay,
            &[RES_SURFACE_COLOR],
            &[RES_SURFACE_COLOR],
            true,
        );
        self.plan_pass(
            RenderPassKind::BackendSubmit,
            &[RES_SURFACE_COLOR],
            &[],
            true,
        );
    }

    #[cfg(test)]
    pub(crate) fn mark_resize_generation(&mut self, generation: u32) {
        self.resources.mark_resize_generation(generation);
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
                read_versions: plan.read_versions.clone(),
                write_versions: plan.write_versions.clone(),
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
        let mut producer_by_version = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if !node.enabled {
                continue;
            }
            for resource in &node.write_versions {
                producer_by_version.insert(*resource, index);
            }
        }
        let mut state = vec![0u8; nodes.len()];
        let mut ordered = Vec::with_capacity(nodes.len());
        for index in 0..nodes.len() {
            self.visit_node(
                index,
                &nodes,
                &producer_by_version,
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
        producer_by_version: &HashMap<RenderResourceVersion, usize>,
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
            for resource in &nodes[index].read_versions {
                if let Some(producer) = producer_by_version.get(resource) {
                    if *producer != index {
                        self.visit_node(*producer, nodes, producer_by_version, state, ordered)?;
                    }
                }
            }
            for resource in &nodes[index].write_versions {
                if resource.version == 0 {
                    continue;
                }
                let previous = RenderResourceVersion {
                    resource: resource.resource,
                    version: resource.version - 1,
                };
                if let Some(producer) = producer_by_version.get(&previous) {
                    if *producer != index {
                        self.visit_node(*producer, nodes, producer_by_version, state, ordered)?;
                    }
                }
            }
        }
        state[index] = 2;
        ordered.push(nodes[index].clone());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn node(&self, kind: RenderPassKind) -> Option<RenderPassNode> {
        self.nodes().into_iter().find(|node| node.kind == kind)
    }

    pub(crate) fn has_pass(&self, kind: RenderPassKind) -> bool {
        self.planned_passes
            .iter()
            .any(|pass| pass.kind == kind && pass.enabled)
    }

    #[cfg(test)]
    pub(crate) fn record_pass(&mut self, kind: RenderPassKind, elapsed_ms: f64) {
        self.record_pass_with_workload(kind, elapsed_ms, RenderPassWorkload::default());
    }

    pub(crate) fn record_pass_with_workload(
        &mut self,
        kind: RenderPassKind,
        elapsed_ms: f64,
        workload: RenderPassWorkload,
    ) {
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
                workload,
            });
            for resource in &plan.reads {
                if !self.resources.is_ready(*resource) {
                    self.missing_reads = self.missing_reads.saturating_add(1);
                }
            }
            let writes = plan.writes.clone();
            for resource in writes {
                self.resources.mark_ready(resource);
            }
        }
    }

    fn record_failure(
        &mut self,
        node: &RenderPassNode,
        stage: &'static str,
        detail: String,
    ) -> String {
        self.failures.push(RenderPassFailureDiagnostic {
            kind: node.kind,
            name: node.name,
            stage,
            detail: detail.clone(),
        });
        format!(
            "voplay.render.failure pass={} stage={} detail={}",
            node.name, stage, detail
        )
    }

    fn validate_ready_reads(&mut self, node: &RenderPassNode) -> Result<(), String> {
        for resource in &node.reads {
            if !self.resources.is_ready(*resource) {
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
            if !self.resources.is_declared(*resource) {
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
        for resource in self.resources.declared_resources() {
            if !resources.contains(&resource) {
                resources.push(resource);
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
                if self.resources.is_declared(*resource) && !active_targets.contains(resource) {
                    active_targets.push(*resource);
                }
            }
        }
        let mut skip_stats = RenderSkipStats {
            missing_targets: self.missing_reads,
            ..RenderSkipStats::default()
        };
        for pass in &self.passes {
            skip_stats.merge(pass.workload.skips);
        }
        FrameGraphReport {
            frame_id: self.frame.frame_id,
            planned_pass_count: self.nodes().len().min(u32::MAX as usize) as u32,
            pass_count: self.passes.len().min(u32::MAX as usize) as u32,
            resource_count: resources.len().min(u32::MAX as usize) as u32,
            target_count: active_targets.len().min(u32::MAX as usize) as u32,
            ready_target_count: active_targets
                .iter()
                .filter(|resource| self.resources.is_ready(**resource))
                .count()
                .min(u32::MAX as usize) as u32,
            slowest_pass,
            slowest_pass_ms,
            total_pass_ms,
            transient_target_count: self
                .resources
                .count_lifetime(RenderResourceLifetime::Transient),
            persistent_target_count: self
                .resources
                .count_lifetime(RenderResourceLifetime::Persistent),
            external_target_count: self
                .resources
                .count_lifetime(RenderResourceLifetime::External),
            missing_read_count: self.missing_reads,
            skipped_pass_count: self
                .planned_passes
                .iter()
                .filter(|pass| !pass.enabled)
                .count()
                .min(u32::MAX as usize) as u32,
            failure_count: self.failures.len().min(u32::MAX as usize) as u32,
            skip_stats,
            resize_generation: self.resources.resize_generation,
            resource_churn: self.resources.churn,
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
                if let Err(error) = dispatcher.after_execute(node.kind, &diagnostic) {
                    return Err(self.graph.record_failure(&node, "after_execute", error));
                }
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
        if let Err(error) = dispatcher.before_execute(node.kind) {
            return Err(self.graph.record_failure(node, "before_execute", error));
        }
        if let Err(error) = self.graph.validate_required_writes(node) {
            return Err(self.graph.record_failure(node, "validate_writes", error));
        }
        if let Err(error) = self.graph.validate_ready_reads(node) {
            return Err(self.graph.record_failure(node, "validate_reads", error));
        }
        let elapsed_ms = match dispatcher.execute(node.kind) {
            Ok(elapsed_ms) => elapsed_ms.max(0.0),
            Err(error) => return Err(self.graph.record_failure(node, "execute", error)),
        };
        let workload = dispatcher.workload(node.kind);
        self.graph
            .record_pass_with_workload(node.kind, elapsed_ms, workload);
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
