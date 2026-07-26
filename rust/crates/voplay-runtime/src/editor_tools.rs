use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::EngineId;

use crate::inspection::{InspectionEditTransaction, InspectionFieldEdit};
use crate::render_graph::{CompiledRenderGraph, GraphNodeId, GraphResourceId};
use crate::world::{World, WorldEntity, WorldId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorToolConfig {
    pub max_selection: usize,
    pub max_pending_picks: usize,
    pub max_pick_candidates: usize,
    pub max_captures: usize,
    pub max_capture_nodes: usize,
    pub max_capture_diagnostics: usize,
    pub max_capture_bytes: usize,
}

impl Default for EditorToolConfig {
    fn default() -> Self {
        Self {
            max_selection: 4096,
            max_pending_picks: 64,
            max_pick_candidates: 4096,
            max_captures: 16,
            max_capture_nodes: 4096,
            max_capture_diagnostics: 4096,
            max_capture_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionSnapshot {
    pub engine: EngineId,
    pub world: Option<WorldId>,
    pub revision: u64,
    pub entities: Vec<WorldEntity>,
    pub stable_keys: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PickRequest {
    pub engine: EngineId,
    pub request_id: u64,
    pub world: WorldId,
    pub expected_world_revision: u64,
    pub render_signature: u64,
    pub position: [u32; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PickCandidate {
    pub entity: WorldEntity,
    pub depth: u64,
    pub primitive: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickResult {
    pub request_id: u64,
    pub selected: Option<WorldEntity>,
    pub selection_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTiming {
    pub node: GraphNodeId,
    pub duration_nanos: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderDiagnostic {
    pub node: GraphNodeId,
    pub severity: ShaderDiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameDebugCaptureInput {
    pub engine: EngineId,
    pub capture_id: u64,
    pub render_signature: u64,
    pub timings: Vec<FrameTiming>,
    pub diagnostics: Vec<ShaderDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameDebugCapture {
    pub engine: EngineId,
    pub capture_id: u64,
    pub render_signature: u64,
    pub ordered_nodes: Vec<GraphNodeId>,
    pub final_versions: Vec<(GraphResourceId, u32)>,
    pub allocations: Vec<(GraphResourceId, u32)>,
    pub timings: Vec<FrameTiming>,
    pub diagnostics: Vec<ShaderDiagnostic>,
    pub encoded_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorToolError {
    InvalidConfig,
    WrongEngine,
    WrongWorld,
    InvalidEntity,
    SelectionCapacity,
    RevisionConflict,
    RevisionExhausted,
    PickCapacity,
    InvalidPick,
    DuplicatePick,
    UnknownPick,
    PickCandidateCapacity,
    RenderSignatureMismatch,
    InvalidGizmo,
    ComponentMissing,
    ComponentShape,
    TransformOverflow,
    CaptureCapacity,
    CaptureSequence,
    CaptureNodeCapacity,
    CaptureDiagnosticCapacity,
    CaptureByteCapacity,
    InvalidCaptureNode,
    DuplicateTiming,
}

pub struct EditorToolService {
    engine: EngineId,
    config: EditorToolConfig,
    selection_world: Option<WorldId>,
    selection: BTreeSet<WorldEntity>,
    selection_stable_keys: BTreeSet<u64>,
    selection_revision: u64,
    pending_picks: BTreeMap<u64, PickRequest>,
    last_pick_request: u64,
    captures: VecDeque<FrameDebugCapture>,
    last_capture_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorToolShutdownReport {
    pub released_selection: usize,
    pub cancelled_picks: usize,
    pub released_captures: usize,
}

impl EditorToolService {
    pub fn new(engine: EngineId, config: EditorToolConfig) -> Result<Self, EditorToolError> {
        if !engine.is_valid() {
            return Err(EditorToolError::WrongEngine);
        }
        if config.max_selection == 0
            || config.max_pending_picks == 0
            || config.max_pick_candidates == 0
            || config.max_captures == 0
            || config.max_capture_nodes == 0
            || config.max_capture_diagnostics == 0
            || config.max_capture_bytes == 0
        {
            return Err(EditorToolError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            selection_world: None,
            selection: BTreeSet::new(),
            selection_stable_keys: BTreeSet::new(),
            selection_revision: 0,
            pending_picks: BTreeMap::new(),
            last_pick_request: 0,
            captures: VecDeque::new(),
            last_capture_id: 0,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn selection(&self) -> SelectionSnapshot {
        SelectionSnapshot {
            engine: self.engine,
            world: self.selection_world,
            revision: self.selection_revision,
            entities: self.selection.iter().copied().collect(),
            stable_keys: self.selection_stable_keys.iter().copied().collect(),
        }
    }

    pub fn pending_pick_count(&self) -> usize {
        self.pending_picks.len()
    }

    pub fn replace_selection(
        &mut self,
        world: &World,
        entities: Vec<WorldEntity>,
        expected_selection_revision: u64,
    ) -> Result<SelectionSnapshot, EditorToolError> {
        self.validate_world(world)?;
        self.expect_selection_revision(expected_selection_revision)?;
        if entities.len() > self.config.max_selection {
            return Err(EditorToolError::SelectionCapacity);
        }
        let selection = entities.into_iter().collect::<BTreeSet<_>>();
        if selection.len() > self.config.max_selection {
            return Err(EditorToolError::SelectionCapacity);
        }
        if selection
            .iter()
            .any(|entity| entity.world != world.id() || !world.is_live(*entity))
        {
            return Err(EditorToolError::InvalidEntity);
        }
        let stable_keys = selection
            .iter()
            .map(|entity| {
                world
                    .stable_key(*entity)
                    .ok_or(EditorToolError::InvalidEntity)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if self.selection_world == Some(world.id())
            && self.selection == selection
            && self.selection_stable_keys == stable_keys
        {
            return Ok(self.selection());
        }
        let next_revision = self.next_selection_revision()?;
        self.selection_world = Some(world.id());
        self.selection = selection;
        self.selection_stable_keys = stable_keys;
        self.selection_revision = next_revision;
        Ok(self.selection())
    }

    pub fn reconcile_selection(
        &mut self,
        world: &World,
    ) -> Result<SelectionSnapshot, EditorToolError> {
        self.validate_world(world)?;
        if self.selection_world != Some(world.id()) {
            return Ok(self.selection());
        }
        let selection = self
            .selection_stable_keys
            .iter()
            .filter_map(|stable_key| world.entity_for_stable_key(*stable_key))
            .collect::<BTreeSet<_>>();
        if selection == self.selection {
            return Ok(self.selection());
        }
        let next_revision = self.next_selection_revision()?;
        self.selection = selection;
        self.selection_revision = next_revision;
        Ok(self.selection())
    }

    pub fn submit_pick(
        &mut self,
        world: &World,
        graph: &CompiledRenderGraph,
        request: PickRequest,
    ) -> Result<(), EditorToolError> {
        self.validate_world(world)?;
        if request.engine != self.engine || request.world != world.id() {
            return Err(EditorToolError::WrongEngine);
        }
        if request.request_id == 0 || request.request_id <= self.last_pick_request {
            return Err(EditorToolError::DuplicatePick);
        }
        if request.expected_world_revision != world.revision() {
            return Err(EditorToolError::RevisionConflict);
        }
        if request.render_signature != graph.signature {
            return Err(EditorToolError::RenderSignatureMismatch);
        }
        if self.pending_picks.len() == self.config.max_pending_picks {
            return Err(EditorToolError::PickCapacity);
        }
        self.last_pick_request = request.request_id;
        self.pending_picks.insert(request.request_id, request);
        Ok(())
    }

    pub fn complete_pick(
        &mut self,
        world: &World,
        graph: &CompiledRenderGraph,
        request_id: u64,
        mut candidates: Vec<PickCandidate>,
    ) -> Result<PickResult, EditorToolError> {
        self.validate_world(world)?;
        let request = *self
            .pending_picks
            .get(&request_id)
            .ok_or(EditorToolError::UnknownPick)?;
        if request.expected_world_revision != world.revision() {
            return Err(EditorToolError::RevisionConflict);
        }
        if request.render_signature != graph.signature {
            return Err(EditorToolError::RenderSignatureMismatch);
        }
        if candidates.len() > self.config.max_pick_candidates {
            return Err(EditorToolError::PickCandidateCapacity);
        }
        if candidates.iter().any(|candidate| {
            candidate.entity.world != world.id() || !world.is_live(candidate.entity)
        }) {
            return Err(EditorToolError::InvalidEntity);
        }
        candidates.sort_by_key(|candidate| {
            (
                candidate.depth,
                candidate.entity.entity.index,
                candidate.entity.entity.generation,
                candidate.primitive,
            )
        });
        let selected = candidates.first().map(|candidate| candidate.entity);
        let new_selection = selected.into_iter().collect::<BTreeSet<_>>();
        let new_stable_keys = new_selection
            .iter()
            .map(|entity| {
                world
                    .stable_key(*entity)
                    .ok_or(EditorToolError::InvalidEntity)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let changed = self.selection_world != Some(world.id())
            || self.selection != new_selection
            || self.selection_stable_keys != new_stable_keys;
        let next_revision = changed
            .then(|| self.next_selection_revision())
            .transpose()?;
        self.pending_picks.remove(&request_id);
        if let Some(next_revision) = next_revision {
            self.selection_world = Some(world.id());
            self.selection = new_selection;
            self.selection_stable_keys = new_stable_keys;
            self.selection_revision = next_revision;
        }
        Ok(PickResult {
            request_id,
            selected,
            selection_revision: self.selection_revision,
        })
    }

    pub fn cancel_pick(&mut self, request_id: u64) -> Result<PickRequest, EditorToolError> {
        self.pending_picks
            .remove(&request_id)
            .ok_or(EditorToolError::UnknownPick)
    }

    pub fn cancel_all_picks(&mut self) -> Vec<PickRequest> {
        std::mem::take(&mut self.pending_picks)
            .into_values()
            .collect()
    }

    pub fn shutdown(&mut self) -> EditorToolShutdownReport {
        let report = EditorToolShutdownReport {
            released_selection: self.selection.len(),
            cancelled_picks: self.pending_picks.len(),
            released_captures: self.captures.len(),
        };
        self.selection_world = None;
        self.selection.clear();
        self.selection_stable_keys.clear();
        self.pending_picks.clear();
        self.captures.clear();
        report
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_translation_edit(
        &self,
        world: &World,
        transaction_id: u64,
        expected_selection_revision: u64,
        expected_world_revision: u64,
        component: u32,
        fields: [u32; 3],
        offsets: [usize; 3],
        delta: [i64; 3],
    ) -> Result<InspectionEditTransaction, EditorToolError> {
        self.validate_world(world)?;
        self.expect_selection_revision(expected_selection_revision)?;
        if self.selection_world != Some(world.id()) {
            return Err(EditorToolError::WrongWorld);
        }
        let mut ranges = offsets
            .into_iter()
            .map(|offset| {
                offset
                    .checked_add(8)
                    .map(|end| (offset, end))
                    .ok_or(EditorToolError::ComponentShape)
            })
            .collect::<Result<Vec<_>, _>>()?;
        ranges.sort_unstable();
        if transaction_id == 0
            || component == 0
            || fields.contains(&0)
            || fields.into_iter().collect::<BTreeSet<_>>().len() != 3
            || ranges.windows(2).any(|pair| pair[0].1 > pair[1].0)
        {
            return Err(EditorToolError::InvalidGizmo);
        }
        if expected_world_revision != world.revision() {
            return Err(EditorToolError::RevisionConflict);
        }
        let mut edits = Vec::with_capacity(self.selection.len().saturating_mul(3));
        for entity in &self.selection {
            let value = world
                .component(*entity, component)
                .ok_or(EditorToolError::ComponentMissing)?;
            for axis in 0..3 {
                let end = offsets[axis]
                    .checked_add(8)
                    .ok_or(EditorToolError::ComponentShape)?;
                let bytes: [u8; 8] = value
                    .get(offsets[axis]..end)
                    .ok_or(EditorToolError::ComponentShape)?
                    .try_into()
                    .map_err(|_| EditorToolError::ComponentShape)?;
                let translated = i64::from_le_bytes(bytes)
                    .checked_add(delta[axis])
                    .ok_or(EditorToolError::TransformOverflow)?;
                edits.push(InspectionFieldEdit {
                    entity: *entity,
                    component,
                    field: fields[axis],
                    value: translated.to_le_bytes().to_vec(),
                });
            }
        }
        if edits.is_empty() {
            return Err(EditorToolError::InvalidGizmo);
        }
        Ok(InspectionEditTransaction {
            engine: self.engine,
            transaction_id,
            expected_world_revision,
            edits,
        })
    }

    pub fn capture_frame(
        &mut self,
        graph: &CompiledRenderGraph,
        input: FrameDebugCaptureInput,
    ) -> Result<FrameDebugCapture, EditorToolError> {
        if input.engine != self.engine {
            return Err(EditorToolError::WrongEngine);
        }
        if input.capture_id == 0 || input.capture_id <= self.last_capture_id {
            return Err(EditorToolError::CaptureSequence);
        }
        if input.render_signature != graph.signature {
            return Err(EditorToolError::RenderSignatureMismatch);
        }
        if graph.ordered_nodes.len() > self.config.max_capture_nodes {
            return Err(EditorToolError::CaptureNodeCapacity);
        }
        if input.diagnostics.len() > self.config.max_capture_diagnostics {
            return Err(EditorToolError::CaptureDiagnosticCapacity);
        }
        let nodes = graph.ordered_nodes.iter().copied().collect::<BTreeSet<_>>();
        let timing_nodes = input
            .timings
            .iter()
            .map(|timing| timing.node)
            .collect::<BTreeSet<_>>();
        if timing_nodes.len() != input.timings.len() {
            return Err(EditorToolError::DuplicateTiming);
        }
        if input
            .timings
            .iter()
            .any(|timing| !nodes.contains(&timing.node))
            || input
                .diagnostics
                .iter()
                .any(|diagnostic| !nodes.contains(&diagnostic.node))
        {
            return Err(EditorToolError::InvalidCaptureNode);
        }
        let encoded_bytes = capture_bytes(graph, &input)?;
        if encoded_bytes > self.config.max_capture_bytes {
            return Err(EditorToolError::CaptureByteCapacity);
        }
        let mut timings = input.timings;
        timings.sort_by_key(|timing| timing.node);
        let mut diagnostics = input.diagnostics;
        diagnostics.sort_by(|left, right| {
            (
                left.node,
                severity_tag(left.severity),
                left.message.as_str(),
            )
                .cmp(&(
                    right.node,
                    severity_tag(right.severity),
                    right.message.as_str(),
                ))
        });
        let capture = FrameDebugCapture {
            engine: self.engine,
            capture_id: input.capture_id,
            render_signature: graph.signature,
            ordered_nodes: graph.ordered_nodes.clone(),
            final_versions: graph
                .final_versions
                .iter()
                .map(|(resource, version)| (*resource, *version))
                .collect(),
            allocations: graph
                .allocations
                .iter()
                .map(|(resource, slot)| (*resource, *slot))
                .collect(),
            timings,
            diagnostics,
            encoded_bytes,
        };
        if self.captures.len() == self.config.max_captures {
            self.captures.pop_front();
        }
        self.last_capture_id = capture.capture_id;
        self.captures.push_back(capture.clone());
        Ok(capture)
    }

    pub fn frame_capture(&self, capture_id: u64) -> Option<&FrameDebugCapture> {
        self.captures
            .iter()
            .find(|capture| capture.capture_id == capture_id)
    }

    fn validate_world(&self, world: &World) -> Result<(), EditorToolError> {
        if world.id().engine == self.engine {
            Ok(())
        } else {
            Err(EditorToolError::WrongEngine)
        }
    }

    fn expect_selection_revision(&self, expected: u64) -> Result<(), EditorToolError> {
        if expected == self.selection_revision {
            Ok(())
        } else {
            Err(EditorToolError::RevisionConflict)
        }
    }

    fn next_selection_revision(&self) -> Result<u64, EditorToolError> {
        self.selection_revision
            .checked_add(1)
            .ok_or(EditorToolError::RevisionExhausted)
    }
}

fn capture_bytes(
    graph: &CompiledRenderGraph,
    input: &FrameDebugCaptureInput,
) -> Result<usize, EditorToolError> {
    let mut bytes = 48_usize
        .checked_add(
            graph
                .ordered_nodes
                .len()
                .checked_mul(4)
                .ok_or(EditorToolError::CaptureByteCapacity)?,
        )
        .and_then(|bytes| bytes.checked_add(graph.final_versions.len().checked_mul(8)?))
        .and_then(|bytes| bytes.checked_add(graph.allocations.len().checked_mul(8)?))
        .and_then(|bytes| bytes.checked_add(input.timings.len().checked_mul(12)?))
        .ok_or(EditorToolError::CaptureByteCapacity)?;
    for diagnostic in &input.diagnostics {
        bytes = bytes
            .checked_add(8)
            .and_then(|bytes| bytes.checked_add(diagnostic.message.len()))
            .ok_or(EditorToolError::CaptureByteCapacity)?;
    }
    Ok(bytes)
}

const fn severity_tag(severity: ShaderDiagnosticSeverity) -> u8 {
    match severity {
        ShaderDiagnosticSeverity::Info => 1,
        ShaderDiagnosticSeverity::Warning => 2,
        ShaderDiagnosticSeverity::Error => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspection::{
        InspectionComponentSchema, InspectionConfig, InspectionFieldSchema,
        InspectionSchemaRegistry, InspectionService,
    };
    use crate::render_graph::GraphResourceId;
    use crate::world::{WorldCommand, WorldConfig};
    use voplay_protocol::Handle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn engine() -> EngineId {
        handle(1)
    }

    fn world() -> World {
        World::new(
            WorldId {
                engine: engine(),
                handle: handle(2),
            },
            WorldConfig::default(),
        )
        .expect("world")
    }

    fn graph(signature: u64) -> CompiledRenderGraph {
        CompiledRenderGraph {
            ordered_nodes: vec![GraphNodeId(1), GraphNodeId(2)],
            final_versions: BTreeMap::from([(GraphResourceId(1), 1)]),
            allocations: BTreeMap::from([(GraphResourceId(1), 3)]),
            signature,
        }
    }

    fn spawn(world: &mut World, stable_key: u64, translation: [i64; 3]) -> WorldEntity {
        let mut value = Vec::new();
        for axis in translation {
            value.extend_from_slice(&axis.to_le_bytes());
        }
        world
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key,
                components: BTreeMap::from([(7, value)]),
            }])
            .expect("spawn")
            .spawned[0]
            .1
    }

    #[test]
    fn pick_binds_world_and_render_revision_and_selects_stable_nearest_candidate() {
        let mut world = world();
        let first = spawn(&mut world, 1, [0, 0, 0]);
        let second = spawn(&mut world, 2, [0, 0, 0]);
        let graph = graph(9);
        let mut tools =
            EditorToolService::new(engine(), EditorToolConfig::default()).expect("tools");
        let request = PickRequest {
            engine: engine(),
            request_id: 1,
            world: world.id(),
            expected_world_revision: world.revision(),
            render_signature: graph.signature,
            position: [10, 20],
        };
        tools.submit_pick(&world, &graph, request).expect("submit");
        let result = tools
            .complete_pick(
                &world,
                &graph,
                1,
                vec![
                    PickCandidate {
                        entity: second,
                        depth: 5,
                        primitive: 2,
                    },
                    PickCandidate {
                        entity: first,
                        depth: 5,
                        primitive: 3,
                    },
                ],
            )
            .expect("complete");
        assert_eq!(result.selected, Some(first));
        assert_eq!(tools.selection().entities, vec![first]);

        let request = PickRequest {
            request_id: 2,
            expected_world_revision: world.revision(),
            ..request
        };
        tools
            .submit_pick(&world, &graph, request)
            .expect("submit 2");
        spawn(&mut world, 3, [0, 0, 0]);
        assert_eq!(
            tools.complete_pick(&world, &graph, 2, vec![]),
            Err(EditorToolError::RevisionConflict)
        );
        assert_eq!(tools.pending_pick_count(), 1);
        assert_eq!(tools.cancel_pick(2), Ok(request));
        assert_eq!(tools.pending_pick_count(), 0);
    }

    #[test]
    fn gizmo_builds_inspection_transaction_and_applies_at_world_stage() {
        let mut world = world();
        let entity = spawn(&mut world, 1, [10, 20, 30]);
        let mut tools =
            EditorToolService::new(engine(), EditorToolConfig::default()).expect("tools");
        tools
            .replace_selection(&world, vec![entity], 0)
            .expect("selection");
        let transaction = tools
            .build_translation_edit(
                &world,
                1,
                tools.selection().revision,
                world.revision(),
                7,
                [1, 2, 3],
                [0, 8, 16],
                [1, -2, 3],
            )
            .expect("gizmo");
        assert_eq!(
            tools.build_translation_edit(
                &world,
                2,
                tools.selection().revision,
                world.revision(),
                7,
                [1, 2, 3],
                [0, 4, 16],
                [1, 1, 1],
            ),
            Err(EditorToolError::InvalidGizmo)
        );
        let mut schemas =
            InspectionSchemaRegistry::new(engine(), InspectionConfig::default()).expect("schemas");
        schemas
            .register(InspectionComponentSchema {
                component: 7,
                type_id: 70,
                version: 1,
                name: "Transform".to_owned(),
                fields: vec![
                    InspectionFieldSchema {
                        field: 1,
                        name: "x".to_owned(),
                        offset: 0,
                        width: 8,
                        editable: true,
                    },
                    InspectionFieldSchema {
                        field: 2,
                        name: "y".to_owned(),
                        offset: 8,
                        width: 8,
                        editable: true,
                    },
                    InspectionFieldSchema {
                        field: 3,
                        name: "z".to_owned(),
                        offset: 16,
                        width: 8,
                        editable: true,
                    },
                ],
            })
            .expect("schema");
        schemas.freeze().expect("freeze");
        let mut inspection =
            InspectionService::new(engine(), InspectionConfig::default()).expect("inspection");
        inspection
            .apply_edit_at_stage(&mut world, &schemas, transaction)
            .expect("apply");
        let value = world.component(entity, 7).expect("component");
        assert_eq!(i64::from_le_bytes(value[0..8].try_into().unwrap()), 11);
        assert_eq!(i64::from_le_bytes(value[8..16].try_into().unwrap()), 18);
        assert_eq!(i64::from_le_bytes(value[16..24].try_into().unwrap()), 33);
    }

    #[test]
    fn frame_capture_validates_graph_nodes_and_commits_atomically() {
        let graph = graph(11);
        let mut tools =
            EditorToolService::new(engine(), EditorToolConfig::default()).expect("tools");
        let capture = tools
            .capture_frame(
                &graph,
                FrameDebugCaptureInput {
                    engine: engine(),
                    capture_id: 1,
                    render_signature: 11,
                    timings: vec![FrameTiming {
                        node: GraphNodeId(2),
                        duration_nanos: 20,
                    }],
                    diagnostics: vec![ShaderDiagnostic {
                        node: GraphNodeId(1),
                        severity: ShaderDiagnosticSeverity::Warning,
                        message: "slow path".to_owned(),
                    }],
                },
            )
            .expect("capture");
        assert_eq!(capture.ordered_nodes, vec![GraphNodeId(1), GraphNodeId(2)]);
        assert!(tools.frame_capture(1).is_some());
        assert_eq!(
            tools.capture_frame(
                &graph,
                FrameDebugCaptureInput {
                    engine: engine(),
                    capture_id: 2,
                    render_signature: 11,
                    timings: vec![FrameTiming {
                        node: GraphNodeId(9),
                        duration_nanos: 1,
                    }],
                    diagnostics: vec![],
                }
            ),
            Err(EditorToolError::InvalidCaptureNode)
        );
        assert!(tools.frame_capture(2).is_none());
    }

    #[test]
    fn quotas_and_foreign_entities_fail_before_state_changes() {
        let mut world = world();
        let first = spawn(&mut world, 1, [0, 0, 0]);
        let second = spawn(&mut world, 2, [0, 0, 0]);
        let mut tools = EditorToolService::new(
            engine(),
            EditorToolConfig {
                max_selection: 1,
                max_pending_picks: 1,
                max_pick_candidates: 1,
                max_captures: 1,
                max_capture_nodes: 1,
                max_capture_diagnostics: 1,
                max_capture_bytes: 64,
            },
        )
        .expect("tools");
        assert_eq!(
            tools.replace_selection(&world, vec![first, second], 0),
            Err(EditorToolError::SelectionCapacity)
        );
        assert_eq!(tools.selection().revision, 0);
        assert_eq!(
            tools.capture_frame(
                &graph(1),
                FrameDebugCaptureInput {
                    engine: engine(),
                    capture_id: 1,
                    render_signature: 1,
                    timings: vec![],
                    diagnostics: vec![],
                }
            ),
            Err(EditorToolError::CaptureNodeCapacity)
        );
        assert!(tools.frame_capture(1).is_none());
    }
}
