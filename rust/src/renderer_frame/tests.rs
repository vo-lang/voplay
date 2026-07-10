use super::*;

struct TimedTestDispatcher {
    elapsed_ms: f64,
}

impl RenderPassNodeDispatcher for TimedTestDispatcher {
    fn execute(&mut self, _kind: RenderPassKind) -> Result<f64, String> {
        Ok(self.elapsed_ms)
    }

    fn workload(&self, _kind: RenderPassKind) -> RenderPassWorkload {
        RenderPassWorkload::default()
    }
}

fn execute_test_pass(
    graph: &mut FrameGraph,
    kind: RenderPassKind,
    elapsed_ms: f64,
) -> Result<Option<f64>, String> {
    let node = graph
        .node(kind)
        .ok_or_else(|| format!("missing test frame graph node {}", kind.name()))?;
    let mut dispatcher = TimedTestDispatcher { elapsed_ms };
    Ok(graph
        .executor()
        .execute_node(&node, true, &mut dispatcher)?
        .map(|diagnostic| diagnostic.elapsed_ms))
}

fn standard_test_graph(options: FrameGraphPassOptions) -> FrameGraph {
    let mut graph = FrameGraph::single_view(99, 0);
    graph.declare_external_target(RES_SURFACE_COLOR, true);
    graph.declare_target(RES_MAIN_COLOR, true);
    graph.declare_target(RES_DEPTH, true);
    graph.declare_target(RES_SHADOW_MAP, true);
    graph.declare_target(RES_RECEIVER_MASK, true);
    graph.declare_target(RES_SURFACE_PROPS, true);
    graph.plan_standard_passes(options);
    graph
}

#[test]
fn frame_graph_standard_plan_executes_all_feature_combinations_without_cycles() {
    for mask in 0u8..16 {
        let options = FrameGraphPassOptions {
            depth_prepass: mask & 1 != 0,
            shadow: mask & 2 != 0,
            transparent: mask & 4 != 0,
            water: mask & 8 != 0,
        };
        let mut graph = standard_test_graph(options);
        let diagnostics = graph
            .executor()
            .execute_all(&mut TimedTestDispatcher { elapsed_ms: 0.1 })
            .unwrap_or_else(|error| panic!("feature mask {mask:#06b} failed: {error}"));
        let executed = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.kind)
            .collect::<Vec<_>>();
        let mut expected = Vec::new();
        if options.depth_prepass {
            expected.push(RenderPassKind::DepthPrepass);
        }
        if options.shadow {
            expected.push(RenderPassKind::Shadow);
        }
        expected.push(RenderPassKind::MainOpaque);
        if options.transparent {
            expected.push(RenderPassKind::MainTransparent);
        }
        if options.water {
            expected.push(RenderPassKind::Water);
        }
        expected.extend([
            RenderPassKind::Post,
            RenderPassKind::Overlay,
            RenderPassKind::BackendSubmit,
        ]);
        assert_eq!(executed, expected, "feature mask {mask:#06b}");
    }
}

#[test]
fn frame_graph_reads_bind_to_latest_prior_resource_version() {
    let graph = standard_test_graph(FrameGraphPassOptions {
        depth_prepass: true,
        shadow: true,
        transparent: true,
        water: true,
    });
    let nodes = graph.nodes();
    let post = nodes
        .iter()
        .find(|node| node.kind == RenderPassKind::Post)
        .expect("post node");
    let post_main_color = post
        .read_versions
        .iter()
        .find(|version| version.resource == RES_MAIN_COLOR)
        .expect("post main color read");
    assert_eq!(post_main_color.version, 3);

    let overlay = nodes
        .iter()
        .find(|node| node.kind == RenderPassKind::Overlay)
        .expect("overlay node");
    assert_eq!(overlay.read_versions[0].resource, RES_SURFACE_COLOR);
    assert_eq!(overlay.read_versions[0].version, 1);
    assert_eq!(overlay.write_versions[0].version, 2);

    let backend = nodes
        .iter()
        .find(|node| node.kind == RenderPassKind::BackendSubmit)
        .expect("backend submit node");
    assert_eq!(backend.read_versions[0].resource, RES_SURFACE_COLOR);
    assert_eq!(backend.read_versions[0].version, 2);
    assert!(backend.write_versions.is_empty());
}

#[test]
fn frame_graph_depth_attachment_contract_preserves_downstream_depth_reads() {
    for kind in [
        RenderPassKind::MainOpaque,
        RenderPassKind::MainTransparent,
        RenderPassKind::Water,
    ] {
        assert_eq!(
            depth_attachment_store_contract(kind),
            RenderAttachmentStoreContract::Store,
            "{} must preserve depth for downstream load or sampling",
            kind.name(),
        );
    }
}

#[test]
fn frame_graph_reports_passes_resources_and_slowest_pass() {
    let mut graph = FrameGraph::single_view(42, 0);
    graph.declare_target(RES_DEPTH, true);
    graph.declare_target(RES_SHADOW_MAP, true);
    graph.declare_target(RES_MAIN_COLOR, true);
    graph.declare_target(RES_SURFACE_COLOR, true);
    graph.declare_target(RES_RECEIVER_MASK, true);
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
    assert_eq!(
        execute_test_pass(&mut graph, RenderPassKind::Shadow, 0.8).unwrap(),
        Some(0.8)
    );
    assert_eq!(
        execute_test_pass(&mut graph, RenderPassKind::MainOpaque, 2.4).unwrap(),
        Some(2.4)
    );
    assert_eq!(
        execute_test_pass(&mut graph, RenderPassKind::Post, 1.1).unwrap(),
        Some(1.1)
    );
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
    graph.declare_target(RES_DEPTH, true);
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
    assert_eq!(
        execute_test_pass(&mut graph, RenderPassKind::MainTransparent, 0.5).unwrap(),
        None
    );
    assert_eq!(
        execute_test_pass(&mut graph, RenderPassKind::Water, 0.7).unwrap(),
        Some(0.7)
    );
    let nodes = graph.nodes();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[1].transient_writes, vec![RES_WATER_COLOR]);
    assert_eq!(nodes[1].diagnostics.len(), 1);
    let report = graph.report();
    assert_eq!(report.pass_count, 1);
    assert_eq!(report.ready_target_count, 2);
    assert_eq!(report.transient_target_count, 1);
    assert_eq!(report.missing_read_count, 0);
}

#[test]
fn frame_graph_executor_fails_on_missing_required_read() {
    let mut graph = FrameGraph::single_view(14, 0);
    graph.plan_transient_pass(
        RenderPassKind::Water,
        &[RES_DEPTH],
        &[RES_WATER_COLOR],
        true,
    );
    let node = graph.node(RenderPassKind::Water).unwrap();
    let err = graph
        .executor()
        .execute_node(&node, true, &mut TimedTestDispatcher { elapsed_ms: 0.7 })
        .unwrap_err();
    assert!(err.contains("missing required read depth"));
    assert!(err.contains("voplay.render.failure pass=water stage=validate_reads"));
    assert_eq!(graph.report().missing_read_count, 1);
    assert_eq!(graph.report().failure_count, 1);
}

#[test]
fn frame_graph_executor_fails_on_missing_required_write() {
    let mut graph = FrameGraph::single_view(15, 0);
    graph.declare_target(RES_DEPTH, true);
    graph.plan_pass(
        RenderPassKind::Shadow,
        &[RES_DEPTH],
        &[RES_SHADOW_MAP],
        true,
    );
    let node = graph.node(RenderPassKind::Shadow).unwrap();
    let err = graph
        .executor()
        .execute_node(&node, true, &mut TimedTestDispatcher { elapsed_ms: 0.7 })
        .unwrap_err();
    assert!(err.contains("missing required write shadow-map"));
    assert!(err.contains("voplay.render.failure pass=shadow stage=validate_writes"));
    assert_eq!(graph.report().failure_count, 1);
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
fn frame_graph_registry_requires_actual_backing_generation_for_ready_targets() {
    let mut registry = RenderResourceRegistry::default();
    registry.declare_target(RES_DEPTH, true, RenderResourceLifetime::Persistent);
    assert!(registry.is_ready(RES_DEPTH));
    let failure = registry.validate_backing(RES_DEPTH).unwrap_err();
    assert_eq!(failure.code, "identity_missing");
    assert!(failure
        .structured_message()
        .contains("stage=frame_graph_resource"));
    assert!(registry.actual_texture_view(RES_DEPTH).is_none());
    registry.mark_resize_generation(3);
    let target = registry
        .targets()
        .iter()
        .find(|target| target.resource == RES_DEPTH)
        .expect("depth target status");
    assert_eq!(target.backing_generation, 0);
    assert!(target.backing_identity.is_none());
    assert_eq!(
        registry.validate_backing(RES_DEPTH).unwrap_err().code,
        "target_not_ready"
    );
}

#[test]
fn frame_graph_declares_capture_and_readback_lifetimes() {
    let mut graph = FrameGraph::single_view(11, 0);
    graph.declare_target(RES_SHADOW_MAP, true);
    graph.declare_transient_target(RES_WATER_COLOR, false);
    graph.declare_external_target(RES_OVERLAY, true);
    graph.declare_transient_target(RES_CAPTURE, false);
    graph.declare_external_target(RES_READBACK, false);
    let report = graph.report();

    assert_eq!(report.persistent_target_count, 1);
    assert_eq!(report.transient_target_count, 2);
    assert_eq!(report.external_target_count, 2);
}

#[test]
fn frame_graph_executor_dispatches_node_and_reports_workload() {
    struct TestDispatcher {
        elapsed_ms: f64,
        workload: RenderPassWorkload,
    }

    impl RenderPassNodeDispatcher for TestDispatcher {
        fn execute(&mut self, _kind: RenderPassKind) -> Result<f64, String> {
            Ok(self.elapsed_ms)
        }

        fn workload(&self, _kind: RenderPassKind) -> RenderPassWorkload {
            self.workload
        }
    }

    let mut graph = FrameGraph::single_view(12, 0);
    graph.declare_target(RES_MAIN_COLOR, true);
    graph.declare_external_target(RES_OVERLAY, false);
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
    let mut dispatcher = TestDispatcher {
        elapsed_ms: 0.42,
        workload: RenderPassWorkload {
            draw_calls: 3,
            batches: 2,
            instances: 8,
            triangles: 96,
            upload_bytes: 128,
            skips: RenderSkipStats {
                filtered_draws: 4,
                missing_textures: 1,
                invalid_batch_indices: 2,
                fallback_paths: 3,
                ..RenderSkipStats::default()
            },
        },
    };
    let diagnostic = graph
        .executor()
        .execute_node(&node, true, &mut dispatcher)
        .unwrap()
        .unwrap();
    assert_eq!(diagnostic.name, "overlay");
    assert_eq!(diagnostic.workload.draw_calls, 3);
    assert_eq!(diagnostic.workload.instances, 8);
    let report = graph.report();
    assert_eq!(report.pass_count, 1);
    assert_eq!(report.ready_target_count, 2);
    assert_eq!(report.skip_stats.filtered_draws, 4);
    assert_eq!(report.skip_stats.missing_resources(), 1);
    assert_eq!(report.skip_stats.invalid_batches(), 2);
    assert_eq!(report.skip_stats.fallback_paths, 3);
}

#[test]
fn render_frame_pipeline_captures_stage_metrics() {
    let mut graph = FrameGraph::single_view(13, 0x20);
    graph.declare_target(RES_MAIN_COLOR, true);
    graph.plan_pass(
        RenderPassKind::Overlay,
        &[RES_MAIN_COLOR],
        &[RES_OVERLAY],
        true,
    );
    graph.record_pass(RenderPassKind::Overlay, 0.75);
    let report = graph.report();

    let pipeline = RenderFramePipeline::from_frame_metrics(
        13,
        44,
        3,
        12,
        0.5,
        18,
        7,
        4,
        0x20,
        &report,
        0.2,
        report.total_pass_ms,
        6,
        0.1,
    );

    assert_eq!(pipeline.decode.command_count, 44);
    assert_eq!(pipeline.snapshot.visible_chunk_count, 7);
    assert_eq!(
        pipeline.graph_build.planned_pass_count,
        report.planned_pass_count
    );
    assert_eq!(pipeline.graph_execute.executed_pass_count, 1);
    assert_eq!(pipeline.perf_packet.payload_version, 6);
}

#[test]
fn render_target_status_rejects_stale_or_mismatched_physical_backing() {
    let identity = RenderBackingIdentity {
        serial: 41,
        generation: 7,
        slot: RenderBackingSlot::PostColor,
    };
    let descriptor = RenderBackingDescriptor {
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: 1280,
        height: 720,
        sample_count: 1,
    };
    let target = RenderTargetStatus {
        resource: RES_POST_COLOR,
        ready: true,
        lifetime: RenderResourceLifetime::Persistent,
        revision: 7,
        backing_generation: 7,
        backing_identity: Some(identity),
        backing_descriptor: Some(descriptor),
        backing_owner: "post",
        ready_cause: "allocated",
    };

    assert!(target.matches_backing(7, Some(identity), Some(descriptor), true));
    assert!(!target.matches_backing(8, Some(identity), Some(descriptor), true));
    assert!(!target.matches_backing(
        7,
        Some(RenderBackingIdentity {
            serial: identity.serial + 1,
            ..identity
        }),
        Some(descriptor),
        true
    ));
    assert!(!target.matches_backing(
        7,
        Some(identity),
        Some(RenderBackingDescriptor {
            width: descriptor.width + 1,
            ..descriptor
        }),
        true
    ));
    assert!(!target.matches_backing(
        7,
        Some(identity),
        Some(RenderBackingDescriptor {
            format: wgpu::TextureFormat::Rgba8Unorm,
            ..descriptor
        }),
        true
    ));
    let descriptor_failure = target
        .validate_backing(
            7,
            Some(identity),
            Some(RenderBackingDescriptor {
                sample_count: 4,
                ..descriptor
            }),
            true,
        )
        .unwrap_err();
    assert_eq!(descriptor_failure.code, "descriptor_mismatch");
    assert!(descriptor_failure
        .structured_message()
        .contains(&format!("resource={}", RES_POST_COLOR.name)));
    assert!(!target.matches_backing(7, Some(identity), Some(descriptor), false));
}
