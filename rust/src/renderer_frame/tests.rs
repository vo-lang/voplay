
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
        .execute_node(
            &node,
            true,
            &mut TimedTestDispatcher { elapsed_ms: 0.7 },
        )
        .unwrap_err();
    assert!(err.contains("missing required read depth"));
    assert_eq!(graph.report().missing_read_count, 1);
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
        .execute_node(
            &node,
            true,
            &mut TimedTestDispatcher { elapsed_ms: 0.7 },
        )
        .unwrap_err();
    assert!(err.contains("missing required write shadow-map"));
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
