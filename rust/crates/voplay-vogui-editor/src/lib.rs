use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod play_mode_wire;
pub use play_mode_wire::*;

use vogui_runtime::tree::{ViewKind, ViewNode};
use voplay_protocol::{EngineId, Handle};
use voplay_runtime::asset::{AssetInspectionEntry, AssetServer};
use voplay_runtime::editor_inspection::{
    EditorInspectionError, EditorInspectionService, EditorPanelKind, EditorPanelPage,
    PerfPanelSample, RenderPanelEntry,
};
use voplay_runtime::editor_tools::{
    EditorToolError, EditorToolService, PickCandidate, PickRequest, PickResult, SelectionSnapshot,
};
use voplay_runtime::fault_injection::{FaultInjectionMetrics, FaultRule, FaultTraceEvent};
use voplay_runtime::fault_wire::{
    decode_fault_wire_response, encode_fault_wire_command, FaultWireCommand, FaultWireResponse,
};
use voplay_runtime::frame_debug_capture::{
    FrameCaptureId, FrameCaptureOutcome, FrameCaptureRequest, FrameCaptureResult,
};
use voplay_runtime::frame_debug_wire::{
    decode_frame_capture_attachment_response, decode_frame_capture_result,
    encode_frame_capture_command, FrameCaptureAttachmentResponse, FrameCaptureWireCommand,
};
use voplay_runtime::hierarchy::{Hierarchy, HierarchyInspectionNode};
use voplay_runtime::inspection::{
    InspectionEditTransaction, InspectionError, InspectionSchemaRegistry, InspectionService,
    InspectionWorldPage,
};
use voplay_runtime::play_mode::{
    PlayModeConfig, PlayModeError, PlayModeExit, PlayModeExitReport, PlayModeId, PlayModeManager,
    PlayModePolicy, PlayModeResources, PlayModeStartReport,
};
use voplay_runtime::render_graph::CompiledRenderGraph;
use voplay_runtime::schedule::{Schedule, SystemSpec};
use voplay_runtime::supervisor::Role;
use voplay_runtime::world::{World, WorldConfig, WorldEntity, WorldId};
use voplay_runtime::{asset::ArtifactId, buffer_lease::BufferLease};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EditorPanel {
    Hierarchy,
    Inspector,
    Assets,
    Schedule,
    Render,
    Performance,
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

impl EditorPanel {
    pub const ALL: [Self; 6] = [
        Self::Hierarchy,
        Self::Inspector,
        Self::Assets,
        Self::Schedule,
        Self::Render,
        Self::Performance,
    ];

    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Hierarchy => "hierarchy",
            Self::Inspector => "inspector",
            Self::Assets => "assets",
            Self::Schedule => "schedule",
            Self::Render => "render",
            Self::Performance => "performance",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorPanelSchema {
    pub panel: EditorPanel,
    pub schema_type: u64,
    pub schema_version: u32,
    pub title: String,
    pub columns: Vec<String>,
}

impl EditorPanelSchema {
    pub fn canonical(panel: EditorPanel) -> Self {
        let columns = match panel {
            EditorPanel::Hierarchy => vec!["parent", "child_count", "local", "world"],
            EditorPanel::Inspector => vec![
                "stable_key",
                "world_index",
                "world_generation",
                "entity_index",
                "entity_generation",
                "component",
                "component_name",
                "type_id",
                "field",
                "field_name",
                "value",
                "value_truncated",
                "editable",
                "schema_version",
                "world_revision",
            ],
            EditorPanel::Assets => vec![
                "asset_id",
                "asset_type",
                "source_revision",
                "artifact_id",
                "state",
                "dependencies",
                "leases",
            ],
            EditorPanel::Schedule => vec!["stage", "deterministic", "before", "after"],
            EditorPanel::Render => {
                vec!["kind", "position", "node", "resource", "version", "slot"]
            }
            EditorPanel::Performance => vec![
                "tick",
                "system",
                "duration_nanos",
                "allocation_bytes",
                "lock_wait_nanos",
                "queue_items",
                "queue_bytes",
                "error_count",
                "live_roles",
                "live_entities",
                "live_surfaces",
                "input_scopes",
                "pending_render_ops",
                "pending_render_bytes",
                "device_bindings",
                "stale_device_rejections",
            ],
        };
        Self {
            panel,
            schema_type: 0x5645_0000 + panel as u64 + 1,
            schema_version: if matches!(panel, EditorPanel::Inspector | EditorPanel::Performance) {
                2
            } else {
                1
            },
            title: panel.stable_name().to_owned(),
            columns: columns.into_iter().map(str::to_owned).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorClientConfig {
    pub max_rows_per_panel: usize,
    pub max_row_bytes: usize,
    pub max_panel_bytes: usize,
    pub max_pending_requests: usize,
    pub max_request_bytes: usize,
    pub max_schema_bytes: usize,
}

impl Default for EditorClientConfig {
    fn default() -> Self {
        Self {
            max_rows_per_panel: 100_000,
            max_row_bytes: 16 * 1024,
            max_panel_bytes: 64 * 1024 * 1024,
            max_pending_requests: 1024,
            max_request_bytes: 4 * 1024 * 1024,
            max_schema_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorRow {
    pub key: String,
    pub cells: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorPanelState {
    pub panel: EditorPanel,
    pub source_revision: u64,
    pub rows: Vec<EditorRow>,
    pub next_cursor: Option<usize>,
    pub encoded_bytes: usize,
    pub dropped_before: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorControlAction {
    Pause,
    Step { count: u32 },
    Resume,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorRequestKind {
    Refresh {
        panel: EditorPanel,
        cursor: usize,
        expected_source_revision: Option<u64>,
    },
    Select {
        world: WorldId,
        expected_selection_revision: u64,
        entities: Vec<WorldEntity>,
    },
    ApplyEdit(InspectionEditTransaction),
    Undo {
        world: WorldId,
        expected_world_revision: u64,
    },
    Redo {
        world: WorldId,
        expected_world_revision: u64,
    },
    Control {
        expected_control_revision: u64,
        action: EditorControlAction,
    },
    Pick(PickRequest),
    CancelPick {
        request_id: u64,
    },
    TranslateGizmo {
        world: WorldId,
        expected_selection_revision: u64,
        expected_world_revision: u64,
        component: u32,
        fields: [u32; 3],
        offsets: [usize; 3],
        delta: [i64; 3],
    },
    StartPlayMode {
        authoring_world: WorldId,
        expected_authoring_revision: u64,
        resources: PlayModeResources,
        world_config: WorldConfig,
        policy: PlayModePolicy,
    },
    ExitPlayMode {
        id: PlayModeId,
        exit: PlayModeExit,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorRequest {
    pub request_id: u64,
    pub engine: EngineId,
    pub kind: EditorRequestKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorResponseOutcome {
    Applied { revision: u64 },
    PlayModeStarted(PlayModeStartReport),
    PlayModeExited(PlayModeExitReport),
    RevisionConflict { actual_revision: u64 },
    Rejected,
    Cancelled,
    DroppedBeforeDispatch,
    OutcomeUnknownOnRestart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorResponse {
    pub request_id: u64,
    pub engine: EngineId,
    pub outcome: EditorResponseOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorHostDispatch {
    Complete(EditorResponse),
    PickPending {
        request_id: u64,
        selection_revision: u64,
    },
    HierarchyPage {
        request_id: u64,
        cursor: usize,
        page: EditorPanelPage<HierarchyInspectionNode>,
    },
    InspectorPage {
        request_id: u64,
        cursor: usize,
        page: InspectionWorldPage,
    },
    AssetsPage {
        request_id: u64,
        cursor: usize,
        page: EditorPanelPage<AssetInspectionEntry>,
    },
    SchedulePage {
        request_id: u64,
        cursor: usize,
        page: EditorPanelPage<SystemSpec>,
    },
    RenderPage {
        request_id: u64,
        cursor: usize,
        page: EditorPanelPage<RenderPanelEntry>,
    },
    PerformancePage {
        request_id: u64,
        cursor: usize,
        page: EditorPanelPage<PerfPanelSample>,
    },
}

#[derive(Clone, Copy, Default)]
pub struct EditorPanelSources<'a> {
    pub hierarchy: Option<&'a Hierarchy>,
    pub assets: Option<&'a AssetServer>,
    pub schedule: Option<&'a Schedule>,
    pub render_graph: Option<&'a CompiledRenderGraph>,
}

pub struct EditorCommandHost {
    engine: EngineId,
    schemas: InspectionSchemaRegistry,
    inspection: InspectionService,
    tools: EditorToolService,
    panels: EditorInspectionService,
    page_limit: usize,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorCommandHostOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub selection_revision: u64,
    pub selected_entities: usize,
    pub pending_picks: usize,
    pub execution: voplay_runtime::inspection::InspectionExecutionState,
    pub control_revision: u64,
    pub pending_steps: u32,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub performance_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorCommandHostShutdownReport {
    pub tools: voplay_runtime::editor_tools::EditorToolShutdownReport,
    pub inspection: voplay_runtime::inspection::InspectionShutdownReport,
    pub panels: voplay_runtime::editor_inspection::EditorInspectionShutdownReport,
}

impl EditorCommandHost {
    pub fn new(
        engine: EngineId,
        schemas: InspectionSchemaRegistry,
        inspection: InspectionService,
        tools: EditorToolService,
        panels: EditorInspectionService,
        page_limit: usize,
    ) -> Result<Self, EditorClientError> {
        if !engine.is_valid()
            || schemas.engine() != engine
            || !schemas.is_frozen()
            || inspection.engine() != engine
            || tools.engine() != engine
            || panels.engine() != engine
            || page_limit == 0
        {
            return Err(EditorClientError::InvalidConfig);
        }
        Ok(Self {
            engine,
            schemas,
            inspection,
            tools,
            panels,
            page_limit,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn inspection(&self) -> &InspectionService {
        &self.inspection
    }

    pub fn selection(&self) -> SelectionSnapshot {
        self.tools.selection()
    }

    pub fn reconcile_selection(
        &mut self,
        world: &World,
    ) -> Result<SelectionSnapshot, EditorToolError> {
        self.tools.reconcile_selection(world)
    }

    pub fn owner_snapshot(&self) -> EditorCommandHostOwnerSnapshot {
        let selection = self.tools.selection();
        EditorCommandHostOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            selection_revision: selection.revision,
            selected_entities: selection.entities.len(),
            pending_picks: self.tools.pending_pick_count(),
            execution: self.inspection.execution_state(),
            control_revision: self.inspection.control_revision(),
            pending_steps: self.inspection.pending_steps(),
            undo_depth: self.inspection.undo_depth(),
            redo_depth: self.inspection.redo_depth(),
            performance_revision: self.panels.performance_revision(),
        }
    }

    pub fn dispatch(
        &mut self,
        world: &mut World,
        graph: Option<&CompiledRenderGraph>,
        request: &EditorRequest,
    ) -> EditorHostDispatch {
        self.dispatch_with_sources(
            world,
            EditorPanelSources {
                render_graph: graph,
                ..EditorPanelSources::default()
            },
            request,
        )
    }

    pub fn dispatch_with_sources(
        &mut self,
        world: &mut World,
        sources: EditorPanelSources<'_>,
        request: &EditorRequest,
    ) -> EditorHostDispatch {
        if self.closed {
            return self.complete(request.request_id, EditorResponseOutcome::Rejected);
        }
        if request.engine != self.engine || world.id().engine != self.engine {
            return self.complete(request.request_id, EditorResponseOutcome::Rejected);
        }
        if self.tools.reconcile_selection(world).is_err() {
            return self.complete(request.request_id, EditorResponseOutcome::Rejected);
        }
        if let EditorRequestKind::Refresh {
            panel,
            cursor,
            expected_source_revision,
        } = &request.kind
        {
            return self.refresh(
                world,
                sources,
                request.request_id,
                *panel,
                *cursor,
                *expected_source_revision,
            );
        }
        let outcome = match &request.kind {
            EditorRequestKind::Select {
                world: requested_world,
                expected_selection_revision,
                entities,
            } if *requested_world == world.id() => self
                .tools
                .replace_selection(world, entities.clone(), *expected_selection_revision)
                .map(|selection| EditorResponseOutcome::Applied {
                    revision: selection.revision,
                })
                .unwrap_or_else(|error| self.selection_error(error)),
            EditorRequestKind::ApplyEdit(transaction) => self
                .inspection
                .apply_edit_at_stage(world, &self.schemas, transaction.clone())
                .map(|result| EditorResponseOutcome::Applied {
                    revision: result.world_revision,
                })
                .unwrap_or_else(|error| self.inspection_error(error, world)),
            EditorRequestKind::Undo {
                world: requested_world,
                expected_world_revision,
            } if *requested_world == world.id() => {
                if *expected_world_revision != world.revision() {
                    EditorResponseOutcome::RevisionConflict {
                        actual_revision: world.revision(),
                    }
                } else {
                    self.inspection
                        .undo(world)
                        .map(|result| EditorResponseOutcome::Applied {
                            revision: result.world_revision,
                        })
                        .unwrap_or_else(|error| self.inspection_error(error, world))
                }
            }
            EditorRequestKind::Redo {
                world: requested_world,
                expected_world_revision,
            } if *requested_world == world.id() => {
                if *expected_world_revision != world.revision() {
                    EditorResponseOutcome::RevisionConflict {
                        actual_revision: world.revision(),
                    }
                } else {
                    self.inspection
                        .redo(world)
                        .map(|result| EditorResponseOutcome::Applied {
                            revision: result.world_revision,
                        })
                        .unwrap_or_else(|error| self.inspection_error(error, world))
                }
            }
            EditorRequestKind::Control {
                expected_control_revision,
                action,
            } => {
                let result = match action {
                    EditorControlAction::Pause => self.inspection.pause(*expected_control_revision),
                    EditorControlAction::Step { count } => self
                        .inspection
                        .request_steps(*expected_control_revision, *count),
                    EditorControlAction::Resume => {
                        self.inspection.resume(*expected_control_revision)
                    }
                };
                result
                    .map(|revision| EditorResponseOutcome::Applied { revision })
                    .unwrap_or_else(|error| self.control_error(error))
            }
            EditorRequestKind::TranslateGizmo {
                world: requested_world,
                expected_selection_revision,
                expected_world_revision,
                component,
                fields,
                offsets,
                delta,
            } if *requested_world == world.id() => {
                if *expected_selection_revision != self.tools.selection().revision {
                    EditorResponseOutcome::RevisionConflict {
                        actual_revision: self.tools.selection().revision,
                    }
                } else if *expected_world_revision != world.revision() {
                    EditorResponseOutcome::RevisionConflict {
                        actual_revision: world.revision(),
                    }
                } else {
                    self.tools
                        .build_translation_edit(
                            world,
                            request.request_id,
                            *expected_selection_revision,
                            *expected_world_revision,
                            *component,
                            *fields,
                            *offsets,
                            *delta,
                        )
                        .map_err(|error| self.tool_error(error, world))
                        .and_then(|transaction| {
                            self.inspection
                                .apply_edit_at_stage(world, &self.schemas, transaction)
                                .map(|result| EditorResponseOutcome::Applied {
                                    revision: result.world_revision,
                                })
                                .map_err(|error| self.inspection_error(error, world))
                        })
                        .unwrap_or_else(|outcome| outcome)
                }
            }
            EditorRequestKind::Pick(pick) => {
                let Some(graph) = sources.render_graph else {
                    return self.complete(request.request_id, EditorResponseOutcome::Rejected);
                };
                if pick.request_id != request.request_id {
                    return self.complete(request.request_id, EditorResponseOutcome::Rejected);
                }
                match self.tools.submit_pick(world, graph, *pick) {
                    Ok(()) => {
                        return EditorHostDispatch::PickPending {
                            request_id: request.request_id,
                            selection_revision: self.tools.selection().revision,
                        };
                    }
                    Err(error) => self.tool_error(error, world),
                }
            }
            EditorRequestKind::CancelPick {
                request_id: pick_id,
            } => self
                .tools
                .cancel_pick(*pick_id)
                .map(|_| EditorResponseOutcome::Applied {
                    revision: self.tools.selection().revision,
                })
                .unwrap_or_else(|error| self.tool_error(error, world)),
            _ => EditorResponseOutcome::Rejected,
        };
        self.complete(request.request_id, outcome)
    }

    fn refresh(
        &self,
        world: &World,
        sources: EditorPanelSources<'_>,
        request_id: u64,
        panel: EditorPanel,
        cursor: usize,
        expected_source_revision: Option<u64>,
    ) -> EditorHostDispatch {
        match panel {
            EditorPanel::Hierarchy => {
                let Some(hierarchy) = sources.hierarchy else {
                    return self.complete(request_id, EditorResponseOutcome::Rejected);
                };
                match self.panels.hierarchy_page(
                    hierarchy,
                    cursor,
                    self.page_limit,
                    expected_source_revision,
                ) {
                    Ok(page) => EditorHostDispatch::HierarchyPage {
                        request_id,
                        cursor,
                        page,
                    },
                    Err(error) => self.complete(
                        request_id,
                        self.panel_error(error, hierarchy.inspection_revision()),
                    ),
                }
            }
            EditorPanel::Inspector => {
                let selection = self.tools.selection();
                let source_revision = inspector_source_revision(
                    world.revision(),
                    selection.revision,
                    self.schemas.fingerprint().unwrap_or(0),
                );
                if expected_source_revision.is_some_and(|expected| expected != source_revision) {
                    return self.complete(
                        request_id,
                        EditorResponseOutcome::RevisionConflict {
                            actual_revision: source_revision,
                        },
                    );
                }
                match self.inspection.inspect_entities(
                    world,
                    &self.schemas,
                    &selection.entities,
                    source_revision,
                    cursor,
                    self.page_limit,
                ) {
                    Ok(page) => EditorHostDispatch::InspectorPage {
                        request_id,
                        cursor,
                        page,
                    },
                    Err(error) => self.complete(request_id, self.inspection_error(error, world)),
                }
            }
            EditorPanel::Assets => {
                let Some(assets) = sources.assets else {
                    return self.complete(request_id, EditorResponseOutcome::Rejected);
                };
                match self.panels.assets_page(
                    assets,
                    cursor,
                    self.page_limit,
                    expected_source_revision,
                ) {
                    Ok(page) => EditorHostDispatch::AssetsPage {
                        request_id,
                        cursor,
                        page,
                    },
                    Err(error) => self.complete(
                        request_id,
                        self.panel_error(error, assets.inspection_revision()),
                    ),
                }
            }
            EditorPanel::Schedule => {
                let Some(schedule) = sources.schedule else {
                    return self.complete(request_id, EditorResponseOutcome::Rejected);
                };
                match self.panels.schedule_page(
                    self.engine,
                    schedule,
                    cursor,
                    self.page_limit,
                    expected_source_revision,
                ) {
                    Ok(page) => EditorHostDispatch::SchedulePage {
                        request_id,
                        cursor,
                        page,
                    },
                    Err(error) => {
                        self.complete(request_id, self.panel_error(error, schedule.hash()))
                    }
                }
            }
            EditorPanel::Render => {
                let Some(graph) = sources.render_graph else {
                    return self.complete(request_id, EditorResponseOutcome::Rejected);
                };
                match self.panels.render_page(
                    self.engine,
                    graph,
                    cursor,
                    self.page_limit,
                    expected_source_revision,
                ) {
                    Ok(page) => EditorHostDispatch::RenderPage {
                        request_id,
                        cursor,
                        page,
                    },
                    Err(error) => {
                        self.complete(request_id, self.panel_error(error, graph.signature))
                    }
                }
            }
            EditorPanel::Performance => match self.panels.performance_page(
                cursor,
                self.page_limit,
                expected_source_revision,
            ) {
                Ok(page) => EditorHostDispatch::PerformancePage {
                    request_id,
                    cursor,
                    page,
                },
                Err(error) => self.complete(
                    request_id,
                    self.panel_error(error, self.panels.performance_revision()),
                ),
            },
        }
    }

    pub fn complete_pick(
        &mut self,
        world: &World,
        graph: &CompiledRenderGraph,
        request_id: u64,
        candidates: Vec<PickCandidate>,
    ) -> EditorResponse {
        if self.closed {
            return EditorResponse {
                request_id,
                engine: self.engine,
                outcome: EditorResponseOutcome::Rejected,
            };
        }
        let outcome = match self
            .tools
            .complete_pick(world, graph, request_id, candidates)
        {
            Ok(PickResult {
                selection_revision, ..
            }) => EditorResponseOutcome::Applied {
                revision: selection_revision,
            },
            Err(error) => {
                let _ = self.tools.cancel_pick(request_id);
                self.tool_error(error, world)
            }
        };
        EditorResponse {
            request_id,
            engine: self.engine,
            outcome,
        }
    }

    pub fn restart_endpoint(&mut self) -> Vec<u64> {
        if self.closed {
            return Vec::new();
        }
        self.tools
            .cancel_all_picks()
            .into_iter()
            .map(|request| request.request_id)
            .collect()
    }

    pub fn take_step(&mut self) -> bool {
        !self.closed && self.inspection.take_step()
    }

    pub fn shutdown(&mut self) -> EditorCommandHostShutdownReport {
        self.closed = true;
        EditorCommandHostShutdownReport {
            tools: self.tools.shutdown(),
            inspection: self.inspection.shutdown(),
            panels: self.panels.shutdown(),
        }
    }

    fn complete(&self, request_id: u64, outcome: EditorResponseOutcome) -> EditorHostDispatch {
        EditorHostDispatch::Complete(EditorResponse {
            request_id,
            engine: self.engine,
            outcome,
        })
    }

    fn inspection_error(&self, error: InspectionError, world: &World) -> EditorResponseOutcome {
        if error == InspectionError::RevisionConflict {
            EditorResponseOutcome::RevisionConflict {
                actual_revision: world.revision(),
            }
        } else {
            EditorResponseOutcome::Rejected
        }
    }

    fn tool_error(&self, error: EditorToolError, world: &World) -> EditorResponseOutcome {
        if error == EditorToolError::RevisionConflict {
            EditorResponseOutcome::RevisionConflict {
                actual_revision: world.revision(),
            }
        } else {
            EditorResponseOutcome::Rejected
        }
    }

    fn selection_error(&self, error: EditorToolError) -> EditorResponseOutcome {
        if error == EditorToolError::RevisionConflict {
            EditorResponseOutcome::RevisionConflict {
                actual_revision: self.tools.selection().revision,
            }
        } else {
            EditorResponseOutcome::Rejected
        }
    }

    fn control_error(&self, error: InspectionError) -> EditorResponseOutcome {
        if error == InspectionError::RevisionConflict {
            EditorResponseOutcome::RevisionConflict {
                actual_revision: self.inspection.control_revision(),
            }
        } else {
            EditorResponseOutcome::Rejected
        }
    }

    fn panel_error(
        &self,
        error: EditorInspectionError,
        actual_revision: u64,
    ) -> EditorResponseOutcome {
        if error == EditorInspectionError::StaleSourceRevision {
            EditorResponseOutcome::RevisionConflict { actual_revision }
        } else {
            EditorResponseOutcome::Rejected
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorFrameCaptureConfig {
    pub max_pending: usize,
    pub max_results: usize,
    pub max_result_bytes: usize,
    pub max_retained_result_bytes: usize,
    pub max_attachment_leases: usize,
    pub max_attachment_bytes: usize,
}

impl Default for EditorFrameCaptureConfig {
    fn default() -> Self {
        Self {
            max_pending: 8,
            max_results: 32,
            max_result_bytes: 64 * 1024 * 1024,
            max_retained_result_bytes: 256 * 1024 * 1024,
            max_attachment_leases: 4096,
            max_attachment_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorFrameCaptureDispatch {
    pub id: FrameCaptureId,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorFrameCaptureOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub closed: bool,
    pub pending_captures: usize,
    pub retained_results: usize,
    pub retained_result_bytes: usize,
    pub pending_attachment_requests: usize,
    pub live_attachment_leases: usize,
    pub retained_attachments: usize,
    pub retained_attachment_bytes: usize,
    pub next_request: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorFrameCaptureShutdownReport {
    pub before: EditorFrameCaptureOwnerSnapshot,
    pub cancelled_captures: Vec<FrameCaptureId>,
    pub released_attachment_leases: Vec<BufferLease>,
    pub after: EditorFrameCaptureOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorFaultInjectionDispatch {
    pub role: Role,
    pub payload: Vec<u8>,
}

pub struct EditorFaultInjectionClient {
    engine: EngineId,
    pending: BTreeMap<Role, u16>,
    responses: BTreeMap<Role, FaultWireResponse>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorFaultInjectionOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub pending_roles: usize,
    pub response_roles: usize,
    pub retained_trace_events: usize,
    pub latest_trace_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorFaultInjectionShutdownReport {
    pub before: EditorFaultInjectionOwnerSnapshot,
    pub after: EditorFaultInjectionOwnerSnapshot,
}

impl EditorFaultInjectionClient {
    pub fn new(engine: EngineId) -> Result<Self, EditorClientError> {
        if !engine.is_valid() {
            return Err(EditorClientError::InvalidConfig);
        }
        Ok(Self {
            engine,
            pending: BTreeMap::new(),
            responses: BTreeMap::new(),
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn owner_snapshot(&self) -> EditorFaultInjectionOwnerSnapshot {
        EditorFaultInjectionOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            pending_roles: self.pending.len(),
            response_roles: self.responses.len(),
            retained_trace_events: self
                .responses
                .values()
                .map(|response| response.trace.len())
                .sum(),
            latest_trace_sequence: self
                .responses
                .values()
                .flat_map(|response| response.trace.last())
                .map(|event| event.sequence)
                .max()
                .unwrap_or(0),
        }
    }

    pub fn install(
        &mut self,
        role: Role,
        rule: FaultRule,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.command(role, FaultWireCommand::Install(rule))
    }

    pub fn remove(
        &mut self,
        role: Role,
        point: voplay_runtime::fault_injection::FaultPoint,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.command(role, FaultWireCommand::Remove(point))
    }

    pub fn clear(&mut self, role: Role) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.command(role, FaultWireCommand::Clear)
    }

    pub fn request_metrics(
        &mut self,
        role: Role,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.command(role, FaultWireCommand::Metrics)
    }

    pub fn request_trace(
        &mut self,
        role: Role,
        after_sequence: u64,
        limit: u32,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        if limit == 0 || limit > 4096 {
            return Err(EditorClientError::RequestCapacity);
        }
        self.command(
            role,
            FaultWireCommand::Trace {
                after_sequence,
                limit,
            },
        )
    }

    pub fn clear_trace(
        &mut self,
        role: Role,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.command(role, FaultWireCommand::ClearTrace)
    }

    pub fn ingest(
        &mut self,
        role: Role,
        payload: &[u8],
    ) -> Result<FaultWireResponse, EditorClientError> {
        self.ensure_open()?;
        let response =
            decode_fault_wire_response(payload).map_err(|_| EditorClientError::ResponseMismatch)?;
        if self.pending.get(&role).copied() != Some(response.action) {
            return Err(EditorClientError::ResponseMismatch);
        }
        self.pending.remove(&role);
        self.responses.insert(role, response.clone());
        Ok(response)
    }

    pub fn metrics(&self, role: Role) -> Option<FaultInjectionMetrics> {
        self.responses.get(&role).map(|response| response.metrics)
    }

    pub fn trace(&self, role: Role) -> Option<&[FaultTraceEvent]> {
        self.responses
            .get(&role)
            .map(|response| response.trace.as_slice())
    }

    pub fn restart_endpoint(&mut self, role: Role) -> bool {
        if self.closed {
            return false;
        }
        self.pending.remove(&role).is_some()
    }

    pub fn shutdown(&mut self) -> EditorFaultInjectionShutdownReport {
        let before = self.owner_snapshot();
        if !self.closed {
            self.pending.clear();
            self.responses.clear();
            self.closed = true;
        }
        EditorFaultInjectionShutdownReport {
            before,
            after: self.owner_snapshot(),
        }
    }

    fn command(
        &mut self,
        role: Role,
        command: FaultWireCommand,
    ) -> Result<EditorFaultInjectionDispatch, EditorClientError> {
        self.ensure_open()?;
        let action = match command {
            FaultWireCommand::Install(_) => 1,
            FaultWireCommand::Remove(_) => 2,
            FaultWireCommand::Clear => 3,
            FaultWireCommand::Metrics => 4,
            FaultWireCommand::Trace { .. } => 5,
            FaultWireCommand::ClearTrace => 6,
        };
        if self.pending.contains_key(&role) {
            return Err(EditorClientError::RequestAlreadyDispatched);
        }
        self.pending.insert(role, action);
        Ok(EditorFaultInjectionDispatch {
            role,
            payload: encode_fault_wire_command(command),
        })
    }

    fn ensure_open(&self) -> Result<(), EditorClientError> {
        if self.closed {
            Err(EditorClientError::Closed)
        } else {
            Ok(())
        }
    }
}

pub struct EditorFrameCaptureClient {
    engine: EngineId,
    endpoint_generation: Handle,
    config: EditorFrameCaptureConfig,
    next_request: u64,
    pending: BTreeMap<FrameCaptureId, FrameCaptureRequest>,
    results: VecDeque<FrameCaptureResult>,
    result_sizes: BTreeMap<FrameCaptureId, usize>,
    retained_result_bytes: usize,
    pending_attachment_requests: BTreeMap<u64, BufferLease>,
    live_attachment_leases: BTreeMap<Handle, BufferLease>,
    attachment_bytes: BTreeMap<ArtifactId, Vec<u8>>,
    retained_attachment_bytes: usize,
    closed: bool,
}

impl EditorFrameCaptureClient {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: EditorFrameCaptureConfig,
    ) -> Result<Self, EditorClientError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_pending == 0
            || config.max_results == 0
            || config.max_result_bytes == 0
            || config.max_retained_result_bytes < config.max_result_bytes
            || config.max_attachment_leases == 0
            || config.max_attachment_bytes == 0
        {
            return Err(EditorClientError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            next_request: 1,
            pending: BTreeMap::new(),
            results: VecDeque::new(),
            result_sizes: BTreeMap::new(),
            retained_result_bytes: 0,
            pending_attachment_requests: BTreeMap::new(),
            live_attachment_leases: BTreeMap::new(),
            attachment_bytes: BTreeMap::new(),
            retained_attachment_bytes: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> EditorFrameCaptureOwnerSnapshot {
        EditorFrameCaptureOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            closed: self.closed,
            pending_captures: self.pending.len(),
            retained_results: self.results.len(),
            retained_result_bytes: self.retained_result_bytes,
            pending_attachment_requests: self.pending_attachment_requests.len(),
            live_attachment_leases: self.live_attachment_leases.len(),
            retained_attachments: self.attachment_bytes.len(),
            retained_attachment_bytes: self.retained_attachment_bytes,
            next_request: self.next_request,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &mut self,
        frame_id: u64,
        graph_signature: u64,
        include_attachments: bool,
        include_shader_diagnostics: bool,
        deadline_millis: u64,
        max_bytes: usize,
    ) -> Result<EditorFrameCaptureDispatch, EditorClientError> {
        self.ensure_open()?;
        if frame_id == 0
            || graph_signature == 0
            || deadline_millis == 0
            || max_bytes == 0
            || max_bytes > self.config.max_result_bytes
            || self.pending.len() == self.config.max_pending
        {
            return Err(EditorClientError::RequestCapacity);
        }
        let id = FrameCaptureId(self.next_request);
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(EditorClientError::RequestIdExhausted)?;
        let request = FrameCaptureRequest {
            id,
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            frame_id,
            expected_graph_signature: graph_signature,
            include_attachments,
            include_shader_diagnostics,
            deadline_millis,
            max_bytes,
        };
        let payload = encode_frame_capture_command(FrameCaptureWireCommand::Submit(request))
            .map_err(|_| EditorClientError::RequestByteCapacity)?;
        self.pending.insert(id, request);
        Ok(EditorFrameCaptureDispatch { id, payload })
    }

    pub fn cancel(
        &mut self,
        id: FrameCaptureId,
    ) -> Result<EditorFrameCaptureDispatch, EditorClientError> {
        self.ensure_open()?;
        if !self.pending.contains_key(&id) {
            return Err(EditorClientError::UnknownRequest);
        }
        let payload = encode_frame_capture_command(FrameCaptureWireCommand::Cancel(id))
            .map_err(|_| EditorClientError::RequestByteCapacity)?;
        Ok(EditorFrameCaptureDispatch { id, payload })
    }

    pub fn ingest(&mut self, bytes: &[u8]) -> Result<FrameCaptureId, EditorClientError> {
        self.ensure_open()?;
        if bytes.len() > self.config.max_result_bytes {
            return Err(EditorClientError::RequestByteCapacity);
        }
        let result = decode_frame_capture_result(self.engine, bytes)
            .map_err(|_| EditorClientError::ResponseMismatch)?;
        let id = result.request.id;
        if result.request.endpoint_generation != self.endpoint_generation
            || self.pending.get(&id) != Some(&result.request)
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        let mut live_attachment_leases = self.live_attachment_leases.clone();
        for lease in result.attachments.iter().map(|attachment| attachment.lease) {
            if live_attachment_leases
                .get(&lease.handle)
                .is_some_and(|current| *current != lease)
            {
                return Err(EditorClientError::ResponseMismatch);
            }
            live_attachment_leases.insert(lease.handle, lease);
        }
        if live_attachment_leases.len() > self.config.max_attachment_leases {
            return Err(EditorClientError::RequestCapacity);
        }
        self.pending.remove(&id);
        self.retain_result(result, bytes.len())?;
        self.live_attachment_leases = live_attachment_leases;
        Ok(id)
    }

    pub fn result(&self, id: FrameCaptureId) -> Option<&FrameCaptureResult> {
        self.results.iter().find(|result| result.request.id == id)
    }

    pub fn latest(&self) -> Option<&FrameCaptureResult> {
        self.results.back()
    }

    pub fn discard_result(
        &mut self,
        id: FrameCaptureId,
    ) -> Result<FrameCaptureResult, EditorClientError> {
        self.ensure_open()?;
        let index = self
            .results
            .iter()
            .position(|result| result.request.id == id)
            .ok_or(EditorClientError::UnknownRequest)?;
        let result = self
            .results
            .remove(index)
            .ok_or(EditorClientError::UnknownRequest)?;
        let bytes = self.result_sizes.remove(&id).unwrap_or(0);
        self.retained_result_bytes = self.retained_result_bytes.saturating_sub(bytes);
        self.prune_attachment_bytes();
        Ok(result)
    }

    pub fn rebind_endpoint(
        &mut self,
        endpoint_generation: Handle,
    ) -> Result<Vec<FrameCaptureId>, EditorClientError> {
        self.ensure_open()?;
        if !endpoint_generation.is_valid()
            || endpoint_generation.index != self.endpoint_generation.index
            || endpoint_generation.generation <= self.endpoint_generation.generation
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        let pending = std::mem::take(&mut self.pending);
        let ids = pending.keys().copied().collect::<Vec<_>>();
        for request in pending.into_values() {
            self.retain_result(
                FrameCaptureResult {
                    request,
                    graph_signature: request.expected_graph_signature,
                    outcome: FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart,
                    nodes: Vec::new(),
                    attachments: Vec::new(),
                    shader_diagnostics: Vec::new(),
                    diagnostic: "render endpoint restarted after capture dispatch".to_owned(),
                },
                128,
            )?;
        }
        self.pending_attachment_requests.clear();
        self.live_attachment_leases.clear();
        self.attachment_bytes.clear();
        self.retained_attachment_bytes = 0;
        self.endpoint_generation = endpoint_generation;
        Ok(ids)
    }

    pub fn build_view(&self, id: FrameCaptureId) -> Result<ViewNode, EditorClientError> {
        let capture = self.result(id).ok_or(EditorClientError::UnknownRequest)?;
        let mut props = BTreeMap::new();
        props.insert("data-capture-id".to_owned(), id.0.to_string());
        props.insert(
            "data-frame-id".to_owned(),
            capture.request.frame_id.to_string(),
        );
        props.insert(
            "data-graph-signature".to_owned(),
            capture.graph_signature.to_string(),
        );
        props.insert(
            "data-outcome".to_owned(),
            frame_capture_outcome_name(capture.outcome).to_owned(),
        );
        props.insert("data-diagnostic".to_owned(), capture.diagnostic.clone());
        let mut children = Vec::new();
        for node in &capture.nodes {
            let mut node_props = BTreeMap::new();
            node_props.insert("data-kind".to_owned(), "node".to_owned());
            node_props.insert("data-label".to_owned(), node.label.clone());
            node_props.insert("data-queue".to_owned(), node.queue.to_string());
            node_props.insert(
                "data-duration-nanos".to_owned(),
                node.finished_nanos
                    .saturating_sub(node.started_nanos)
                    .to_string(),
            );
            node_props.insert("data-draw-calls".to_owned(), node.draw_calls.to_string());
            node_props.insert(
                "data-dispatch-calls".to_owned(),
                node.dispatch_calls.to_string(),
            );
            children.push(ViewNode {
                key: Some(format!("node-{}", node.node.0)),
                kind: ViewKind::Element("voplay-frame-capture-node".to_owned()),
                props: node_props,
                children: Vec::new(),
            });
        }
        for attachment in &capture.attachments {
            let mut attachment_props = BTreeMap::new();
            attachment_props.insert("data-kind".to_owned(), "attachment".to_owned());
            attachment_props.insert("data-width".to_owned(), attachment.width.to_string());
            attachment_props.insert("data-height".to_owned(), attachment.height.to_string());
            attachment_props.insert("data-format".to_owned(), attachment.format.to_string());
            attachment_props.insert(
                "data-row-bytes".to_owned(),
                attachment.row_bytes.to_string(),
            );
            attachment_props.insert(
                "data-lease-bytes".to_owned(),
                attachment.lease.len.to_string(),
            );
            let loaded_bytes = self
                .attachment_bytes
                .get(&attachment.lease.artifact_id)
                .map_or(0, Vec::len);
            attachment_props.insert("data-loaded-bytes".to_owned(), loaded_bytes.to_string());
            attachment_props.insert(
                "data-ready".to_owned(),
                (loaded_bytes == attachment.lease.len).to_string(),
            );
            children.push(ViewNode {
                key: Some(format!(
                    "attachment-{}-{}",
                    attachment.resource.0, attachment.version
                )),
                kind: ViewKind::Element("voplay-frame-capture-attachment".to_owned()),
                props: attachment_props,
                children: Vec::new(),
            });
        }
        for (index, diagnostic) in capture.shader_diagnostics.iter().enumerate() {
            let mut diagnostic_props = BTreeMap::new();
            diagnostic_props.insert("data-kind".to_owned(), "shader-diagnostic".to_owned());
            diagnostic_props.insert("data-severity".to_owned(), diagnostic.severity.to_string());
            diagnostic_props.insert("data-message".to_owned(), diagnostic.message.clone());
            diagnostic_props.insert(
                "data-source-start".to_owned(),
                diagnostic.source_start.to_string(),
            );
            diagnostic_props.insert(
                "data-source-end".to_owned(),
                diagnostic.source_end.to_string(),
            );
            children.push(ViewNode {
                key: Some(format!("diagnostic-{index}")),
                kind: ViewKind::Element("voplay-frame-capture-diagnostic".to_owned()),
                props: diagnostic_props,
                children: Vec::new(),
            });
        }
        Ok(ViewNode {
            key: Some(format!("frame-capture-{}", id.0)),
            kind: ViewKind::Element("voplay-frame-capture".to_owned()),
            props,
            children,
        })
    }

    pub fn read_attachment(
        &mut self,
        lease: BufferLease,
        max_chunk_bytes: usize,
        now_millis: u64,
    ) -> Result<Option<EditorFrameCaptureDispatch>, EditorClientError> {
        self.ensure_open()?;
        if lease.engine != self.engine
            || lease.provider_generation != self.endpoint_generation
            || lease.consumer != self.endpoint_generation
            || max_chunk_bytes == 0
            || now_millis > lease.deadline_millis
            || lease.len > self.config.max_attachment_bytes
            || !self.known_attachment_lease(lease)
            || self.pending_attachment_requests.len() >= self.config.max_pending
            || self
                .pending_attachment_requests
                .values()
                .any(|pending| *pending == lease)
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        let offset = self
            .attachment_bytes
            .get(&lease.artifact_id)
            .map_or(0, Vec::len);
        if offset > lease.len
            || self
                .retained_attachment_bytes
                .saturating_sub(offset)
                .checked_add(lease.len)
                .is_none_or(|bytes| bytes > self.config.max_attachment_bytes)
        {
            return Err(EditorClientError::RequestByteCapacity);
        }
        if offset == lease.len {
            return Ok(None);
        }
        let len = max_chunk_bytes.min(lease.len - offset);
        let request = self.next_request;
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(EditorClientError::RequestIdExhausted)?;
        let payload = encode_frame_capture_command(FrameCaptureWireCommand::ReadAttachment {
            request,
            lease,
            offset,
            len,
            now_millis,
        })
        .map_err(|_| EditorClientError::RequestByteCapacity)?;
        self.pending_attachment_requests.insert(request, lease);
        Ok(Some(EditorFrameCaptureDispatch {
            id: FrameCaptureId(request),
            payload,
        }))
    }

    pub fn release_attachment(
        &mut self,
        lease: BufferLease,
    ) -> Result<EditorFrameCaptureDispatch, EditorClientError> {
        self.ensure_open()?;
        if lease.engine != self.engine
            || lease.provider_generation != self.endpoint_generation
            || lease.consumer != self.endpoint_generation
            || !self.known_attachment_lease(lease)
            || self.pending_attachment_requests.len() >= self.config.max_pending
            || self
                .pending_attachment_requests
                .values()
                .any(|pending| *pending == lease)
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        let request = self.next_request;
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(EditorClientError::RequestIdExhausted)?;
        let payload = encode_frame_capture_command(FrameCaptureWireCommand::ReleaseAttachment {
            request,
            lease,
        })
        .map_err(|_| EditorClientError::RequestByteCapacity)?;
        self.pending_attachment_requests.insert(request, lease);
        Ok(EditorFrameCaptureDispatch {
            id: FrameCaptureId(request),
            payload,
        })
    }

    pub fn ingest_attachment_response(&mut self, bytes: &[u8]) -> Result<u64, EditorClientError> {
        self.ensure_open()?;
        let response = decode_frame_capture_attachment_response(self.engine, bytes)
            .map_err(|_| EditorClientError::ResponseMismatch)?;
        let (request, lease) = match &response {
            FrameCaptureAttachmentResponse::Chunk { request, lease, .. }
            | FrameCaptureAttachmentResponse::Released { request, lease } => (*request, *lease),
        };
        if self.pending_attachment_requests.get(&request) != Some(&lease) {
            return Err(EditorClientError::ResponseMismatch);
        }
        match &response {
            FrameCaptureAttachmentResponse::Chunk {
                lease,
                offset,
                bytes,
                ..
            } => {
                let current = self
                    .attachment_bytes
                    .get(&lease.artifact_id)
                    .map_or(0, Vec::len);
                if bytes.is_empty()
                    || current != *offset
                    || current
                        .checked_add(bytes.len())
                        .is_none_or(|len| len > lease.len)
                    || self
                        .retained_attachment_bytes
                        .checked_add(bytes.len())
                        .is_none_or(|len| len > self.config.max_attachment_bytes)
                {
                    return Err(EditorClientError::ResponseMismatch);
                }
            }
            FrameCaptureAttachmentResponse::Released { .. } => {}
        }
        self.pending_attachment_requests.remove(&request);
        match response {
            FrameCaptureAttachmentResponse::Chunk { lease, bytes, .. } => {
                self.retained_attachment_bytes += bytes.len();
                self.attachment_bytes
                    .entry(lease.artifact_id)
                    .or_default()
                    .extend_from_slice(&bytes);
            }
            FrameCaptureAttachmentResponse::Released { lease, .. } => {
                if self.live_attachment_leases.get(&lease.handle) == Some(&lease) {
                    self.live_attachment_leases.remove(&lease.handle);
                }
                if self
                    .attachment_bytes
                    .get(&lease.artifact_id)
                    .is_some_and(|bytes| bytes.len() != lease.len)
                    && !self
                        .live_attachment_leases
                        .values()
                        .any(|live| live.artifact_id == lease.artifact_id)
                {
                    let removed = self
                        .attachment_bytes
                        .remove(&lease.artifact_id)
                        .map_or(0, |bytes| bytes.len());
                    self.retained_attachment_bytes =
                        self.retained_attachment_bytes.saturating_sub(removed);
                }
            }
        }
        self.prune_attachment_bytes();
        Ok(request)
    }

    pub fn attachment_bytes(&self, artifact: ArtifactId) -> Option<&[u8]> {
        self.attachment_bytes.get(&artifact).map(Vec::as_slice)
    }

    pub fn shutdown(&mut self) -> EditorFrameCaptureShutdownReport {
        let before = self.owner_snapshot();
        if self.closed {
            return EditorFrameCaptureShutdownReport {
                before,
                cancelled_captures: Vec::new(),
                released_attachment_leases: Vec::new(),
                after: before,
            };
        }
        let cancelled_captures = self.pending.keys().copied().collect();
        let released_attachment_leases = self.live_attachment_leases.values().copied().collect();
        self.pending.clear();
        self.results.clear();
        self.result_sizes.clear();
        self.retained_result_bytes = 0;
        self.pending_attachment_requests.clear();
        self.live_attachment_leases.clear();
        self.attachment_bytes.clear();
        self.retained_attachment_bytes = 0;
        self.closed = true;
        EditorFrameCaptureShutdownReport {
            before,
            cancelled_captures,
            released_attachment_leases,
            after: self.owner_snapshot(),
        }
    }

    fn retain_result(
        &mut self,
        result: FrameCaptureResult,
        encoded_bytes: usize,
    ) -> Result<(), EditorClientError> {
        if encoded_bytes > self.config.max_result_bytes {
            return Err(EditorClientError::RequestByteCapacity);
        }
        while self.results.len() >= self.config.max_results
            || self
                .retained_result_bytes
                .checked_add(encoded_bytes)
                .is_none_or(|bytes| bytes > self.config.max_retained_result_bytes)
        {
            let removed = self
                .results
                .pop_front()
                .ok_or(EditorClientError::RequestByteCapacity)?;
            let bytes = self.result_sizes.remove(&removed.request.id).unwrap_or(0);
            self.retained_result_bytes = self.retained_result_bytes.saturating_sub(bytes);
        }
        let id = result.request.id;
        self.results.push_back(result);
        self.result_sizes.insert(id, encoded_bytes);
        self.retained_result_bytes += encoded_bytes;
        self.prune_attachment_bytes();
        Ok(())
    }

    fn prune_attachment_bytes(&mut self) {
        let mut retained = self
            .results
            .iter()
            .flat_map(|result| {
                result
                    .attachments
                    .iter()
                    .map(|attachment| attachment.lease.artifact_id)
            })
            .collect::<BTreeSet<_>>();
        retained.extend(
            self.pending_attachment_requests
                .values()
                .map(|lease| lease.artifact_id),
        );
        self.attachment_bytes
            .retain(|artifact, _| retained.contains(artifact));
        self.retained_attachment_bytes = self.attachment_bytes.values().map(Vec::len).sum();
    }

    fn known_attachment_lease(&self, lease: BufferLease) -> bool {
        self.live_attachment_leases.get(&lease.handle) == Some(&lease)
    }

    fn ensure_open(&self) -> Result<(), EditorClientError> {
        if self.closed {
            Err(EditorClientError::Closed)
        } else {
            Ok(())
        }
    }
}

const fn frame_capture_outcome_name(outcome: FrameCaptureOutcome) -> &'static str {
    match outcome {
        FrameCaptureOutcome::Completed => "completed",
        FrameCaptureOutcome::Cancelled => "cancelled",
        FrameCaptureOutcome::DeadlineExceeded => "deadline-exceeded",
        FrameCaptureOutcome::GraphChanged => "graph-changed",
        FrameCaptureOutcome::EndpointRestartedBeforeCapture => "endpoint-restarted-before-capture",
        FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart => {
            "outcome-unknown-on-endpoint-restart"
        }
        FrameCaptureOutcome::Failed => "failed",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorPlayModeHostError {
    WrongEngine,
    UnsupportedRequest,
    MalformedWire,
    WireCapacity,
}

pub struct EditorPlayModeHost {
    engine: EngineId,
    play_modes: PlayModeManager,
}

impl EditorPlayModeHost {
    pub fn new(engine: EngineId, config: PlayModeConfig) -> Result<Self, PlayModeError> {
        Ok(Self {
            engine,
            play_modes: PlayModeManager::new(engine, config)?,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn live_sessions(&self) -> usize {
        self.play_modes.live_sessions()
    }

    pub fn owner_snapshot(&self) -> voplay_runtime::play_mode::PlayModeOwnerSnapshot {
        self.play_modes.owner_snapshot()
    }

    pub fn play_world(&self, id: PlayModeId) -> Result<&World, PlayModeError> {
        self.play_modes.play_world(id)
    }

    pub fn play_world_mut(&mut self, id: PlayModeId) -> Result<&mut World, PlayModeError> {
        self.play_modes.play_world_mut(id)
    }

    pub fn dispatch(
        &mut self,
        authoring: &mut World,
        request: &EditorRequest,
    ) -> Result<EditorResponse, EditorPlayModeHostError> {
        if request.engine != self.engine || authoring.id().engine != self.engine {
            return Err(EditorPlayModeHostError::WrongEngine);
        }
        let outcome = match &request.kind {
            EditorRequestKind::StartPlayMode {
                authoring_world,
                expected_authoring_revision,
                resources,
                world_config,
                policy,
            } => {
                if *authoring_world != authoring.id() {
                    return Err(EditorPlayModeHostError::WrongEngine);
                }
                if *expected_authoring_revision != authoring.revision() {
                    EditorResponseOutcome::RevisionConflict {
                        actual_revision: authoring.revision(),
                    }
                } else {
                    match self.play_modes.start(
                        authoring,
                        *resources,
                        *world_config,
                        policy.clone(),
                    ) {
                        Ok(report) => EditorResponseOutcome::PlayModeStarted(report),
                        Err(PlayModeError::RevisionConflict) => {
                            EditorResponseOutcome::RevisionConflict {
                                actual_revision: authoring.revision(),
                            }
                        }
                        Err(_) => EditorResponseOutcome::Rejected,
                    }
                }
            }
            EditorRequestKind::ExitPlayMode { id, exit } => {
                match self.play_modes.exit(*id, authoring, *exit) {
                    Ok(report) => EditorResponseOutcome::PlayModeExited(report),
                    Err(PlayModeError::RevisionConflict) => {
                        EditorResponseOutcome::RevisionConflict {
                            actual_revision: authoring.revision(),
                        }
                    }
                    Err(_) => EditorResponseOutcome::Rejected,
                }
            }
            _ => return Err(EditorPlayModeHostError::UnsupportedRequest),
        };
        Ok(EditorResponse {
            request_id: request.request_id,
            engine: self.engine,
            outcome,
        })
    }

    pub fn dispatch_wire(
        &mut self,
        authoring: &mut World,
        request_bytes: &[u8],
        max_policy_fields: usize,
        max_response_bytes: usize,
    ) -> Result<Vec<u8>, EditorPlayModeHostError> {
        let request = decode_play_mode_request(self.engine, request_bytes, max_policy_fields)
            .map_err(map_play_mode_wire_error)?;
        let response = self.dispatch(authoring, &request)?;
        encode_play_mode_response(response, max_response_bytes).map_err(map_play_mode_wire_error)
    }

    pub fn shutdown(&mut self) -> Result<Vec<PlayModeExitReport>, PlayModeError> {
        Ok(self.play_modes.shutdown()?.released_sessions)
    }
}

fn map_play_mode_wire_error(error: EditorPlayModeWireError) -> EditorPlayModeHostError {
    match error {
        EditorPlayModeWireError::Capacity => EditorPlayModeHostError::WireCapacity,
        EditorPlayModeWireError::Unsupported => EditorPlayModeHostError::UnsupportedRequest,
        EditorPlayModeWireError::Malformed | EditorPlayModeWireError::WrongEngine => {
            EditorPlayModeHostError::MalformedWire
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorClientError {
    InvalidConfig,
    WrongEngine,
    RegistryFrozen,
    RegistryNotFrozen,
    DuplicateSchema,
    MissingSchema,
    InvalidSchema,
    SchemaByteCapacity,
    WrongPanel,
    StalePage,
    InvalidCursor,
    RowCapacity,
    RowByteCapacity,
    PanelByteCapacity,
    RowSchemaMismatch,
    RequestCapacity,
    RequestByteCapacity,
    RequestIdExhausted,
    UnknownRequest,
    RequestNotDispatched,
    RequestAlreadyDispatched,
    ResponseMismatch,
    DuplicatePlayMode,
    UnknownPlayMode,
    Closed,
}

struct PendingRequest {
    request: EditorRequest,
    encoded_bytes: usize,
    dispatched: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorClientOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub schemas: usize,
    pub schema_bytes: usize,
    pub schemas_frozen: bool,
    pub schema_fingerprint: u64,
    pub panels: usize,
    pub panel_rows: usize,
    pub pending_requests: usize,
    pub dispatched_requests: usize,
    pub pending_request_bytes: usize,
    pub play_modes: usize,
    pub next_request_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorClientShutdownReport {
    pub before: EditorClientOwnerSnapshot,
    pub terminal_requests: Vec<EditorResponse>,
    pub after: EditorClientOwnerSnapshot,
}

pub struct EditorClient {
    engine: EngineId,
    config: EditorClientConfig,
    schemas: BTreeMap<EditorPanel, EditorPanelSchema>,
    schema_bytes: usize,
    frozen: bool,
    schema_fingerprint: u64,
    panels: BTreeMap<EditorPanel, EditorPanelState>,
    panel_applied_requests: BTreeMap<EditorPanel, u64>,
    next_request_id: u64,
    pending_request_bytes: usize,
    pending: BTreeMap<u64, PendingRequest>,
    request_order: VecDeque<u64>,
    play_modes: BTreeMap<PlayModeId, PlayModeStartReport>,
    closed: bool,
}

impl EditorClient {
    pub fn new(engine: EngineId, config: EditorClientConfig) -> Result<Self, EditorClientError> {
        if !engine.is_valid() {
            return Err(EditorClientError::WrongEngine);
        }
        if config.max_rows_per_panel == 0
            || config.max_row_bytes == 0
            || config.max_panel_bytes == 0
            || config.max_pending_requests == 0
            || config.max_request_bytes == 0
            || config.max_schema_bytes == 0
        {
            return Err(EditorClientError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            schemas: BTreeMap::new(),
            schema_bytes: 0,
            frozen: false,
            schema_fingerprint: 0,
            panels: BTreeMap::new(),
            panel_applied_requests: BTreeMap::new(),
            next_request_id: 1,
            pending_request_bytes: 0,
            pending: BTreeMap::new(),
            request_order: VecDeque::new(),
            play_modes: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn register_schema(&mut self, schema: EditorPanelSchema) -> Result<(), EditorClientError> {
        self.ensure_open()?;
        if self.frozen {
            return Err(EditorClientError::RegistryFrozen);
        }
        if schema.schema_type == 0
            || schema.schema_version == 0
            || schema.title.is_empty()
            || schema.columns.is_empty()
            || schema.columns.iter().any(String::is_empty)
            || schema.columns.iter().collect::<BTreeSet<_>>().len() != schema.columns.len()
        {
            return Err(EditorClientError::InvalidSchema);
        }
        if self.schemas.contains_key(&schema.panel) {
            return Err(EditorClientError::DuplicateSchema);
        }
        let initial_bytes = 32_usize
            .checked_add(schema.title.len())
            .ok_or(EditorClientError::SchemaByteCapacity)?;
        let bytes = schema
            .columns
            .iter()
            .try_fold(initial_bytes, |bytes, column| {
                bytes.checked_add(4)?.checked_add(column.len())
            });
        let next_bytes = bytes
            .and_then(|bytes| self.schema_bytes.checked_add(bytes))
            .filter(|bytes| *bytes <= self.config.max_schema_bytes)
            .ok_or(EditorClientError::SchemaByteCapacity)?;
        self.schemas.insert(schema.panel, schema);
        self.schema_bytes = next_bytes;
        Ok(())
    }

    pub fn freeze_schemas(&mut self) -> Result<u64, EditorClientError> {
        self.ensure_open()?;
        if self.frozen {
            return Ok(self.schema_fingerprint);
        }
        if EditorPanel::ALL
            .iter()
            .any(|panel| !self.schemas.contains_key(panel))
        {
            return Err(EditorClientError::MissingSchema);
        }
        let mut hash = 0xcbf29ce484222325_u64;
        for (panel, schema) in &self.schemas {
            hash_bytes(&mut hash, panel.stable_name().as_bytes());
            hash_bytes(&mut hash, &schema.schema_type.to_le_bytes());
            hash_bytes(&mut hash, &schema.schema_version.to_le_bytes());
            hash_bytes(&mut hash, schema.title.as_bytes());
            for column in &schema.columns {
                hash_bytes(&mut hash, column.as_bytes());
            }
        }
        self.schema_fingerprint = hash;
        self.frozen = true;
        Ok(hash)
    }

    pub fn panel(&self, panel: EditorPanel) -> Option<&EditorPanelState> {
        self.panels.get(&panel)
    }

    pub fn build_inspector_edit(
        &self,
        transaction_id: u64,
        row_key: &str,
        value_hex: &str,
    ) -> Result<InspectionEditTransaction, EditorClientError> {
        self.build_inspector_edits(transaction_id, [(row_key, value_hex)])
    }

    pub fn build_inspector_edits<'a>(
        &self,
        transaction_id: u64,
        changes: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<InspectionEditTransaction, EditorClientError> {
        if transaction_id == 0 {
            return Err(EditorClientError::ResponseMismatch);
        }
        let panel = self
            .panels
            .get(&EditorPanel::Inspector)
            .ok_or(EditorClientError::UnknownRequest)?;
        let mut edits = Vec::new();
        let mut expected_world = None;
        let mut expected_world_revision = None;
        let mut identities = BTreeSet::new();
        let mut decoded_bytes = 0_usize;
        for (row_key, value_hex) in changes {
            if edits.len() == self.config.max_rows_per_panel {
                return Err(EditorClientError::RequestCapacity);
            }
            let row = panel
                .rows
                .iter()
                .find(|row| row.key == row_key)
                .ok_or(EditorClientError::UnknownRequest)?;
            if row.cells.get("editable").map(String::as_str) != Some("true")
                || row.cells.get("value_truncated").map(String::as_str) != Some("false")
            {
                return Err(EditorClientError::ResponseMismatch);
            }
            let parse_u32 = |name: &str| {
                row.cells
                    .get(name)
                    .and_then(|value| value.parse::<u32>().ok())
                    .ok_or(EditorClientError::ResponseMismatch)
            };
            let parse_u64 = |name: &str| {
                row.cells
                    .get(name)
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or(EditorClientError::ResponseMismatch)
            };
            let value = decode_hex(value_hex)?;
            let current = row
                .cells
                .get("value")
                .ok_or(EditorClientError::ResponseMismatch)?;
            if value.len().checked_mul(2) != Some(current.len()) {
                return Err(EditorClientError::ResponseMismatch);
            }
            decoded_bytes = decoded_bytes
                .checked_add(value.len())
                .filter(|bytes| *bytes <= self.config.max_request_bytes)
                .ok_or(EditorClientError::RequestByteCapacity)?;
            let world = WorldId {
                engine: self.engine,
                handle: Handle {
                    index: parse_u32("world_index")?,
                    generation: parse_u32("world_generation")?,
                },
            };
            let entity = WorldEntity {
                world,
                entity: Handle {
                    index: parse_u32("entity_index")?,
                    generation: parse_u32("entity_generation")?,
                },
            };
            let component = parse_u32("component")?;
            let field = parse_u32("field")?;
            let world_revision = parse_u64("world_revision")?;
            if !world.is_valid() || !entity.entity.is_valid() || component == 0 || field == 0 {
                return Err(EditorClientError::ResponseMismatch);
            }
            if expected_world.is_some_and(|expected| expected != world)
                || expected_world_revision.is_some_and(|expected| expected != world_revision)
                || !identities.insert((entity, component, field))
            {
                return Err(EditorClientError::ResponseMismatch);
            }
            expected_world = Some(world);
            expected_world_revision = Some(world_revision);
            edits.push(voplay_runtime::inspection::InspectionFieldEdit {
                entity,
                component,
                field,
                value,
            });
        }
        let expected_world_revision =
            expected_world_revision.ok_or(EditorClientError::ResponseMismatch)?;
        Ok(InspectionEditTransaction {
            engine: self.engine,
            transaction_id,
            expected_world_revision,
            edits,
        })
    }

    pub fn owner_snapshot(&self) -> EditorClientOwnerSnapshot {
        EditorClientOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            schemas: self.schemas.len(),
            schema_bytes: self.schema_bytes,
            schemas_frozen: self.frozen,
            schema_fingerprint: self.schema_fingerprint,
            panels: self.panels.len(),
            panel_rows: self.panels.values().map(|panel| panel.rows.len()).sum(),
            pending_requests: self.pending.len(),
            dispatched_requests: self
                .pending
                .values()
                .filter(|request| request.dispatched)
                .count(),
            pending_request_bytes: self.pending_request_bytes,
            play_modes: self.play_modes.len(),
            next_request_id: self.next_request_id,
        }
    }

    pub fn ingest_hierarchy(
        &mut self,
        requested_cursor: usize,
        page: EditorPanelPage<HierarchyInspectionNode>,
    ) -> Result<(), EditorClientError> {
        validate_voplay_panel(&page, self.engine, EditorPanelKind::Hierarchy)?;
        let rows = page.entries.iter().map(hierarchy_row).collect::<Vec<_>>();
        self.ingest_rows(
            EditorPanel::Hierarchy,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            page.dropped_before,
            rows,
        )
    }

    pub fn ingest_inspector(
        &mut self,
        requested_cursor: usize,
        page: InspectionWorldPage,
    ) -> Result<(), EditorClientError> {
        if page.world.engine != self.engine {
            return Err(EditorClientError::WrongEngine);
        }
        let mut schemas = BTreeMap::new();
        for schema in &page.component_schemas {
            if schemas.insert(schema.component, schema).is_some() {
                return Err(EditorClientError::ResponseMismatch);
            }
        }
        let mut rows = Vec::new();
        for entity in &page.entities {
            for component in &entity.components {
                let Some(schema) = schemas.get(&component.component).copied() else {
                    let (value, value_truncated) = hex_preview(&component.value, 4096);
                    let mut cells = BTreeMap::new();
                    cells.insert("stable_key".to_owned(), entity.stable_key.to_string());
                    cells.insert(
                        "world_index".to_owned(),
                        entity.entity.world.handle.index.to_string(),
                    );
                    cells.insert(
                        "world_generation".to_owned(),
                        entity.entity.world.handle.generation.to_string(),
                    );
                    cells.insert(
                        "entity_index".to_owned(),
                        entity.entity.entity.index.to_string(),
                    );
                    cells.insert(
                        "entity_generation".to_owned(),
                        entity.entity.entity.generation.to_string(),
                    );
                    cells.insert("component".to_owned(), component.component.to_string());
                    cells.insert("component_name".to_owned(), "<unknown>".to_owned());
                    cells.insert("type_id".to_owned(), "0".to_owned());
                    cells.insert("field".to_owned(), "0".to_owned());
                    cells.insert("field_name".to_owned(), "<raw>".to_owned());
                    cells.insert("value".to_owned(), value);
                    cells.insert("value_truncated".to_owned(), value_truncated.to_string());
                    cells.insert("editable".to_owned(), "false".to_owned());
                    cells.insert("schema_version".to_owned(), "0".to_owned());
                    cells.insert("world_revision".to_owned(), page.revision.to_string());
                    rows.push(EditorRow {
                        key: format!("{}:{}:0", entity.stable_key, component.component),
                        cells,
                    });
                    continue;
                };
                for field in &schema.fields {
                    let end = field
                        .offset
                        .checked_add(field.width)
                        .ok_or(EditorClientError::ResponseMismatch)?;
                    let field_value = component
                        .value
                        .get(field.offset..end)
                        .ok_or(EditorClientError::ResponseMismatch)?;
                    let (value, value_truncated) = hex_preview(field_value, 4096);
                    let mut cells = BTreeMap::new();
                    cells.insert("stable_key".to_owned(), entity.stable_key.to_string());
                    cells.insert(
                        "world_index".to_owned(),
                        entity.entity.world.handle.index.to_string(),
                    );
                    cells.insert(
                        "world_generation".to_owned(),
                        entity.entity.world.handle.generation.to_string(),
                    );
                    cells.insert(
                        "entity_index".to_owned(),
                        entity.entity.entity.index.to_string(),
                    );
                    cells.insert(
                        "entity_generation".to_owned(),
                        entity.entity.entity.generation.to_string(),
                    );
                    cells.insert("component".to_owned(), component.component.to_string());
                    cells.insert("component_name".to_owned(), schema.name.clone());
                    cells.insert("type_id".to_owned(), schema.type_id.to_string());
                    cells.insert("field".to_owned(), field.field.to_string());
                    cells.insert("field_name".to_owned(), field.name.clone());
                    cells.insert("value".to_owned(), value);
                    cells.insert("value_truncated".to_owned(), value_truncated.to_string());
                    cells.insert(
                        "editable".to_owned(),
                        (field.editable && !value_truncated).to_string(),
                    );
                    cells.insert("schema_version".to_owned(), schema.version.to_string());
                    cells.insert("world_revision".to_owned(), page.revision.to_string());
                    rows.push(EditorRow {
                        key: format!(
                            "{}:{}:{}",
                            entity.stable_key, component.component, field.field
                        ),
                        cells,
                    });
                }
            }
        }
        self.ingest_rows(
            EditorPanel::Inspector,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            0,
            rows,
        )
    }

    pub fn ingest_assets(
        &mut self,
        requested_cursor: usize,
        page: EditorPanelPage<AssetInspectionEntry>,
    ) -> Result<(), EditorClientError> {
        validate_voplay_panel(&page, self.engine, EditorPanelKind::Assets)?;
        let rows = page.entries.iter().map(asset_row).collect();
        self.ingest_rows(
            EditorPanel::Assets,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            page.dropped_before,
            rows,
        )
    }

    pub fn ingest_schedule(
        &mut self,
        requested_cursor: usize,
        page: EditorPanelPage<SystemSpec>,
    ) -> Result<(), EditorClientError> {
        validate_voplay_panel(&page, self.engine, EditorPanelKind::Schedule)?;
        let rows = page.entries.iter().map(schedule_row).collect();
        self.ingest_rows(
            EditorPanel::Schedule,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            page.dropped_before,
            rows,
        )
    }

    pub fn ingest_render(
        &mut self,
        requested_cursor: usize,
        page: EditorPanelPage<RenderPanelEntry>,
    ) -> Result<(), EditorClientError> {
        validate_voplay_panel(&page, self.engine, EditorPanelKind::Render)?;
        let rows = page.entries.iter().map(render_row).collect();
        self.ingest_rows(
            EditorPanel::Render,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            page.dropped_before,
            rows,
        )
    }

    pub fn ingest_performance(
        &mut self,
        requested_cursor: usize,
        page: EditorPanelPage<PerfPanelSample>,
    ) -> Result<(), EditorClientError> {
        validate_voplay_panel(&page, self.engine, EditorPanelKind::Performance)?;
        let rows = page.entries.iter().map(perf_row).collect();
        self.ingest_rows(
            EditorPanel::Performance,
            requested_cursor,
            page.source_revision,
            page.next_cursor,
            page.dropped_before,
            rows,
        )
    }

    pub fn build_panel_view(&self, panel: EditorPanel) -> Result<ViewNode, EditorClientError> {
        if !self.frozen {
            return Err(EditorClientError::RegistryNotFrozen);
        }
        let schema = self
            .schemas
            .get(&panel)
            .ok_or(EditorClientError::MissingSchema)?;
        let state = self.panels.get(&panel);
        let mut props = BTreeMap::new();
        props.insert("data-panel".to_owned(), panel.stable_name().to_owned());
        props.insert("data-title".to_owned(), schema.title.clone());
        props.insert(
            "data-schema-type".to_owned(),
            schema.schema_type.to_string(),
        );
        props.insert(
            "data-schema-version".to_owned(),
            schema.schema_version.to_string(),
        );
        props.insert(
            "data-schema-fingerprint".to_owned(),
            self.schema_fingerprint.to_string(),
        );
        props.insert(
            "data-source-revision".to_owned(),
            state.map_or(0, |state| state.source_revision).to_string(),
        );
        props.insert(
            "data-complete".to_owned(),
            state
                .is_none_or(|state| state.next_cursor.is_none())
                .to_string(),
        );
        let children = state
            .into_iter()
            .flat_map(|state| &state.rows)
            .map(|row| ViewNode {
                key: Some(row.key.clone()),
                kind: ViewKind::Element("voplay-editor-row".to_owned()),
                props: row.cells.clone(),
                children: Vec::new(),
            })
            .collect();
        Ok(ViewNode {
            key: Some(panel.stable_name().to_owned()),
            kind: ViewKind::Element("voplay-editor-panel".to_owned()),
            props,
            children,
        })
    }

    pub fn enqueue_request(
        &mut self,
        mut kind: EditorRequestKind,
    ) -> Result<u64, EditorClientError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(EditorClientError::RegistryNotFrozen);
        }
        validate_request_owner(self.engine, &kind)?;
        if self.pending.len() == self.config.max_pending_requests {
            return Err(EditorClientError::RequestCapacity);
        }
        let encoded_bytes = request_bytes(&kind).ok_or(EditorClientError::RequestByteCapacity)?;
        let next_bytes = self
            .pending_request_bytes
            .checked_add(encoded_bytes)
            .filter(|bytes| *bytes <= self.config.max_request_bytes)
            .ok_or(EditorClientError::RequestByteCapacity)?;
        let request_id = self.next_request_id;
        if let EditorRequestKind::Pick(request) = &mut kind {
            request.request_id = request_id;
        }
        let next_request_id = request_id
            .checked_add(1)
            .ok_or(EditorClientError::RequestIdExhausted)?;
        self.pending.insert(
            request_id,
            PendingRequest {
                request: EditorRequest {
                    request_id,
                    engine: self.engine,
                    kind,
                },
                encoded_bytes,
                dispatched: false,
            },
        );
        self.request_order.push_back(request_id);
        self.pending_request_bytes = next_bytes;
        self.next_request_id = next_request_id;
        Ok(request_id)
    }

    pub fn poll_request(&mut self) -> Option<EditorRequest> {
        if self.closed {
            return None;
        }
        while let Some(request_id) = self.request_order.pop_front() {
            if let Ok(request) = self.dispatch_request(request_id) {
                return Some(request);
            }
        }
        None
    }

    pub fn dispatch_request(
        &mut self,
        request_id: u64,
    ) -> Result<EditorRequest, EditorClientError> {
        self.ensure_open()?;
        let pending = self
            .pending
            .get_mut(&request_id)
            .ok_or(EditorClientError::UnknownRequest)?;
        if pending.dispatched {
            return Err(EditorClientError::RequestAlreadyDispatched);
        }
        pending.dispatched = true;
        self.request_order.retain(|queued| *queued != request_id);
        Ok(pending.request.clone())
    }

    pub fn complete_host_dispatch(
        &mut self,
        dispatch: EditorHostDispatch,
    ) -> Result<Option<EditorResponse>, EditorClientError> {
        self.ensure_open()?;
        let response = match dispatch {
            EditorHostDispatch::Complete(response) => response,
            EditorHostDispatch::PickPending { request_id, .. } => {
                let pending = self
                    .pending
                    .get(&request_id)
                    .ok_or(EditorClientError::UnknownRequest)?;
                if !pending.dispatched
                    || !matches!(
                        &pending.request.kind,
                        EditorRequestKind::Pick(request) if request.request_id == request_id
                    )
                {
                    return Err(EditorClientError::ResponseMismatch);
                }
                return Ok(None);
            }
            EditorHostDispatch::HierarchyPage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Hierarchy, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Hierarchy, cursor, revision)?;
                self.ingest_hierarchy(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Hierarchy, request_id);
                self.page_response(request_id, revision)
            }
            EditorHostDispatch::InspectorPage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Inspector, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Inspector, cursor, revision)?;
                self.ingest_inspector(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Inspector, request_id);
                self.page_response(request_id, revision)
            }
            EditorHostDispatch::AssetsPage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Assets, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Assets, cursor, revision)?;
                self.ingest_assets(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Assets, request_id);
                self.page_response(request_id, revision)
            }
            EditorHostDispatch::SchedulePage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Schedule, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Schedule, cursor, revision)?;
                self.ingest_schedule(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Schedule, request_id);
                self.page_response(request_id, revision)
            }
            EditorHostDispatch::RenderPage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Render, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Render, cursor, revision)?;
                self.ingest_render(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Render, request_id);
                self.page_response(request_id, revision)
            }
            EditorHostDispatch::PerformancePage {
                request_id,
                cursor,
                page,
            } => {
                if self.page_response_is_superseded(EditorPanel::Performance, request_id) {
                    return self
                        .complete_request(self.rejected_response(request_id))
                        .map(Some);
                }
                let revision = page.source_revision;
                self.expect_page_request(request_id, EditorPanel::Performance, cursor, revision)?;
                self.ingest_performance(cursor, page)?;
                self.panel_applied_requests
                    .insert(EditorPanel::Performance, request_id);
                self.page_response(request_id, revision)
            }
        };
        self.complete_request(response).map(Some)
    }

    pub fn complete_request(
        &mut self,
        response: EditorResponse,
    ) -> Result<EditorResponse, EditorClientError> {
        self.ensure_open()?;
        if response.engine != self.engine {
            return Err(EditorClientError::WrongEngine);
        }
        let pending = self
            .pending
            .get(&response.request_id)
            .ok_or(EditorClientError::UnknownRequest)?;
        if !pending.dispatched {
            return Err(EditorClientError::RequestNotDispatched);
        }
        match (&pending.request.kind, response.outcome) {
            (
                EditorRequestKind::StartPlayMode {
                    authoring_world,
                    expected_authoring_revision,
                    resources,
                    ..
                },
                EditorResponseOutcome::PlayModeStarted(report),
            ) => {
                if report.id.editor_engine != self.engine
                    || report.authoring_world != *authoring_world
                    || report.authoring_revision != *expected_authoring_revision
                    || report.resources != *resources
                {
                    return Err(EditorClientError::ResponseMismatch);
                }
                if self.play_modes.contains_key(&report.id) {
                    return Err(EditorClientError::DuplicatePlayMode);
                }
            }
            (
                EditorRequestKind::ExitPlayMode { id, .. },
                EditorResponseOutcome::PlayModeExited(report),
            ) if report.id == *id => {
                if !self.play_modes.contains_key(id) {
                    return Err(EditorClientError::UnknownPlayMode);
                }
            }
            (EditorRequestKind::StartPlayMode { .. }, EditorResponseOutcome::Rejected)
            | (
                EditorRequestKind::StartPlayMode { .. },
                EditorResponseOutcome::RevisionConflict { .. },
            )
            | (EditorRequestKind::ExitPlayMode { .. }, EditorResponseOutcome::Rejected)
            | (
                EditorRequestKind::ExitPlayMode { .. },
                EditorResponseOutcome::RevisionConflict { .. },
            ) => {}
            (EditorRequestKind::StartPlayMode { .. }, _)
            | (EditorRequestKind::ExitPlayMode { .. }, _) => {
                return Err(EditorClientError::ResponseMismatch);
            }
            (
                _,
                EditorResponseOutcome::PlayModeStarted(_)
                | EditorResponseOutcome::PlayModeExited(_)
                | EditorResponseOutcome::DroppedBeforeDispatch
                | EditorResponseOutcome::OutcomeUnknownOnRestart,
            ) => return Err(EditorClientError::ResponseMismatch),
            _ => {}
        }
        let pending = self.pending.remove(&response.request_id).unwrap();
        self.pending_request_bytes -= pending.encoded_bytes;
        match response.outcome {
            EditorResponseOutcome::PlayModeStarted(report) => {
                self.play_modes.insert(report.id, report);
            }
            EditorResponseOutcome::PlayModeExited(report) => {
                self.play_modes.remove(&report.id);
            }
            _ => {}
        }
        Ok(response)
    }

    pub fn play_modes(&self) -> impl ExactSizeIterator<Item = &PlayModeStartReport> {
        self.play_modes.values()
    }

    pub fn reconcile_play_modes(
        &mut self,
        snapshot: &voplay_runtime::play_mode::PlayModeOwnerSnapshot,
    ) -> Result<(), EditorClientError> {
        self.ensure_open()?;
        if snapshot.editor_engine != self.engine
            || snapshot.closed
            || snapshot.live_sessions != snapshot.sessions.len()
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        let mut staged = BTreeMap::new();
        let mut play_engines = BTreeSet::new();
        let mut play_worlds = BTreeSet::new();
        let mut input_scopes = BTreeSet::new();
        let mut render_targets = BTreeSet::new();
        let mut asset_scopes = BTreeSet::new();
        for session in &snapshot.sessions {
            if session.id.editor_engine != self.engine
                || session.authoring_world.engine != self.engine
                || session.resources.world.engine == self.engine
                || !session.resources.world.is_valid()
                || session.resources.input_scope.engine != session.resources.world.engine
                || session.resources.render_target.engine != session.resources.world.engine
                || session.resources.asset_scope.engine != session.resources.world.engine
                || session.resources.render_target.kind
                    != voplay_runtime::control::ControlKind::RenderTarget
                || !play_engines.insert(session.resources.world.engine)
                || !play_worlds.insert(session.resources.world)
                || !input_scopes.insert(session.resources.input_scope)
                || !render_targets.insert(session.resources.render_target)
                || !asset_scopes.insert(session.resources.asset_scope)
                || staged
                    .insert(
                        session.id,
                        PlayModeStartReport {
                            id: session.id,
                            authoring_world: session.authoring_world,
                            authoring_revision: session.authoring_revision,
                            resources: session.resources,
                            cloned_entities: session.cloned_entities,
                            cloned_bytes: session.cloned_bytes,
                        },
                    )
                    .is_some()
            {
                return Err(EditorClientError::ResponseMismatch);
            }
        }
        self.play_modes = staged;
        Ok(())
    }

    pub fn cancel_request(&mut self, request_id: u64) -> Result<EditorResponse, EditorClientError> {
        self.ensure_open()?;
        let pending = self
            .pending
            .get(&request_id)
            .ok_or(EditorClientError::UnknownRequest)?;
        if pending.dispatched {
            return Err(EditorClientError::RequestAlreadyDispatched);
        }
        let pending = self.pending.remove(&request_id).unwrap();
        self.request_order.retain(|queued| *queued != request_id);
        self.pending_request_bytes -= pending.encoded_bytes;
        Ok(EditorResponse {
            request_id,
            engine: self.engine,
            outcome: EditorResponseOutcome::Cancelled,
        })
    }

    pub fn restart_endpoint(&mut self) -> Vec<EditorResponse> {
        if self.closed {
            return Vec::new();
        }
        let pending = std::mem::take(&mut self.pending);
        self.request_order.clear();
        self.pending_request_bytes = 0;
        pending
            .into_values()
            .map(|pending| EditorResponse {
                request_id: pending.request.request_id,
                engine: self.engine,
                outcome: if pending.dispatched {
                    EditorResponseOutcome::OutcomeUnknownOnRestart
                } else {
                    EditorResponseOutcome::DroppedBeforeDispatch
                },
            })
            .collect()
    }

    pub fn shutdown(&mut self) -> EditorClientShutdownReport {
        let before = self.owner_snapshot();
        if self.closed {
            return EditorClientShutdownReport {
                before: before.clone(),
                terminal_requests: Vec::new(),
                after: before,
            };
        }
        let terminal_requests = self.restart_endpoint();
        self.schemas.clear();
        self.schema_bytes = 0;
        self.frozen = false;
        self.schema_fingerprint = 0;
        self.panels.clear();
        self.panel_applied_requests.clear();
        self.play_modes.clear();
        self.closed = true;
        EditorClientShutdownReport {
            before,
            terminal_requests,
            after: self.owner_snapshot(),
        }
    }

    fn expect_page_request(
        &self,
        request_id: u64,
        panel: EditorPanel,
        cursor: usize,
        source_revision: u64,
    ) -> Result<(), EditorClientError> {
        let pending = self
            .pending
            .get(&request_id)
            .ok_or(EditorClientError::UnknownRequest)?;
        if !pending.dispatched
            || !matches!(
                &pending.request.kind,
                EditorRequestKind::Refresh {
                    panel: requested,
                    cursor: requested_cursor,
                    expected_source_revision,
                } if *requested == panel
                    && *requested_cursor == cursor
                    && expected_source_revision
                        .is_none_or(|expected| expected == source_revision)
            )
        {
            return Err(EditorClientError::ResponseMismatch);
        }
        Ok(())
    }

    fn page_response(&self, request_id: u64, revision: u64) -> EditorResponse {
        EditorResponse {
            request_id,
            engine: self.engine,
            outcome: EditorResponseOutcome::Applied { revision },
        }
    }

    fn rejected_response(&self, request_id: u64) -> EditorResponse {
        EditorResponse {
            request_id,
            engine: self.engine,
            outcome: EditorResponseOutcome::Rejected,
        }
    }

    fn page_response_is_superseded(&self, panel: EditorPanel, request_id: u64) -> bool {
        self.panel_applied_requests
            .get(&panel)
            .is_some_and(|applied| *applied > request_id)
    }

    fn ingest_rows(
        &mut self,
        panel: EditorPanel,
        requested_cursor: usize,
        source_revision: u64,
        next_cursor: Option<usize>,
        dropped_before: u64,
        rows: Vec<EditorRow>,
    ) -> Result<(), EditorClientError> {
        self.ensure_open()?;
        if !self.frozen {
            return Err(EditorClientError::RegistryNotFrozen);
        }
        if next_cursor.is_some_and(|next| next <= requested_cursor) {
            return Err(EditorClientError::InvalidCursor);
        }
        if requested_cursor > 0 {
            let state = self
                .panels
                .get(&panel)
                .ok_or(EditorClientError::InvalidCursor)?;
            if state.source_revision != source_revision {
                return Err(EditorClientError::StalePage);
            }
            if state.next_cursor != Some(requested_cursor) {
                return Err(EditorClientError::InvalidCursor);
            }
        }
        let (base_rows, base_bytes) = if requested_cursor == 0 {
            (Vec::new(), 0)
        } else {
            let state = self.panels.get(&panel).unwrap();
            (state.rows.clone(), state.encoded_bytes)
        };
        let mut staged_rows = base_rows;
        let mut staged_bytes = base_bytes;
        let expected_columns = self
            .schemas
            .get(&panel)
            .ok_or(EditorClientError::MissingSchema)?
            .columns
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for row in rows {
            if row
                .cells
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != expected_columns
            {
                return Err(EditorClientError::RowSchemaMismatch);
            }
            let row_bytes = encoded_row_bytes(&row).ok_or(EditorClientError::RowByteCapacity)?;
            if row_bytes > self.config.max_row_bytes {
                return Err(EditorClientError::RowByteCapacity);
            }
            if staged_rows.len() == self.config.max_rows_per_panel {
                return Err(EditorClientError::RowCapacity);
            }
            staged_bytes = staged_bytes
                .checked_add(row_bytes)
                .filter(|bytes| *bytes <= self.config.max_panel_bytes)
                .ok_or(EditorClientError::PanelByteCapacity)?;
            staged_rows.push(row);
        }
        let keys = staged_rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<BTreeSet<_>>();
        if keys.len() != staged_rows.len() {
            return Err(EditorClientError::StalePage);
        }
        self.panels.insert(
            panel,
            EditorPanelState {
                panel,
                source_revision,
                rows: staged_rows,
                next_cursor,
                encoded_bytes: staged_bytes,
                dropped_before,
            },
        );
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), EditorClientError> {
        if self.closed {
            Err(EditorClientError::Closed)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StudioSessionId {
    pub studio: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StudioSessionConfig {
    pub max_sessions: usize,
}

impl Default for StudioSessionConfig {
    fn default() -> Self {
        Self { max_sessions: 16 }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StudioSessionOwnerSnapshot {
    pub id: StudioSessionId,
    pub endpoint_generation: u32,
    pub editor_engine: EngineId,
    pub authoring_world: WorldId,
    pub authoring_revision: u64,
    pub authoring_entities: usize,
    pub client: EditorClientOwnerSnapshot,
    pub previews: voplay_runtime::play_mode::PlayModeOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StudioOwnerSnapshot {
    pub studio: Handle,
    pub closed: bool,
    pub live_sessions: usize,
    pub sessions: Vec<StudioSessionOwnerSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StudioShutdownReport {
    pub before: StudioOwnerSnapshot,
    pub sessions: Vec<StudioSessionShutdownReport>,
    pub after: StudioOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StudioEndpointRestartReport {
    pub session: StudioSessionId,
    pub endpoint_generation: u32,
    pub terminal_requests: Vec<EditorResponse>,
    pub discarded_previews: Vec<PlayModeExitReport>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StudioSessionShutdownReport {
    pub session: StudioSessionId,
    pub endpoint_generation: u32,
    pub final_authoring_revision: u64,
    pub final_authoring_entities: usize,
    pub terminal_requests: Vec<EditorResponse>,
    pub discarded_previews: Vec<PlayModeExitReport>,
}

#[derive(Debug)]
pub enum StudioSessionError {
    InvalidConfig,
    InvalidSession,
    StaleSession,
    SessionCapacity,
    DuplicateEditorEngine,
    PreviewResourceCollision,
    GenerationExhausted,
    Closed,
    UnexpectedResponse,
    Client(EditorClientError),
    PlayMode(PlayModeError),
    PlayModeHost(EditorPlayModeHostError),
}

impl From<EditorClientError> for StudioSessionError {
    fn from(error: EditorClientError) -> Self {
        Self::Client(error)
    }
}

impl From<PlayModeError> for StudioSessionError {
    fn from(error: PlayModeError) -> Self {
        Self::PlayMode(error)
    }
}

impl From<EditorPlayModeHostError> for StudioSessionError {
    fn from(error: EditorPlayModeHostError) -> Self {
        Self::PlayModeHost(error)
    }
}

struct StudioSession {
    authoring: World,
    client: EditorClient,
    play_mode_host: EditorPlayModeHost,
    play_mode_config: PlayModeConfig,
    endpoint_generation: u32,
}

struct StudioSessionSlot {
    generation: u32,
    session: Option<StudioSession>,
}

pub struct StudioSessionManager {
    studio: Handle,
    config: StudioSessionConfig,
    slots: Vec<StudioSessionSlot>,
    free: Vec<u32>,
    live: usize,
    closed: bool,
}

impl StudioSessionManager {
    pub fn new(studio: Handle, config: StudioSessionConfig) -> Result<Self, StudioSessionError> {
        if !studio.is_valid() || config.max_sessions == 0 || config.max_sessions > u32::MAX as usize
        {
            return Err(StudioSessionError::InvalidConfig);
        }
        Ok(Self {
            studio,
            config,
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            closed: false,
        })
    }

    pub const fn live_sessions(&self) -> usize {
        self.live
    }

    pub fn create_session(
        &mut self,
        authoring: World,
        client_config: EditorClientConfig,
        play_mode_config: PlayModeConfig,
    ) -> Result<StudioSessionId, StudioSessionError> {
        self.ensure_open()?;
        if self.live == self.config.max_sessions {
            return Err(StudioSessionError::SessionCapacity);
        }
        let editor_engine = authoring.id().engine;
        if !editor_engine.is_valid()
            || self.live_sessions_iter().any(|session| {
                session.authoring.id().engine == editor_engine
                    || session
                        .play_mode_host
                        .owner_snapshot()
                        .sessions
                        .iter()
                        .any(|preview| preview.resources.world.engine == editor_engine)
            })
        {
            return Err(StudioSessionError::DuplicateEditorEngine);
        }
        let mut client = EditorClient::new(editor_engine, client_config)?;
        for panel in EditorPanel::ALL {
            client.register_schema(EditorPanelSchema::canonical(panel))?;
        }
        client.freeze_schemas()?;
        let play_mode_host = EditorPlayModeHost::new(editor_engine, play_mode_config)?;
        let (index, generation) = self.allocate_slot()?;
        self.slots[index as usize].session = Some(StudioSession {
            authoring,
            client,
            play_mode_host,
            play_mode_config,
            endpoint_generation: 1,
        });
        self.live += 1;
        Ok(StudioSessionId {
            studio: self.studio,
            handle: Handle { index, generation },
        })
    }

    pub fn owner_snapshot(&self) -> StudioOwnerSnapshot {
        let sessions = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let session = slot.session.as_ref()?;
                Some(StudioSessionOwnerSnapshot {
                    id: StudioSessionId {
                        studio: self.studio,
                        handle: Handle {
                            index: index as u32,
                            generation: slot.generation,
                        },
                    },
                    endpoint_generation: session.endpoint_generation,
                    editor_engine: session.authoring.id().engine,
                    authoring_world: session.authoring.id(),
                    authoring_revision: session.authoring.revision(),
                    authoring_entities: session.authoring.live_entities(),
                    client: session.client.owner_snapshot(),
                    previews: session.play_mode_host.owner_snapshot(),
                })
            })
            .collect();
        StudioOwnerSnapshot {
            studio: self.studio,
            closed: self.closed,
            live_sessions: self.live,
            sessions,
        }
    }

    pub fn authoring_world(&self, id: StudioSessionId) -> Result<&World, StudioSessionError> {
        Ok(&self.session(id)?.authoring)
    }

    pub fn authoring_world_mut(
        &mut self,
        id: StudioSessionId,
    ) -> Result<&mut World, StudioSessionError> {
        Ok(&mut self.session_mut(id)?.authoring)
    }

    pub fn client(&self, id: StudioSessionId) -> Result<&EditorClient, StudioSessionError> {
        Ok(&self.session(id)?.client)
    }

    pub fn client_mut(
        &mut self,
        id: StudioSessionId,
    ) -> Result<&mut EditorClient, StudioSessionError> {
        Ok(&mut self.session_mut(id)?.client)
    }

    pub fn preview_world(
        &self,
        session: StudioSessionId,
        preview: PlayModeId,
    ) -> Result<&World, StudioSessionError> {
        Ok(self.session(session)?.play_mode_host.play_world(preview)?)
    }

    pub fn preview_world_mut(
        &mut self,
        session: StudioSessionId,
        preview: PlayModeId,
    ) -> Result<&mut World, StudioSessionError> {
        Ok(self
            .session_mut(session)?
            .play_mode_host
            .play_world_mut(preview)?)
    }

    pub fn start_preview(
        &mut self,
        id: StudioSessionId,
        resources: PlayModeResources,
        world_config: WorldConfig,
        policy: PlayModePolicy,
    ) -> Result<PlayModeStartReport, StudioSessionError> {
        self.session(id)?;
        if self.preview_resources_collide(resources) {
            return Err(StudioSessionError::PreviewResourceCollision);
        }
        let session = self.session_mut(id)?;
        let request_id = session
            .client
            .enqueue_request(EditorRequestKind::StartPlayMode {
                authoring_world: session.authoring.id(),
                expected_authoring_revision: session.authoring.revision(),
                resources,
                world_config,
                policy,
            })?;
        let request = session.client.dispatch_request(request_id)?;
        let response = session
            .play_mode_host
            .dispatch(&mut session.authoring, &request)?;
        let response = session.client.complete_request(response)?;
        match response.outcome {
            EditorResponseOutcome::PlayModeStarted(report) => Ok(report),
            EditorResponseOutcome::RevisionConflict { .. } => {
                Err(StudioSessionError::UnexpectedResponse)
            }
            _ => Err(StudioSessionError::UnexpectedResponse),
        }
    }

    pub fn exit_preview(
        &mut self,
        session_id: StudioSessionId,
        preview: PlayModeId,
        exit: PlayModeExit,
    ) -> Result<PlayModeExitReport, StudioSessionError> {
        let session = self.session_mut(session_id)?;
        let request_id = session
            .client
            .enqueue_request(EditorRequestKind::ExitPlayMode { id: preview, exit })?;
        let request = session.client.dispatch_request(request_id)?;
        let response = session
            .play_mode_host
            .dispatch(&mut session.authoring, &request)?;
        let response = session.client.complete_request(response)?;
        match response.outcome {
            EditorResponseOutcome::PlayModeExited(report) => Ok(report),
            _ => Err(StudioSessionError::UnexpectedResponse),
        }
    }

    pub fn restart_client_endpoint(
        &mut self,
        id: StudioSessionId,
    ) -> Result<StudioEndpointRestartReport, StudioSessionError> {
        let session = self.session_mut(id)?;
        let endpoint_generation = session
            .endpoint_generation
            .checked_add(1)
            .ok_or(StudioSessionError::GenerationExhausted)?;
        let authority = session.play_mode_host.owner_snapshot();
        let terminal_requests = session.client.restart_endpoint();
        session.client.reconcile_play_modes(&authority)?;
        session.endpoint_generation = endpoint_generation;
        Ok(StudioEndpointRestartReport {
            session: id,
            endpoint_generation,
            terminal_requests,
            discarded_previews: Vec::new(),
        })
    }

    pub fn restart_play_mode_endpoint(
        &mut self,
        id: StudioSessionId,
    ) -> Result<StudioEndpointRestartReport, StudioSessionError> {
        let session = self.session_mut(id)?;
        let endpoint_generation = session
            .endpoint_generation
            .checked_add(1)
            .ok_or(StudioSessionError::GenerationExhausted)?;
        let replacement =
            EditorPlayModeHost::new(session.authoring.id().engine, session.play_mode_config)?;
        let discarded_previews = session.play_mode_host.shutdown()?;
        let terminal_requests = session.client.restart_endpoint();
        session.play_mode_host = replacement;
        session
            .client
            .reconcile_play_modes(&session.play_mode_host.owner_snapshot())?;
        session.endpoint_generation = endpoint_generation;
        Ok(StudioEndpointRestartReport {
            session: id,
            endpoint_generation,
            terminal_requests,
            discarded_previews,
        })
    }

    pub fn shutdown_session(
        &mut self,
        id: StudioSessionId,
    ) -> Result<StudioSessionShutdownReport, StudioSessionError> {
        self.session(id)?;
        self.slots[id.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(StudioSessionError::GenerationExhausted)?;
        let session = self.session_mut(id)?;
        let discarded_previews = session.play_mode_host.shutdown()?;
        let terminal_requests = session.client.shutdown().terminal_requests;
        let report = StudioSessionShutdownReport {
            session: id,
            endpoint_generation: session.endpoint_generation,
            final_authoring_revision: session.authoring.revision(),
            final_authoring_entities: session.authoring.live_entities(),
            terminal_requests,
            discarded_previews,
        };
        self.release_slot(id)?;
        Ok(report)
    }

    pub fn shutdown_all(&mut self) -> Result<Vec<StudioSessionShutdownReport>, StudioSessionError> {
        let ids = self.live_session_ids().collect::<Vec<_>>();
        for id in &ids {
            self.slots[id.handle.index as usize]
                .generation
                .checked_add(1)
                .ok_or(StudioSessionError::GenerationExhausted)?;
            for preview in &self.session(*id)?.play_mode_host.owner_snapshot().sessions {
                preview
                    .id
                    .handle
                    .generation
                    .checked_add(1)
                    .ok_or(StudioSessionError::GenerationExhausted)?;
            }
        }
        ids.into_iter()
            .map(|id| self.shutdown_session(id))
            .collect()
    }

    pub fn shutdown(&mut self) -> Result<StudioShutdownReport, StudioSessionError> {
        let before = self.owner_snapshot();
        if self.closed {
            return Ok(StudioShutdownReport {
                before: before.clone(),
                sessions: Vec::new(),
                after: before,
            });
        }
        let sessions = self.shutdown_all()?;
        self.closed = true;
        Ok(StudioShutdownReport {
            before,
            sessions,
            after: self.owner_snapshot(),
        })
    }

    fn preview_resources_collide(&self, candidate: PlayModeResources) -> bool {
        self.live_sessions_iter().any(|session| {
            session.authoring.id().engine == candidate.world.engine
                || session
                    .play_mode_host
                    .owner_snapshot()
                    .sessions
                    .iter()
                    .any(|preview| {
                        preview.resources.world.engine == candidate.world.engine
                            || preview.resources.world == candidate.world
                            || preview.resources.input_scope == candidate.input_scope
                            || preview.resources.render_target == candidate.render_target
                            || preview.resources.asset_scope == candidate.asset_scope
                    })
        })
    }

    fn live_session_ids(&self) -> impl Iterator<Item = StudioSessionId> + '_ {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.session.as_ref().map(|_| StudioSessionId {
                studio: self.studio,
                handle: Handle {
                    index: index as u32,
                    generation: slot.generation,
                },
            })
        })
    }

    fn live_sessions_iter(&self) -> impl Iterator<Item = &StudioSession> {
        self.slots.iter().filter_map(|slot| slot.session.as_ref())
    }

    fn session(&self, id: StudioSessionId) -> Result<&StudioSession, StudioSessionError> {
        if id.studio != self.studio || !id.handle.is_valid() {
            return Err(StudioSessionError::InvalidSession);
        }
        let slot = self
            .slots
            .get(id.handle.index as usize)
            .ok_or(StudioSessionError::InvalidSession)?;
        if slot.generation != id.handle.generation || slot.session.is_none() {
            return Err(StudioSessionError::StaleSession);
        }
        slot.session
            .as_ref()
            .ok_or(StudioSessionError::StaleSession)
    }

    fn session_mut(
        &mut self,
        id: StudioSessionId,
    ) -> Result<&mut StudioSession, StudioSessionError> {
        if id.studio != self.studio || !id.handle.is_valid() {
            return Err(StudioSessionError::InvalidSession);
        }
        let slot = self
            .slots
            .get_mut(id.handle.index as usize)
            .ok_or(StudioSessionError::InvalidSession)?;
        if slot.generation != id.handle.generation || slot.session.is_none() {
            return Err(StudioSessionError::StaleSession);
        }
        slot.session
            .as_mut()
            .ok_or(StudioSessionError::StaleSession)
    }

    fn allocate_slot(&mut self) -> Result<(u32, u32), StudioSessionError> {
        self.ensure_open()?;
        if let Some(index) = self.free.pop() {
            let generation = self.slots[index as usize].generation;
            return Ok((index, generation));
        }
        let index =
            u32::try_from(self.slots.len()).map_err(|_| StudioSessionError::SessionCapacity)?;
        self.slots.push(StudioSessionSlot {
            generation: 1,
            session: None,
        });
        Ok((index, 1))
    }

    fn ensure_open(&self) -> Result<(), StudioSessionError> {
        if self.closed {
            Err(StudioSessionError::Closed)
        } else {
            Ok(())
        }
    }

    fn release_slot(&mut self, id: StudioSessionId) -> Result<(), StudioSessionError> {
        let slot = self
            .slots
            .get_mut(id.handle.index as usize)
            .ok_or(StudioSessionError::InvalidSession)?;
        if slot.generation != id.handle.generation || slot.session.is_none() {
            return Err(StudioSessionError::StaleSession);
        }
        let generation = slot
            .generation
            .checked_add(1)
            .ok_or(StudioSessionError::GenerationExhausted)?;
        slot.session = None;
        slot.generation = generation;
        self.free.push(id.handle.index);
        self.live -= 1;
        Ok(())
    }
}

fn validate_voplay_panel<T>(
    page: &EditorPanelPage<T>,
    engine: EngineId,
    expected: EditorPanelKind,
) -> Result<(), EditorClientError> {
    if page.engine != engine {
        return Err(EditorClientError::WrongEngine);
    }
    if page.panel != expected {
        return Err(EditorClientError::WrongPanel);
    }
    Ok(())
}

fn hierarchy_row(node: &HierarchyInspectionNode) -> EditorRow {
    let mut cells = BTreeMap::new();
    cells.insert(
        "parent".to_owned(),
        node.parent.map_or_else(
            || "root".to_owned(),
            |parent| format!("{}:{}", parent.entity.index, parent.entity.generation),
        ),
    );
    cells.insert("child_count".to_owned(), node.child_count.to_string());
    cells.insert("local".to_owned(), format_vector(node.local.translation));
    cells.insert("world".to_owned(), format_vector(node.world.translation));
    EditorRow {
        key: format!(
            "{}:{}",
            node.entity.entity.index, node.entity.entity.generation
        ),
        cells,
    }
}

fn asset_row(entry: &AssetInspectionEntry) -> EditorRow {
    let mut cells = BTreeMap::new();
    cells.insert("asset_id".to_owned(), hex(&entry.asset_id.0));
    cells.insert("asset_type".to_owned(), entry.asset_type.to_string());
    cells.insert(
        "source_revision".to_owned(),
        entry.source_revision.to_string(),
    );
    cells.insert("artifact_id".to_owned(), hex(&entry.artifact_id.0));
    cells.insert("state".to_owned(), format!("{:?}", entry.state));
    cells.insert(
        "dependencies".to_owned(),
        entry.dependencies.len().to_string(),
    );
    cells.insert("leases".to_owned(), entry.lease_count.to_string());
    EditorRow {
        key: format!(
            "{}:{}",
            entry.asset_ref.handle.index, entry.asset_ref.handle.generation
        ),
        cells,
    }
}

fn schedule_row(system: &SystemSpec) -> EditorRow {
    let mut cells = BTreeMap::new();
    cells.insert("stage".to_owned(), format!("{:?}", system.stage));
    cells.insert("deterministic".to_owned(), system.deterministic.to_string());
    cells.insert("before".to_owned(), system.before.join(","));
    cells.insert("after".to_owned(), system.after.join(","));
    EditorRow {
        key: system.name.clone(),
        cells,
    }
}

fn render_row(entry: &RenderPanelEntry) -> EditorRow {
    let mut cells = BTreeMap::from([
        ("kind".to_owned(), String::new()),
        ("position".to_owned(), String::new()),
        ("node".to_owned(), String::new()),
        ("resource".to_owned(), String::new()),
        ("version".to_owned(), String::new()),
        ("slot".to_owned(), String::new()),
    ]);
    let key = match entry {
        RenderPanelEntry::Node { position, node } => {
            cells.insert("kind".to_owned(), "node".to_owned());
            cells.insert("position".to_owned(), position.to_string());
            cells.insert("node".to_owned(), node.0.to_string());
            format!("node:{}", node.0)
        }
        RenderPanelEntry::FinalVersion { resource, version } => {
            cells.insert("kind".to_owned(), "final-version".to_owned());
            cells.insert("resource".to_owned(), resource.0.to_string());
            cells.insert("version".to_owned(), version.to_string());
            format!("version:{}", resource.0)
        }
        RenderPanelEntry::Allocation { resource, slot } => {
            cells.insert("kind".to_owned(), "allocation".to_owned());
            cells.insert("resource".to_owned(), resource.0.to_string());
            cells.insert("slot".to_owned(), slot.to_string());
            format!("allocation:{}", resource.0)
        }
    };
    EditorRow { key, cells }
}

fn perf_row(sample: &PerfPanelSample) -> EditorRow {
    let mut cells = BTreeMap::new();
    cells.insert("tick".to_owned(), sample.tick.to_string());
    cells.insert("system".to_owned(), sample.system.clone());
    cells.insert(
        "duration_nanos".to_owned(),
        sample.duration_nanos.to_string(),
    );
    cells.insert(
        "allocation_bytes".to_owned(),
        sample.allocation_bytes.to_string(),
    );
    cells.insert(
        "lock_wait_nanos".to_owned(),
        sample.lock_wait_nanos.to_string(),
    );
    cells.insert("queue_items".to_owned(), sample.queue_items.to_string());
    cells.insert("queue_bytes".to_owned(), sample.queue_bytes.to_string());
    cells.insert("error_count".to_owned(), sample.error_count.to_string());
    cells.insert(
        "live_roles".to_owned(),
        sample.owners.live_roles.to_string(),
    );
    cells.insert(
        "live_entities".to_owned(),
        sample.owners.live_entities.to_string(),
    );
    cells.insert(
        "live_surfaces".to_owned(),
        sample.owners.live_surfaces.to_string(),
    );
    cells.insert(
        "input_scopes".to_owned(),
        sample.owners.input_scopes.to_string(),
    );
    cells.insert(
        "pending_render_ops".to_owned(),
        sample.owners.pending_render_ops.to_string(),
    );
    cells.insert(
        "pending_render_bytes".to_owned(),
        sample.owners.pending_render_bytes.to_string(),
    );
    cells.insert(
        "device_bindings".to_owned(),
        sample.owners.device_bindings.to_string(),
    );
    cells.insert(
        "stale_device_rejections".to_owned(),
        sample.owners.stale_device_rejections.to_string(),
    );
    EditorRow {
        key: sample.sequence.to_string(),
        cells,
    }
}

fn validate_request_owner(
    engine: EngineId,
    request: &EditorRequestKind,
) -> Result<(), EditorClientError> {
    let valid = match request {
        EditorRequestKind::Refresh { .. }
        | EditorRequestKind::Control { .. }
        | EditorRequestKind::CancelPick { .. } => true,
        EditorRequestKind::Select {
            world, entities, ..
        } => world.engine == engine && entities.iter().all(|entity| entity.world == *world),
        EditorRequestKind::ApplyEdit(transaction) => transaction.engine == engine,
        EditorRequestKind::Undo { world, .. } | EditorRequestKind::Redo { world, .. } => {
            world.engine == engine
        }
        EditorRequestKind::Pick(request) => {
            request.engine == engine && request.world.engine == engine
        }
        EditorRequestKind::TranslateGizmo { world, .. } => world.engine == engine,
        EditorRequestKind::StartPlayMode {
            authoring_world,
            resources,
            ..
        } => {
            authoring_world.engine == engine
                && resources.world.engine != engine
                && resources.input_scope.engine == resources.world.engine
                && resources.render_target.engine == resources.world.engine
                && resources.asset_scope.engine == resources.world.engine
        }
        EditorRequestKind::ExitPlayMode { id, .. } => id.editor_engine == engine,
    };
    if valid {
        Ok(())
    } else {
        Err(EditorClientError::WrongEngine)
    }
}

fn request_bytes(request: &EditorRequestKind) -> Option<usize> {
    match request {
        EditorRequestKind::Refresh { .. } => Some(40),
        EditorRequestKind::Select { entities, .. } => {
            48_usize.checked_add(entities.len().checked_mul(24)?)
        }
        EditorRequestKind::ApplyEdit(transaction) => {
            transaction.edits.iter().try_fold(48_usize, |bytes, edit| {
                bytes.checked_add(32)?.checked_add(edit.value.len())
            })
        }
        EditorRequestKind::Undo { .. } | EditorRequestKind::Redo { .. } => Some(40),
        EditorRequestKind::Control { .. } => Some(32),
        EditorRequestKind::Pick(_) => Some(72),
        EditorRequestKind::CancelPick { .. } => Some(24),
        EditorRequestKind::TranslateGizmo { .. } => Some(144),
        EditorRequestKind::StartPlayMode { policy, .. } => {
            128_usize.checked_add(policy.writeback_fields.len().checked_mul(24)?)
        }
        EditorRequestKind::ExitPlayMode { .. } => Some(48),
    }
}

fn encoded_row_bytes(row: &EditorRow) -> Option<usize> {
    let initial = 8_usize.checked_add(row.key.len())?;
    row.cells.iter().try_fold(initial, |bytes, (key, value)| {
        bytes
            .checked_add(8)?
            .checked_add(key.len())?
            .checked_add(value.len())
    })
}

fn format_vector(vector: [i64; 3]) -> String {
    format!("{},{},{}", vector[0], vector[1], vector[2])
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}

fn hex_preview(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    let truncated = bytes.len() > max_bytes;
    (hex(&bytes[..bytes.len().min(max_bytes)]), truncated)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, EditorClientError> {
    if value.len() % 2 != 0 {
        return Err(EditorClientError::ResponseMismatch);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |value: u8| match value {
                b'0'..=b'9' => Some(value - b'0'),
                b'a'..=b'f' => Some(value - b'a' + 10),
                b'A'..=b'F' => Some(value - b'A' + 10),
                _ => None,
            };
            let high = digit(pair[0]).ok_or(EditorClientError::ResponseMismatch)?;
            let low = digit(pair[1]).ok_or(EditorClientError::ResponseMismatch)?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = (*hash).wrapping_mul(0x100000001b3);
    }
}

fn inspector_source_revision(
    world_revision: u64,
    selection_revision: u64,
    schema_fingerprint: u64,
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    hash_bytes(&mut hash, b"voplay.inspector-source/1");
    hash_bytes(&mut hash, &world_revision.to_le_bytes());
    hash_bytes(&mut hash, &selection_revision.to_le_bytes());
    hash_bytes(&mut hash, &schema_fingerprint.to_le_bytes());
    hash
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use vogui_protocol::v2::Handle as UiHandle;
    use vogui_runtime::tree::ViewKind;
    use vogui_runtime::{RootView, UiSession, UiSessionConfig};
    use voplay_protocol::Handle;
    use voplay_runtime::asset::{ArtifactId, AssetId, AssetRef, CpuNodeState};
    use voplay_runtime::editor_inspection::EditorPanelKind;
    use voplay_runtime::hierarchy::Transform;
    use voplay_runtime::inspection::{InspectedComponent, InspectedEntity};
    use voplay_runtime::schedule::{AccessSet, Stage};

    use super::*;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn ui_handle(index: u32) -> UiHandle {
        UiHandle {
            index,
            generation: 1,
        }
    }

    fn engine() -> EngineId {
        handle(1)
    }

    fn schemas(client: &mut EditorClient) {
        for panel in EditorPanel::ALL {
            client
                .register_schema(EditorPanelSchema::canonical(panel))
                .expect("schema");
        }
        client.freeze_schemas().expect("freeze");
    }

    fn client() -> EditorClient {
        let mut client =
            EditorClient::new(engine(), EditorClientConfig::default()).expect("client");
        schemas(&mut client);
        client
    }

    #[test]
    fn schemas_freeze_and_panel_views_commit_through_vogui_runtime() {
        let mut client = client();
        let world = WorldId {
            engine: engine(),
            handle: handle(2),
        };
        client
            .ingest_hierarchy(
                0,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Hierarchy,
                    source_revision: 7,
                    entries: vec![HierarchyInspectionNode {
                        entity: WorldEntity {
                            world,
                            entity: handle(3),
                        },
                        parent: None,
                        local: Transform::default(),
                        world: Transform::default(),
                        child_count: 0,
                    }],
                    next_cursor: None,
                    encoded_bytes: 88,
                    dropped_before: 0,
                },
            )
            .expect("page");
        let view = client
            .build_panel_view(EditorPanel::Hierarchy)
            .expect("view");
        assert_eq!(view.children.len(), 1);
        assert_eq!(
            view.kind,
            ViewKind::Element("voplay-editor-panel".to_owned())
        );

        let mut ui = UiSession::new(ui_handle(9), UiSessionConfig::default()).expect("ui");
        let root = ui.attach_root().expect("root");
        let commit = ui
            .commit_views(vec![RootView { root, view }])
            .expect("commit");
        assert_eq!(commit.transactions.len(), 1);
        assert!(!commit.transactions[0].1.patches.is_empty());
        assert_eq!(
            client.register_schema(EditorPanelSchema {
                panel: EditorPanel::Hierarchy,
                schema_type: 999,
                schema_version: 1,
                title: "late".to_owned(),
                columns: vec!["x".to_owned()],
            }),
            Err(EditorClientError::RegistryFrozen)
        );
    }

    #[test]
    fn all_panel_pages_are_typed_owner_checked_and_render_stable_rows() {
        let mut client = client();
        let world = WorldId {
            engine: engine(),
            handle: handle(2),
        };
        client
            .ingest_inspector(
                0,
                InspectionWorldPage {
                    world,
                    revision: 1,
                    source_revision: 1,
                    schema_fingerprint: 77,
                    component_schemas: Vec::new(),
                    entities: vec![InspectedEntity {
                        entity: WorldEntity {
                            world,
                            entity: handle(3),
                        },
                        stable_key: 9,
                        components: vec![InspectedComponent {
                            component: 7,
                            value: vec![1, 2, 3],
                        }],
                    }],
                    next_cursor: None,
                    encoded_bytes: 35,
                },
            )
            .expect("inspector");
        client
            .ingest_assets(
                0,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Assets,
                    source_revision: 1,
                    entries: vec![AssetInspectionEntry {
                        asset_ref: AssetRef {
                            engine: engine(),
                            handle: handle(4),
                        },
                        asset_id: AssetId([1; 16]),
                        asset_type: 8,
                        source_revision: 1,
                        artifact_id: ArtifactId([2; 16]),
                        dependencies: vec![],
                        state: CpuNodeState::CpuReady,
                        lease_count: 1,
                    }],
                    next_cursor: None,
                    encoded_bytes: 88,
                    dropped_before: 0,
                },
            )
            .expect("assets");
        client
            .ingest_schedule(
                0,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Schedule,
                    source_revision: 2,
                    entries: vec![SystemSpec {
                        name: "gameplay".to_owned(),
                        stage: Stage::Gameplay,
                        deterministic: true,
                        access: AccessSet {
                            simulation_reads: BTreeSet::from([1]),
                            ..AccessSet::default()
                        },
                        before: vec![],
                        after: vec![],
                    }],
                    next_cursor: None,
                    encoded_bytes: 48,
                    dropped_before: 0,
                },
            )
            .expect("schedule");
        client
            .ingest_render(
                0,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Render,
                    source_revision: 3,
                    entries: vec![RenderPanelEntry::Node {
                        position: 0,
                        node: voplay_runtime::render_graph::GraphNodeId(1),
                    }],
                    next_cursor: None,
                    encoded_bytes: 16,
                    dropped_before: 0,
                },
            )
            .expect("render");
        client
            .ingest_performance(
                0,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Performance,
                    source_revision: 4,
                    entries: vec![PerfPanelSample {
                        engine: engine(),
                        sequence: 4,
                        tick: 10,
                        system: "gameplay".to_owned(),
                        duration_nanos: 100,
                        allocation_bytes: 0,
                        lock_wait_nanos: 0,
                        queue_items: 0,
                        queue_bytes: 0,
                        error_count: 0,
                        owners: Default::default(),
                    }],
                    next_cursor: None,
                    encoded_bytes: 52,
                    dropped_before: 0,
                },
            )
            .expect("performance");
        for panel in [
            EditorPanel::Inspector,
            EditorPanel::Assets,
            EditorPanel::Schedule,
            EditorPanel::Render,
            EditorPanel::Performance,
        ] {
            let view = client.build_panel_view(panel).expect("view");
            assert_eq!(view.children.len(), 1);
        }
        assert_eq!(
            client.ingest_render(
                0,
                EditorPanelPage {
                    engine: handle(9),
                    panel: EditorPanelKind::Render,
                    source_revision: 5,
                    entries: vec![],
                    next_cursor: None,
                    encoded_bytes: 0,
                    dropped_before: 0,
                }
            ),
            Err(EditorClientError::WrongEngine)
        );
    }

    #[test]
    fn pagination_rejects_stale_or_skipped_pages_without_changing_panel() {
        let mut client = client();
        let first = EditorPanelPage {
            engine: engine(),
            panel: EditorPanelKind::Render,
            source_revision: 7,
            entries: vec![RenderPanelEntry::Node {
                position: 0,
                node: voplay_runtime::render_graph::GraphNodeId(1),
            }],
            next_cursor: Some(1),
            encoded_bytes: 16,
            dropped_before: 0,
        };
        client.ingest_render(0, first).expect("first");
        let before = client.panel(EditorPanel::Render).cloned();
        assert_eq!(
            client.ingest_render(
                1,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Render,
                    source_revision: 8,
                    entries: vec![],
                    next_cursor: None,
                    encoded_bytes: 0,
                    dropped_before: 0,
                }
            ),
            Err(EditorClientError::StalePage)
        );
        assert_eq!(client.panel(EditorPanel::Render).cloned(), before);
        assert_eq!(
            client.ingest_rows(
                EditorPanel::Render,
                0,
                9,
                None,
                0,
                vec![EditorRow {
                    key: "bad".to_owned(),
                    cells: std::collections::BTreeMap::from([(
                        "kind".to_owned(),
                        "node".to_owned(),
                    )]),
                }],
            ),
            Err(EditorClientError::RowSchemaMismatch)
        );
        assert_eq!(client.panel(EditorPanel::Render).cloned(), before);
        assert_eq!(
            client.ingest_render(
                2,
                EditorPanelPage {
                    engine: engine(),
                    panel: EditorPanelKind::Render,
                    source_revision: 7,
                    entries: vec![],
                    next_cursor: None,
                    encoded_bytes: 0,
                    dropped_before: 0,
                }
            ),
            Err(EditorClientError::InvalidCursor)
        );
    }

    #[test]
    fn typed_request_queue_preserves_owner_revision_and_restart_outcomes() {
        let mut client = client();
        let world = WorldId {
            engine: engine(),
            handle: handle(2),
        };
        let select = client
            .enqueue_request(EditorRequestKind::Select {
                world,
                expected_selection_revision: 3,
                entities: vec![WorldEntity {
                    world,
                    entity: handle(4),
                }],
            })
            .expect("select");
        let refresh = client
            .enqueue_request(EditorRequestKind::Refresh {
                panel: EditorPanel::Hierarchy,
                cursor: 0,
                expected_source_revision: Some(7),
            })
            .expect("refresh");
        assert_eq!(client.poll_request().expect("poll").request_id, select);
        assert_eq!(
            client.complete_request(EditorResponse {
                request_id: select,
                engine: handle(9),
                outcome: EditorResponseOutcome::Applied { revision: 4 },
            }),
            Err(EditorClientError::WrongEngine)
        );
        assert_eq!(
            client.cancel_request(select),
            Err(EditorClientError::RequestAlreadyDispatched)
        );
        let outcomes = client.restart_endpoint();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.contains(&EditorResponse {
            request_id: select,
            engine: engine(),
            outcome: EditorResponseOutcome::OutcomeUnknownOnRestart,
        }));
        assert!(outcomes.contains(&EditorResponse {
            request_id: refresh,
            engine: engine(),
            outcome: EditorResponseOutcome::DroppedBeforeDispatch,
        }));
        assert_eq!(
            client.enqueue_request(EditorRequestKind::Select {
                world: WorldId {
                    engine: handle(9),
                    handle: handle(2),
                },
                expected_selection_revision: 0,
                entities: vec![],
            }),
            Err(EditorClientError::WrongEngine)
        );
    }
}
