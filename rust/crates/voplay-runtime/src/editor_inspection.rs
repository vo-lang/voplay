use std::collections::VecDeque;

use voplay_protocol::EngineId;

use crate::asset::{AssetInspectionEntry, AssetServer};
use crate::hierarchy::{Hierarchy, HierarchyInspectionNode};
use crate::render_graph::{CompiledRenderGraph, GraphNodeId, GraphResourceId};
use crate::schedule::{Schedule, SystemSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorInspectionConfig {
    pub max_page_entries: usize,
    pub max_page_bytes: usize,
    pub max_perf_samples: usize,
    pub max_perf_label_bytes: usize,
}

impl Default for EditorInspectionConfig {
    fn default() -> Self {
        Self {
            max_page_entries: 1024,
            max_page_bytes: 4 * 1024 * 1024,
            max_perf_samples: 16_384,
            max_perf_label_bytes: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorPanelKind {
    Hierarchy,
    Assets,
    Schedule,
    Render,
    Performance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorPanelPage<T> {
    pub engine: EngineId,
    pub panel: EditorPanelKind,
    pub source_revision: u64,
    pub entries: Vec<T>,
    pub next_cursor: Option<usize>,
    pub encoded_bytes: usize,
    pub dropped_before: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorInspectionError {
    InvalidConfig,
    WrongEngine,
    InvalidCursor,
    PageCapacity,
    PageByteCapacity,
    StaleSourceRevision,
    InvalidPerfSample,
    PerfSequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPanelEntry {
    Node {
        position: u32,
        node: GraphNodeId,
    },
    FinalVersion {
        resource: GraphResourceId,
        version: u32,
    },
    Allocation {
        resource: GraphResourceId,
        slot: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PerfPanelSample {
    pub engine: EngineId,
    pub sequence: u64,
    pub tick: u64,
    pub system: String,
    pub duration_nanos: u64,
    pub allocation_bytes: u64,
    pub lock_wait_nanos: u64,
    pub queue_items: usize,
    pub queue_bytes: usize,
    pub error_count: u32,
    pub owners: RuntimeOwnerCounters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePerfMeasurement {
    pub tick: u64,
    pub system: String,
    pub duration_nanos: u64,
    pub allocation_bytes: u64,
    pub lock_wait_nanos: u64,
    pub queue_items: usize,
    pub queue_bytes: usize,
    pub error_count: u32,
    pub owners: RuntimeOwnerCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOwnerCounters {
    pub live_roles: usize,
    pub live_entities: usize,
    pub live_surfaces: usize,
    pub input_scopes: usize,
    pub pending_render_ops: usize,
    pub pending_render_bytes: usize,
    pub device_bindings: usize,
    pub stale_device_rejections: u64,
}

impl RuntimeOwnerCounters {
    pub fn capture<G, B, E>(
        runtime: &crate::runtime::VoplayRuntime<G, B, E>,
        device_hub: &crate::device_hub::DeviceHub,
    ) -> Self
    where
        G: crate::game_engine::Game,
        B: crate::surface::RenderBackend,
        E: crate::presentation::FrameCommandEncoder,
    {
        let runtime = runtime.owner_snapshot();
        let devices = device_hub.metrics();
        Self {
            live_roles: runtime.game.live_roles,
            live_entities: runtime.game.live_entities,
            live_surfaces: runtime.surfaces,
            input_scopes: runtime.input_scopes,
            pending_render_ops: runtime.game.render_outbox.pending_ops,
            pending_render_bytes: runtime.game.render_outbox.pending_bytes,
            device_bindings: devices.engine_bindings,
            stale_device_rejections: devices.stale_rejections,
        }
    }
}

pub struct EditorInspectionService {
    engine: EngineId,
    config: EditorInspectionConfig,
    perf_samples: VecDeque<PerfPanelSample>,
    last_perf_sequence: u64,
    dropped_perf_samples: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorInspectionShutdownReport {
    pub released_performance_samples: usize,
}

impl EditorInspectionService {
    pub fn new(
        engine: EngineId,
        config: EditorInspectionConfig,
    ) -> Result<Self, EditorInspectionError> {
        if !engine.is_valid() {
            return Err(EditorInspectionError::WrongEngine);
        }
        if config.max_page_entries == 0
            || config.max_page_bytes == 0
            || config.max_perf_samples == 0
            || config.max_perf_label_bytes == 0
        {
            return Err(EditorInspectionError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            perf_samples: VecDeque::new(),
            last_perf_sequence: 0,
            dropped_perf_samples: 0,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn performance_revision(&self) -> u64 {
        self.last_perf_sequence
    }

    pub fn shutdown(&mut self) -> EditorInspectionShutdownReport {
        let report = EditorInspectionShutdownReport {
            released_performance_samples: self.perf_samples.len(),
        };
        self.perf_samples.clear();
        report
    }

    pub fn hierarchy_page(
        &self,
        hierarchy: &Hierarchy,
        cursor: usize,
        limit: usize,
        expected_source_revision: Option<u64>,
    ) -> Result<EditorPanelPage<HierarchyInspectionNode>, EditorInspectionError> {
        if hierarchy.world().engine != self.engine {
            return Err(EditorInspectionError::WrongEngine);
        }
        let source_revision = hierarchy.inspection_revision();
        validate_revision(source_revision, expected_source_revision)?;
        paginate(
            self.engine,
            EditorPanelKind::Hierarchy,
            source_revision,
            hierarchy.inspection_nodes(),
            cursor,
            limit,
            self.config,
            0,
            |_| Some(88),
        )
    }

    pub fn assets_page(
        &self,
        assets: &AssetServer,
        cursor: usize,
        limit: usize,
        expected_source_revision: Option<u64>,
    ) -> Result<EditorPanelPage<AssetInspectionEntry>, EditorInspectionError> {
        if assets.engine() != self.engine {
            return Err(EditorInspectionError::WrongEngine);
        }
        let source_revision = assets.inspection_revision();
        validate_revision(source_revision, expected_source_revision)?;
        paginate(
            self.engine,
            EditorPanelKind::Assets,
            source_revision,
            assets.inspection_assets(),
            cursor,
            limit,
            self.config,
            0,
            |entry| 88_usize.checked_add(entry.dependencies.len().checked_mul(16)?),
        )
    }

    pub fn schedule_page(
        &self,
        owner: EngineId,
        schedule: &Schedule,
        cursor: usize,
        limit: usize,
        expected_source_revision: Option<u64>,
    ) -> Result<EditorPanelPage<SystemSpec>, EditorInspectionError> {
        self.validate_owner(owner)?;
        let source_revision = schedule.hash();
        validate_revision(source_revision, expected_source_revision)?;
        paginate(
            self.engine,
            EditorPanelKind::Schedule,
            source_revision,
            schedule.systems().to_vec(),
            cursor,
            limit,
            self.config,
            0,
            encoded_system_bytes,
        )
    }

    pub fn render_page(
        &self,
        owner: EngineId,
        graph: &CompiledRenderGraph,
        cursor: usize,
        limit: usize,
        expected_source_revision: Option<u64>,
    ) -> Result<EditorPanelPage<RenderPanelEntry>, EditorInspectionError> {
        self.validate_owner(owner)?;
        validate_revision(graph.signature, expected_source_revision)?;
        let mut entries = Vec::with_capacity(
            graph.ordered_nodes.len() + graph.final_versions.len() + graph.allocations.len(),
        );
        entries.extend(
            graph
                .ordered_nodes
                .iter()
                .enumerate()
                .map(|(position, node)| RenderPanelEntry::Node {
                    position: position as u32,
                    node: *node,
                }),
        );
        entries.extend(graph.final_versions.iter().map(|(resource, version)| {
            RenderPanelEntry::FinalVersion {
                resource: *resource,
                version: *version,
            }
        }));
        entries.extend(graph.allocations.iter().map(|(resource, slot)| {
            RenderPanelEntry::Allocation {
                resource: *resource,
                slot: *slot,
            }
        }));
        paginate(
            self.engine,
            EditorPanelKind::Render,
            graph.signature,
            entries,
            cursor,
            limit,
            self.config,
            0,
            |_| Some(16),
        )
    }

    pub fn publish_perf_sample(
        &mut self,
        sample: PerfPanelSample,
    ) -> Result<(), EditorInspectionError> {
        self.validate_owner(sample.engine)?;
        if sample.sequence == 0
            || sample.sequence <= self.last_perf_sequence
            || sample.system.is_empty()
            || sample.system.len() > self.config.max_perf_label_bytes
        {
            return Err(if sample.sequence <= self.last_perf_sequence {
                EditorInspectionError::PerfSequence
            } else {
                EditorInspectionError::InvalidPerfSample
            });
        }
        if self.perf_samples.len() == self.config.max_perf_samples {
            self.perf_samples.pop_front();
            self.dropped_perf_samples = self.dropped_perf_samples.saturating_add(1);
        }
        self.last_perf_sequence = sample.sequence;
        self.perf_samples.push_back(sample);
        Ok(())
    }

    pub fn record_perf_measurement(
        &mut self,
        measurement: RuntimePerfMeasurement,
    ) -> Result<u64, EditorInspectionError> {
        let sequence = self
            .last_perf_sequence
            .checked_add(1)
            .ok_or(EditorInspectionError::PerfSequence)?;
        self.publish_perf_sample(PerfPanelSample {
            engine: self.engine,
            sequence,
            tick: measurement.tick,
            system: measurement.system,
            duration_nanos: measurement.duration_nanos,
            allocation_bytes: measurement.allocation_bytes,
            lock_wait_nanos: measurement.lock_wait_nanos,
            queue_items: measurement.queue_items,
            queue_bytes: measurement.queue_bytes,
            error_count: measurement.error_count,
            owners: measurement.owners,
        })?;
        Ok(sequence)
    }

    pub fn performance_page(
        &self,
        cursor: usize,
        limit: usize,
        expected_source_revision: Option<u64>,
    ) -> Result<EditorPanelPage<PerfPanelSample>, EditorInspectionError> {
        validate_revision(self.last_perf_sequence, expected_source_revision)?;
        paginate(
            self.engine,
            EditorPanelKind::Performance,
            self.last_perf_sequence,
            self.perf_samples.iter().cloned().collect(),
            cursor,
            limit,
            self.config,
            self.dropped_perf_samples,
            |sample| 132_usize.checked_add(sample.system.len()),
        )
    }

    fn validate_owner(&self, owner: EngineId) -> Result<(), EditorInspectionError> {
        if owner == self.engine {
            Ok(())
        } else {
            Err(EditorInspectionError::WrongEngine)
        }
    }
}

fn validate_revision(actual: u64, expected: Option<u64>) -> Result<(), EditorInspectionError> {
    if expected.is_none_or(|expected| expected == actual) {
        Ok(())
    } else {
        Err(EditorInspectionError::StaleSourceRevision)
    }
}

#[allow(clippy::too_many_arguments)]
fn paginate<T: Clone>(
    engine: EngineId,
    panel: EditorPanelKind,
    source_revision: u64,
    entries: Vec<T>,
    cursor: usize,
    limit: usize,
    config: EditorInspectionConfig,
    dropped_before: u64,
    encoded_size: impl Fn(&T) -> Option<usize>,
) -> Result<EditorPanelPage<T>, EditorInspectionError> {
    if limit == 0 || limit > config.max_page_entries {
        return Err(EditorInspectionError::PageCapacity);
    }
    if cursor > entries.len() {
        return Err(EditorInspectionError::InvalidCursor);
    }
    let mut page_entries = Vec::new();
    let mut encoded_bytes = 0_usize;
    let mut index = cursor;
    while index < entries.len() && page_entries.len() < limit {
        let entry = &entries[index];
        let entry_bytes = encoded_size(entry).ok_or(EditorInspectionError::PageByteCapacity)?;
        if entry_bytes > config.max_page_bytes {
            return Err(EditorInspectionError::PageByteCapacity);
        }
        let Some(next_bytes) = encoded_bytes.checked_add(entry_bytes) else {
            return Err(EditorInspectionError::PageByteCapacity);
        };
        if next_bytes > config.max_page_bytes {
            break;
        }
        page_entries.push(entry.clone());
        encoded_bytes = next_bytes;
        index += 1;
    }
    Ok(EditorPanelPage {
        engine,
        panel,
        source_revision,
        entries: page_entries,
        next_cursor: (index < entries.len()).then_some(index),
        encoded_bytes,
        dropped_before,
    })
}

fn encoded_system_bytes(system: &SystemSpec) -> Option<usize> {
    let mut bytes = 32_usize.checked_add(system.name.len())?;
    for dependency in system.before.iter().chain(&system.after) {
        bytes = bytes.checked_add(4)?.checked_add(dependency.len())?;
    }
    let access_count = system
        .access
        .simulation_reads
        .len()
        .checked_add(system.access.simulation_writes.len())?
        .checked_add(system.access.presentation_reads.len())?
        .checked_add(system.access.presentation_writes.len())?;
    bytes.checked_add(access_count.checked_mul(4)?)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use voplay_protocol::Handle;

    use super::*;
    use crate::asset::{ArtifactId, AssetId, AssetRegistration, AssetServerConfig};
    use crate::hierarchy::{HierarchyConfig, Transform};
    use crate::render_graph::CompiledRenderGraph;
    use crate::schedule::{AccessSet, Stage};
    use crate::world::{WorldEntity, WorldId};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn engine() -> EngineId {
        handle(1)
    }

    fn entity(world: WorldId, index: u32) -> WorldEntity {
        WorldEntity {
            world,
            entity: handle(index),
        }
    }

    #[test]
    fn hierarchy_and_asset_pages_are_owner_qualified_stable_and_revisioned() {
        let service = EditorInspectionService::new(engine(), EditorInspectionConfig::default())
            .expect("service");
        let world = WorldId {
            engine: engine(),
            handle: handle(2),
        };
        let mut hierarchy = Hierarchy::new(world, HierarchyConfig::default()).expect("hierarchy");
        let root = entity(world, 1);
        let child = entity(world, 2);
        hierarchy
            .insert(root, None, Transform::default())
            .expect("root");
        hierarchy
            .insert(
                child,
                Some(root),
                Transform {
                    translation: [1, 2, 3],
                },
            )
            .expect("child");
        let first = service
            .hierarchy_page(&hierarchy, 0, 1, None)
            .expect("first");
        assert_eq!(first.entries[0].entity, root);
        assert_eq!(first.next_cursor, Some(1));
        let second = service
            .hierarchy_page(
                &hierarchy,
                first.next_cursor.expect("cursor"),
                1,
                Some(first.source_revision),
            )
            .expect("second");
        assert_eq!(second.entries[0].entity, child);

        let mut assets =
            AssetServer::new(engine(), handle(3), AssetServerConfig::default()).expect("assets");
        assets
            .register_batch(vec![
                AssetRegistration {
                    asset_id: AssetId([2; 16]),
                    asset_type: 2,
                    source_revision: 1,
                    artifact_id: ArtifactId([12; 16]),
                    dependencies: vec![AssetId([1; 16])],
                },
                AssetRegistration {
                    asset_id: AssetId([1; 16]),
                    asset_type: 1,
                    source_revision: 1,
                    artifact_id: ArtifactId([11; 16]),
                    dependencies: vec![],
                },
            ])
            .expect("register");
        let page = service
            .assets_page(&assets, 0, 8, None)
            .expect("asset page");
        assert_eq!(page.entries[0].asset_id, AssetId([1; 16]));
        assert_eq!(page.entries[1].asset_id, AssetId([2; 16]));

        let foreign_world = WorldId {
            engine: handle(9),
            handle: handle(2),
        };
        let foreign = Hierarchy::new(foreign_world, HierarchyConfig::default()).expect("foreign");
        assert_eq!(
            service.hierarchy_page(&foreign, 0, 1, None),
            Err(EditorInspectionError::WrongEngine)
        );
    }

    #[test]
    fn schedule_and_render_pages_preserve_compiled_order_and_reject_stale_revision() {
        let service = EditorInspectionService::new(engine(), EditorInspectionConfig::default())
            .expect("service");
        let schedule = Schedule::configure(vec![SystemSpec {
            name: "gameplay".to_owned(),
            stage: Stage::Gameplay,
            deterministic: true,
            access: AccessSet {
                simulation_reads: BTreeSet::from([1]),
                ..AccessSet::default()
            },
            before: vec![],
            after: vec![],
        }])
        .expect("schedule");
        let schedule_page = service
            .schedule_page(engine(), &schedule, 0, 4, Some(schedule.hash()))
            .expect("schedule page");
        assert_eq!(schedule_page.entries[0].name, "gameplay");
        assert_eq!(
            service.schedule_page(engine(), &schedule, 0, 4, Some(schedule.hash() + 1)),
            Err(EditorInspectionError::StaleSourceRevision)
        );

        let graph = CompiledRenderGraph {
            ordered_nodes: vec![GraphNodeId(2), GraphNodeId(1)],
            final_versions: BTreeMap::from([(GraphResourceId(5), 3)]),
            allocations: BTreeMap::from([(GraphResourceId(5), 7)]),
            signature: 99,
        };
        let render = service
            .render_page(engine(), &graph, 0, 8, Some(99))
            .expect("render");
        assert_eq!(
            render.entries[0],
            RenderPanelEntry::Node {
                position: 0,
                node: GraphNodeId(2)
            }
        );
        assert_eq!(
            render.entries[1],
            RenderPanelEntry::Node {
                position: 1,
                node: GraphNodeId(1)
            }
        );
        assert_eq!(
            service.render_page(handle(9), &graph, 0, 8, None),
            Err(EditorInspectionError::WrongEngine)
        );
    }

    #[test]
    fn performance_samples_are_monotonic_bounded_and_report_drops() {
        let mut service = EditorInspectionService::new(
            engine(),
            EditorInspectionConfig {
                max_perf_samples: 2,
                ..EditorInspectionConfig::default()
            },
        )
        .expect("service");
        for sequence in 1..=3 {
            service
                .publish_perf_sample(PerfPanelSample {
                    engine: engine(),
                    sequence,
                    tick: sequence,
                    system: format!("system-{sequence}"),
                    duration_nanos: sequence * 10,
                    allocation_bytes: sequence * 20,
                    lock_wait_nanos: sequence * 5,
                    queue_items: sequence as usize,
                    queue_bytes: sequence as usize * 64,
                    error_count: 0,
                    owners: RuntimeOwnerCounters::default(),
                })
                .expect("sample");
        }
        let page = service.performance_page(0, 1, Some(3)).expect("perf page");
        assert_eq!(page.dropped_before, 1);
        assert_eq!(page.entries[0].sequence, 2);
        assert_eq!(page.next_cursor, Some(1));
        assert_eq!(
            service.publish_perf_sample(PerfPanelSample {
                engine: engine(),
                sequence: 3,
                tick: 3,
                system: "duplicate".to_owned(),
                duration_nanos: 0,
                allocation_bytes: 0,
                lock_wait_nanos: 0,
                queue_items: 0,
                queue_bytes: 0,
                error_count: 0,
                owners: RuntimeOwnerCounters::default(),
            }),
            Err(EditorInspectionError::PerfSequence)
        );
    }

    #[test]
    fn byte_budget_stops_before_next_entry_without_skipping_cursor() {
        let service = EditorInspectionService::new(
            engine(),
            EditorInspectionConfig {
                max_page_entries: 8,
                max_page_bytes: 90,
                max_perf_samples: 8,
                max_perf_label_bytes: 32,
            },
        )
        .expect("service");
        let graph = CompiledRenderGraph {
            ordered_nodes: (1..=7).map(GraphNodeId).collect(),
            final_versions: BTreeMap::new(),
            allocations: BTreeMap::new(),
            signature: 7,
        };
        let first = service
            .render_page(engine(), &graph, 0, 8, None)
            .expect("first");
        assert_eq!(first.entries.len(), 5);
        assert_eq!(first.next_cursor, Some(5));
        let second = service
            .render_page(engine(), &graph, 5, 8, Some(7))
            .expect("second");
        assert_eq!(second.entries.len(), 2);
        assert_eq!(second.next_cursor, None);
        assert_eq!(
            service.render_page(engine(), &graph, 8, 1, None),
            Err(EditorInspectionError::InvalidCursor)
        );
    }
}
