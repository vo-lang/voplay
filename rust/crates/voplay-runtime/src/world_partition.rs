use std::collections::{BTreeMap, BTreeSet};

use crate::{
    asset::{
        AssetError, AssetRef, AssetScopeId, AssetScopeKind, AssetServer, AssetTicket,
        AssetTicketOutcome, AssetTicketResult,
    },
    partition_chunk_codec::{
        decode_partition_chunk, PartitionChunkCodecConfig, PartitionChunkCodecError,
    },
    partition_io::{
        PartitionIoError, PartitionIoOutcome, PartitionIoRequest, PartitionIoRequestId,
        PartitionIoRequestStatus, PartitionIoResult, PartitionIoScheduler,
        PartitionIoShutdownReport, PartitionIoSource,
    },
    scene::{
        ExternalObjectBinding, PrefabOverride, SceneApplyReport, SceneError, SceneInstance,
        SceneObjectSpec, SceneTransaction,
    },
    world::World,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PartitionId(pub u64);

impl PartitionId {
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartitionBounds {
    pub min: [i64; 3],
    pub max: [i64; 3],
}

impl PartitionBounds {
    pub const fn is_valid(self) -> bool {
        self.min[0] <= self.max[0] && self.min[1] <= self.max[1] && self.min[2] <= self.max[2]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterestVolume {
    pub bounds: PartitionBounds,
    pub priority: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamingPlan {
    pub desired: BTreeSet<PartitionId>,
    pub begin_load: Vec<PartitionId>,
    pub deferred_load: Vec<PartitionId>,
    pub unload: Vec<PartitionId>,
    pub budget_excluded: Vec<PartitionId>,
    pub blocked_by_failure: Vec<PartitionId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionManifest {
    pub id: PartitionId,
    pub bounds: PartitionBounds,
    pub priority: i32,
    pub dependencies: Vec<PartitionId>,
    pub assets: Vec<AssetRef>,
    pub estimated_objects: usize,
    pub estimated_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionChunk {
    pub id: PartitionId,
    pub objects: Vec<SceneObjectSpec>,
    pub overrides: Vec<PrefabOverride>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldPartitionConfig {
    pub max_partitions: usize,
    pub max_dependencies_per_partition: usize,
    pub max_assets_per_partition: usize,
    pub max_active_partitions: usize,
    pub max_active_objects: usize,
    pub max_active_bytes: usize,
    pub load_margin: u64,
    pub unload_margin: u64,
    pub max_concurrent_loads: usize,
}

impl Default for WorldPartitionConfig {
    fn default() -> Self {
        Self {
            max_partitions: 65_536,
            max_dependencies_per_partition: 256,
            max_assets_per_partition: 4096,
            max_active_partitions: 4096,
            max_active_objects: 1_000_000,
            max_active_bytes: 256 * 1024 * 1024,
            load_margin: 0,
            unload_margin: 64,
            max_concurrent_loads: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionState {
    Unloaded,
    Loading,
    Ready,
    Active,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldPartitionOwnerSnapshot {
    pub closed: bool,
    pub source_revision: u64,
    pub partitions: usize,
    pub unloaded: usize,
    pub loading: usize,
    pub ready: usize,
    pub active: usize,
    pub failed: usize,
    pub asset_tickets: usize,
    pub io_requests: usize,
    pub retired_io_requests: usize,
    pub installed_chunks: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldPartitionShutdownReport {
    pub released_partitions: usize,
    pub settled_requests: Vec<PartitionIoRequestId>,
    pub awaiting_worker_results: Vec<PartitionIoRequestId>,
    pub scene_report: Option<SceneApplyReport>,
    pub source_revision: u64,
    pub io: Option<PartitionIoShutdownReport>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorldPartitionError {
    InvalidConfig,
    InvalidManifest,
    PartitionCapacity,
    DependencyCapacity,
    AssetCapacity,
    DuplicatePartition,
    UnknownPartition,
    UnknownDependency,
    DependencyCycle,
    ActivePartitionCapacity,
    ActiveObjectCapacity,
    ActiveByteCapacity,
    InvalidState,
    DependencyNotReady,
    ChunkNotInstalled,
    ChunkContract,
    ActiveDependency,
    Closed,
    RevisionExhausted,
    IoOutcome,
    IoIdentity,
    Io(PartitionIoError),
    Codec(PartitionChunkCodecError),
    Asset(AssetError),
    Scene(SceneError),
}

impl From<AssetError> for WorldPartitionError {
    fn from(error: AssetError) -> Self {
        Self::Asset(error)
    }
}

impl From<SceneError> for WorldPartitionError {
    fn from(error: SceneError) -> Self {
        Self::Scene(error)
    }
}

impl From<PartitionChunkCodecError> for WorldPartitionError {
    fn from(error: PartitionChunkCodecError) -> Self {
        Self::Codec(error)
    }
}

impl From<PartitionIoError> for WorldPartitionError {
    fn from(error: PartitionIoError) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
struct PartitionRecord {
    manifest: PartitionManifest,
    state: PartitionState,
    scope: Option<AssetScopeId>,
    tickets: BTreeSet<AssetTicket>,
    ready_tickets: BTreeSet<AssetTicket>,
    chunk: Option<PartitionChunk>,
    io_request: Option<PartitionIoRequestId>,
    assets_ready: bool,
    chunk_required: bool,
}

pub struct WorldPartition {
    config: WorldPartitionConfig,
    records: BTreeMap<PartitionId, PartitionRecord>,
    retired_io_requests: BTreeSet<PartitionIoRequestId>,
    source_revision: u64,
    closed: bool,
}

impl WorldPartition {
    pub fn new(config: WorldPartitionConfig) -> Result<Self, WorldPartitionError> {
        if config.max_partitions == 0
            || config.max_dependencies_per_partition == 0
            || config.max_assets_per_partition == 0
            || config.max_active_partitions == 0
            || config.max_active_objects == 0
            || config.max_active_bytes == 0
            || config.load_margin > config.unload_margin
            || config.max_concurrent_loads == 0
        {
            return Err(WorldPartitionError::InvalidConfig);
        }
        Ok(Self {
            config,
            records: BTreeMap::new(),
            retired_io_requests: BTreeSet::new(),
            source_revision: 0,
            closed: false,
        })
    }

    pub const fn source_revision(&self) -> u64 {
        self.source_revision
    }

    pub fn state(&self, id: PartitionId) -> Option<PartitionState> {
        self.records.get(&id).map(|record| record.state)
    }

    pub fn io_request(&self, id: PartitionId) -> Option<PartitionIoRequestId> {
        self.records.get(&id).and_then(|record| record.io_request)
    }

    pub fn owner_snapshot(&self) -> WorldPartitionOwnerSnapshot {
        let mut snapshot = WorldPartitionOwnerSnapshot {
            closed: self.closed,
            source_revision: self.source_revision,
            partitions: self.records.len(),
            unloaded: 0,
            loading: 0,
            ready: 0,
            active: 0,
            failed: 0,
            asset_tickets: 0,
            io_requests: 0,
            retired_io_requests: self.retired_io_requests.len(),
            installed_chunks: 0,
        };
        for record in self.records.values() {
            match record.state {
                PartitionState::Unloaded => snapshot.unloaded += 1,
                PartitionState::Loading => snapshot.loading += 1,
                PartitionState::Ready => snapshot.ready += 1,
                PartitionState::Active => snapshot.active += 1,
                PartitionState::Failed => snapshot.failed += 1,
            }
            snapshot.asset_tickets += record.tickets.len();
            snapshot.io_requests += record.io_request.is_some() as usize;
            snapshot.installed_chunks += record.chunk.is_some() as usize;
        }
        snapshot
    }

    pub fn register_batch(
        &mut self,
        manifests: Vec<PartitionManifest>,
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        if self.records.len().saturating_add(manifests.len()) > self.config.max_partitions {
            return Err(WorldPartitionError::PartitionCapacity);
        }
        let mut graph: BTreeMap<_, Vec<_>> = self
            .records
            .iter()
            .map(|(id, record)| (*id, record.manifest.dependencies.clone()))
            .collect();
        let mut batch = BTreeMap::new();
        for manifest in manifests {
            validate_manifest(&manifest, &self.config)?;
            if self.records.contains_key(&manifest.id)
                || batch.insert(manifest.id, manifest.clone()).is_some()
            {
                return Err(WorldPartitionError::DuplicatePartition);
            }
            graph.insert(manifest.id, manifest.dependencies.clone());
        }
        validate_graph(&graph)?;
        for manifest in batch.into_values() {
            self.records.insert(
                manifest.id,
                PartitionRecord {
                    manifest,
                    state: PartitionState::Unloaded,
                    scope: None,
                    tickets: BTreeSet::new(),
                    ready_tickets: BTreeSet::new(),
                    chunk: None,
                    io_request: None,
                    assets_ready: false,
                    chunk_required: false,
                },
            );
        }
        Ok(())
    }

    pub fn hot_reload_manifests(
        &mut self,
        manifests: Vec<PartitionManifest>,
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        let mut staged = self.records.clone();
        let mut seen = BTreeSet::new();
        for manifest in manifests {
            validate_manifest(&manifest, &self.config)?;
            if !seen.insert(manifest.id) {
                return Err(WorldPartitionError::DuplicatePartition);
            }
            let record = staged
                .get_mut(&manifest.id)
                .ok_or(WorldPartitionError::UnknownPartition)?;
            if record.state != PartitionState::Unloaded && record.manifest.assets != manifest.assets
            {
                return Err(WorldPartitionError::InvalidState);
            }
            if let Some(chunk) = &record.chunk {
                if chunk.objects.len() > manifest.estimated_objects {
                    return Err(WorldPartitionError::ActiveObjectCapacity);
                }
                if chunk_size(chunk)? > manifest.estimated_bytes {
                    return Err(WorldPartitionError::ActiveByteCapacity);
                }
            }
            record.manifest = manifest;
        }
        let graph = staged
            .iter()
            .map(|(id, record)| (*id, record.manifest.dependencies.clone()))
            .collect();
        validate_graph(&graph)?;
        for record in staged
            .values()
            .filter(|record| record.state == PartitionState::Active)
        {
            if record.manifest.dependencies.iter().any(|dependency| {
                staged
                    .get(dependency)
                    .is_none_or(|record| record.state != PartitionState::Active)
            }) {
                return Err(WorldPartitionError::DependencyNotReady);
            }
        }
        let active = staged
            .iter()
            .filter_map(|(id, record)| (record.state == PartitionState::Active).then_some(*id))
            .collect();
        validate_budget_records(&self.config, &staged, &active)?;
        self.records = staged;
        Ok(())
    }

    pub fn load_order(
        &self,
        desired: &BTreeSet<PartitionId>,
    ) -> Result<Vec<PartitionId>, WorldPartitionError> {
        self.ensure_open()?;
        let closure = self.dependency_closure(desired)?;
        self.validate_budget(&closure)?;
        let mut order = Vec::new();
        let mut visited = BTreeSet::new();
        for id in &closure {
            self.visit_order(*id, &closure, &mut visited, &mut order)?;
        }
        Ok(order)
    }

    pub fn plan_interest(
        &self,
        interests: &[InterestVolume],
    ) -> Result<StreamingPlan, WorldPartitionError> {
        self.ensure_open()?;
        if interests.iter().any(|interest| !interest.bounds.is_valid()) {
            return Err(WorldPartitionError::InvalidManifest);
        }
        let mut retained = BTreeSet::new();
        let mut candidates = Vec::new();
        for (id, record) in &self.records {
            if record.state == PartitionState::Failed {
                continue;
            }
            let resident = matches!(
                record.state,
                PartitionState::Loading | PartitionState::Ready | PartitionState::Active
            );
            let margin = if resident {
                self.config.unload_margin
            } else {
                self.config.load_margin
            };
            let mut best: Option<(i64, u128)> = None;
            for interest in interests {
                if bounds_intersect(record.manifest.bounds, interest.bounds, margin) {
                    let score = i64::from(record.manifest.priority)
                        .checked_add(i64::from(interest.priority))
                        .ok_or(WorldPartitionError::InvalidManifest)?;
                    let distance = bounds_distance_squared(record.manifest.bounds, interest.bounds);
                    if best.is_none_or(|current| {
                        score > current.0 || (score == current.0 && distance < current.1)
                    }) {
                        best = Some((score, distance));
                    }
                }
            }
            if let Some((score, distance)) = best {
                if resident {
                    retained.insert(*id);
                }
                candidates.push((*id, score, distance, resident));
            }
        }

        let mut blocked_by_failure = Vec::new();
        let retained_candidates = retained.clone();
        retained.clear();
        for id in retained_candidates {
            let closure = self.dependency_closure(&BTreeSet::from([id]))?;
            if closure
                .iter()
                .any(|member| self.records[member].state == PartitionState::Failed)
            {
                blocked_by_failure.push(id);
            } else {
                retained.insert(id);
            }
        }
        let mut desired = self.dependency_closure(&retained)?;
        self.validate_budget(&desired)?;
        candidates.sort_by_key(|(id, score, distance, resident)| {
            (!*resident, std::cmp::Reverse(*score), *distance, *id)
        });
        let mut budget_excluded = Vec::new();
        let mut scores: BTreeMap<PartitionId, (i64, u128)> = BTreeMap::new();
        for (id, score, distance, _) in candidates {
            let closure = self.dependency_closure(&BTreeSet::from([id]))?;
            if closure
                .iter()
                .any(|member| self.records[member].state == PartitionState::Failed)
            {
                blocked_by_failure.push(id);
                continue;
            }
            let mut proposed = desired.clone();
            proposed.extend(&closure);
            if self.validate_budget(&proposed).is_err() {
                if !desired.contains(&id) {
                    budget_excluded.push(id);
                }
                continue;
            }
            desired = proposed;
            for member in closure {
                let entry = scores.entry(member).or_insert((score, distance));
                if score > entry.0 || (score == entry.0 && distance < entry.1) {
                    *entry = (score, distance);
                }
            }
        }

        let mut pending: BTreeSet<_> = desired
            .iter()
            .copied()
            .filter(|id| self.records[id].state == PartitionState::Unloaded)
            .collect();
        let mut load_order = Vec::new();
        while !pending.is_empty() {
            let next = pending
                .iter()
                .filter(|id| {
                    self.records[id]
                        .manifest
                        .dependencies
                        .iter()
                        .all(|dependency| !pending.contains(dependency))
                })
                .min_by_key(|id| {
                    let (score, distance) =
                        scores.get(id).copied().unwrap_or((i64::MIN, u128::MAX));
                    (std::cmp::Reverse(score), distance, **id)
                })
                .copied()
                .ok_or(WorldPartitionError::DependencyCycle)?;
            pending.remove(&next);
            load_order.push(next);
        }
        let in_flight = self
            .records
            .values()
            .filter(|record| record.state == PartitionState::Loading)
            .count();
        let slots = self.config.max_concurrent_loads.saturating_sub(in_flight);
        let begin_load = load_order.iter().take(slots).copied().collect();
        let deferred_load = load_order.iter().skip(slots).copied().collect();

        let mut unload: Vec<_> = self
            .records
            .iter()
            .filter(|(id, record)| {
                !desired.contains(id)
                    && matches!(
                        record.state,
                        PartitionState::Loading
                            | PartitionState::Ready
                            | PartitionState::Active
                            | PartitionState::Failed
                    )
            })
            .map(|(id, _)| *id)
            .collect();
        unload.sort_by_key(|id| (std::cmp::Reverse(self.dependency_depth(*id)), *id));
        budget_excluded.sort();
        budget_excluded.dedup();
        blocked_by_failure.sort();
        blocked_by_failure.dedup();
        Ok(StreamingPlan {
            desired,
            begin_load,
            deferred_load,
            unload,
            budget_excluded,
            blocked_by_failure,
        })
    }

    pub fn begin_load(
        &mut self,
        id: PartitionId,
        assets: &mut AssetServer,
        deadline_millis: u64,
    ) -> Result<Vec<AssetTicket>, WorldPartitionError> {
        self.ensure_open()?;
        let record = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if record.state != PartitionState::Unloaded {
            return Err(WorldPartitionError::InvalidState);
        }
        if record.manifest.dependencies.iter().any(|dependency| {
            !matches!(
                self.records.get(dependency).map(|record| record.state),
                Some(PartitionState::Loading | PartitionState::Ready | PartitionState::Active)
            )
        }) {
            return Err(WorldPartitionError::DependencyNotReady);
        }
        let asset_refs = record.manifest.assets.clone();
        let (scope, tickets) = assets.create_scope_with_requests(
            AssetScopeKind::Scene,
            &asset_refs,
            deadline_millis,
        )?;
        let record = self.records.get_mut(&id).unwrap();
        record.scope = Some(scope);
        record.tickets = tickets.iter().copied().collect();
        record.ready_tickets.clear();
        record.io_request = None;
        record.assets_ready = tickets.is_empty();
        record.chunk_required = false;
        record.state = if tickets.is_empty() {
            PartitionState::Ready
        } else {
            PartitionState::Loading
        };
        Ok(tickets)
    }

    pub fn begin_stream_load(
        &mut self,
        id: PartitionId,
        assets: &mut AssetServer,
        io: &mut PartitionIoScheduler,
        source: PartitionIoSource,
        priority: i32,
        deadline_millis: u64,
        expected_digest: Option<u64>,
    ) -> Result<(Vec<AssetTicket>, PartitionIoRequestId), WorldPartitionError> {
        self.ensure_open()?;
        let max_bytes = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?
            .manifest
            .estimated_bytes;
        let tickets = self.begin_load(id, assets, deadline_millis)?;
        let request = match io.submit(PartitionIoRequest {
            partition: id,
            source,
            priority,
            deadline_millis,
            max_bytes,
            expected_digest,
        }) {
            Ok(request) => request,
            Err(error) => {
                self.release_load_assets(id, assets)?;
                return Err(WorldPartitionError::Io(error));
            }
        };
        let record = self.records.get_mut(&id).unwrap();
        record.io_request = Some(request);
        record.chunk_required = true;
        record.state = PartitionState::Loading;
        Ok((tickets, request))
    }

    pub fn observe_results(
        &mut self,
        results: &[AssetTicketResult],
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        for result in results {
            let Some((_, record)) = self
                .records
                .iter_mut()
                .find(|(_, record)| record.tickets.contains(&result.ticket))
            else {
                continue;
            };
            if record.state != PartitionState::Loading {
                continue;
            }
            if result.outcome != AssetTicketOutcome::Ready {
                record.state = PartitionState::Failed;
                continue;
            }
            record.ready_tickets.insert(result.ticket);
            if record.ready_tickets == record.tickets {
                record.assets_ready = true;
                if !record.chunk_required || record.chunk.is_some() {
                    record.state = PartitionState::Ready;
                }
            }
        }
        Ok(())
    }

    pub fn install_chunk(&mut self, chunk: PartitionChunk) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        let record = self
            .records
            .get_mut(&chunk.id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        let was_active = record.state == PartitionState::Active;
        if record.state != PartitionState::Ready
            && !(record.state == PartitionState::Loading && record.chunk_required)
            && !was_active
        {
            return Err(WorldPartitionError::InvalidState);
        }
        if chunk
            .objects
            .iter()
            .any(|object| object.partition != chunk.id.0)
        {
            return Err(WorldPartitionError::ChunkContract);
        }
        if chunk.objects.len() > record.manifest.estimated_objects {
            return Err(WorldPartitionError::ActiveObjectCapacity);
        }
        if chunk_size(&chunk)? > record.manifest.estimated_bytes {
            return Err(WorldPartitionError::ActiveByteCapacity);
        }
        record.chunk = Some(chunk);
        if record.assets_ready && !was_active {
            record.state = PartitionState::Ready;
        }
        Ok(())
    }

    pub fn install_io_result(
        &mut self,
        result: &PartitionIoResult,
        expected_scene: crate::scene::SceneInstanceId,
        codec: PartitionChunkCodecConfig,
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        let record = self
            .records
            .get(&result.partition)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if record
            .io_request
            .is_some_and(|request| request != result.request)
        {
            return Err(WorldPartitionError::IoIdentity);
        }
        if result.outcome != PartitionIoOutcome::Ready {
            return Err(WorldPartitionError::IoOutcome);
        }
        let chunk = decode_partition_chunk(&result.bytes, result.partition, expected_scene, codec)?;
        self.install_chunk(chunk)?;
        if self.records[&result.partition].io_request == Some(result.request) {
            self.records.get_mut(&result.partition).unwrap().io_request = None;
        }
        Ok(())
    }

    pub fn observe_io_result(
        &mut self,
        result: &PartitionIoResult,
        expected_scene: crate::scene::SceneInstanceId,
        codec: PartitionChunkCodecConfig,
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        let record = self
            .records
            .get(&result.partition)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if record.io_request != Some(result.request) {
            return Err(WorldPartitionError::IoIdentity);
        }
        if result.outcome != PartitionIoOutcome::Ready {
            let record = self.records.get_mut(&result.partition).unwrap();
            record.io_request = None;
            record.state = PartitionState::Failed;
            return Err(WorldPartitionError::IoOutcome);
        }
        let chunk =
            match decode_partition_chunk(&result.bytes, result.partition, expected_scene, codec) {
                Ok(chunk) => chunk,
                Err(error) => {
                    let record = self.records.get_mut(&result.partition).unwrap();
                    record.io_request = None;
                    record.state = PartitionState::Failed;
                    return Err(WorldPartitionError::Codec(error));
                }
            };
        if let Err(error) = self.install_chunk(chunk) {
            let record = self.records.get_mut(&result.partition).unwrap();
            record.io_request = None;
            record.state = PartitionState::Failed;
            return Err(error);
        }
        self.records.get_mut(&result.partition).unwrap().io_request = None;
        Ok(())
    }

    pub fn activate(
        &mut self,
        desired: &BTreeSet<PartitionId>,
        scene: &mut SceneInstance,
        world: &mut World,
        external_bindings: &BTreeMap<u64, ExternalObjectBinding>,
    ) -> Result<SceneApplyReport, WorldPartitionError> {
        self.ensure_open()?;
        let closure = self.dependency_closure(desired)?;
        self.validate_budget(&closure)?;
        let mut objects = Vec::new();
        let mut overrides = Vec::new();
        for id in &closure {
            let record = &self.records[id];
            if !matches!(record.state, PartitionState::Ready | PartitionState::Active) {
                return Err(WorldPartitionError::DependencyNotReady);
            }
            let chunk = record
                .chunk
                .as_ref()
                .ok_or(WorldPartitionError::ChunkNotInstalled)?;
            objects.extend(chunk.objects.clone());
            overrides.extend(chunk.overrides.clone());
        }
        let source_revision = self
            .source_revision
            .checked_add(1)
            .ok_or(WorldPartitionError::RevisionExhausted)?;
        let report = scene.apply(
            world,
            SceneTransaction {
                source_revision,
                objects,
                overrides,
            },
            external_bindings,
        )?;
        for (id, record) in &mut self.records {
            if closure.contains(id) {
                record.state = PartitionState::Active;
            } else if record.state == PartitionState::Active {
                record.state = PartitionState::Ready;
            }
        }
        self.source_revision = source_revision;
        Ok(report)
    }

    pub fn reconcile_chunk(
        &mut self,
        chunk: PartitionChunk,
        desired: &BTreeSet<PartitionId>,
        scene: &mut SceneInstance,
        world: &mut World,
        external_bindings: &BTreeMap<u64, ExternalObjectBinding>,
    ) -> Result<SceneApplyReport, WorldPartitionError> {
        self.ensure_open()?;
        let id = chunk.id;
        let record = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        let previous_state = record.state;
        let previous_chunk = record.chunk.clone();
        self.install_chunk(chunk)?;
        match self.activate(desired, scene, world, external_bindings) {
            Ok(report) => Ok(report),
            Err(error) => {
                let record = self.records.get_mut(&id).unwrap();
                record.state = previous_state;
                record.chunk = previous_chunk;
                Err(error)
            }
        }
    }

    pub fn unload(
        &mut self,
        id: PartitionId,
        assets: &mut AssetServer,
    ) -> Result<(), WorldPartitionError> {
        self.ensure_open()?;
        if self.records.values().any(|record| {
            record.state == PartitionState::Active && record.manifest.dependencies.contains(&id)
        }) {
            return Err(WorldPartitionError::ActiveDependency);
        }
        let record = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if !matches!(
            record.state,
            PartitionState::Loading | PartitionState::Ready | PartitionState::Failed
        ) {
            return Err(WorldPartitionError::InvalidState);
        }
        if record.io_request.is_some() {
            return Err(WorldPartitionError::InvalidState);
        }
        self.release_load_assets(id, assets)
    }

    pub fn cancel_stream_load(
        &mut self,
        id: PartitionId,
        assets: &mut AssetServer,
        io: &mut PartitionIoScheduler,
    ) -> Result<Option<PartitionIoRequestId>, WorldPartitionError> {
        self.ensure_open()?;
        let record = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if !matches!(
            record.state,
            PartitionState::Loading | PartitionState::Ready | PartitionState::Failed
        ) {
            return Err(WorldPartitionError::InvalidState);
        }
        let request = record.io_request.ok_or(WorldPartitionError::InvalidState)?;
        if self.retired_io_requests.len() >= self.config.max_partitions {
            return Err(WorldPartitionError::PartitionCapacity);
        }
        let pending_completion = if io.cancel(request)?.is_some() {
            let result = io.take_result(request)?;
            if result.partition != id {
                return Err(WorldPartitionError::IoIdentity);
            }
            None
        } else {
            self.retired_io_requests.insert(request);
            Some(request)
        };
        self.release_load_assets(id, assets)?;
        Ok(pending_completion)
    }

    pub fn discard_retired_io_result(
        &mut self,
        expected: PartitionIoRequestId,
        result: &PartitionIoResult,
    ) -> Result<(), WorldPartitionError> {
        if result.request != expected
            || result.outcome == PartitionIoOutcome::Ready
            || !self.retired_io_requests.remove(&expected)
        {
            return Err(WorldPartitionError::IoIdentity);
        }
        Ok(())
    }

    pub fn shutdown(
        &mut self,
        assets: &mut AssetServer,
        io: &mut PartitionIoScheduler,
        scene: &mut SceneInstance,
        world: &mut World,
        external_bindings: &BTreeMap<u64, ExternalObjectBinding>,
    ) -> Result<WorldPartitionShutdownReport, WorldPartitionError> {
        if self.closed {
            return Ok(WorldPartitionShutdownReport {
                released_partitions: 0,
                settled_requests: Vec::new(),
                awaiting_worker_results: self.retired_io_requests.iter().copied().collect(),
                scene_report: None,
                source_revision: self.source_revision,
                io: None,
            });
        }
        let io_requests = self
            .records
            .values()
            .filter_map(|record| record.io_request)
            .collect::<Vec<_>>();
        let mut dispatched = 0_usize;
        for request in &io_requests {
            if io.request_status(*request)? == PartitionIoRequestStatus::Dispatched {
                dispatched += 1;
            }
        }
        if self
            .retired_io_requests
            .len()
            .checked_add(dispatched)
            .is_none_or(|count| count > self.config.max_partitions)
        {
            return Err(WorldPartitionError::PartitionCapacity);
        }
        if self
            .records
            .values()
            .any(|record| record.state == PartitionState::Active)
        {
            self.source_revision
                .checked_add(1)
                .ok_or(WorldPartitionError::RevisionExhausted)?;
        }
        let scene_report = if self
            .records
            .values()
            .any(|record| record.state == PartitionState::Active)
        {
            Some(self.activate(&BTreeSet::new(), scene, world, external_bindings)?)
        } else {
            None
        };
        let mut settled_requests = Vec::new();
        for id in self.records.keys().copied().collect::<Vec<_>>() {
            if self.records[&id].io_request.is_some() {
                let request = self.records[&id].io_request.unwrap();
                if self.cancel_stream_load(id, assets, io)?.is_none() {
                    settled_requests.push(request);
                }
            } else if self.records[&id].scope.is_some() {
                self.release_load_assets(id, assets)?;
            }
        }
        let released_partitions = self.records.len();
        self.records.clear();
        let io_report = io.shutdown()?;
        self.retired_io_requests.clear();
        self.closed = true;
        Ok(WorldPartitionShutdownReport {
            released_partitions,
            settled_requests,
            awaiting_worker_results: Vec::new(),
            scene_report,
            source_revision: self.source_revision,
            io: Some(io_report),
        })
    }

    fn ensure_open(&self) -> Result<(), WorldPartitionError> {
        if self.closed {
            Err(WorldPartitionError::Closed)
        } else {
            Ok(())
        }
    }

    fn release_load_assets(
        &mut self,
        id: PartitionId,
        assets: &mut AssetServer,
    ) -> Result<(), WorldPartitionError> {
        let record = self
            .records
            .get(&id)
            .ok_or(WorldPartitionError::UnknownPartition)?;
        if !matches!(
            record.state,
            PartitionState::Loading | PartitionState::Ready | PartitionState::Failed
        ) {
            return Err(WorldPartitionError::InvalidState);
        }
        let scope = record.scope.ok_or(WorldPartitionError::InvalidState)?;
        let tickets: Vec<_> = record.tickets.iter().copied().collect();
        assets.close_scope(scope)?;
        for ticket in tickets {
            assets.release_ticket(ticket)?;
        }
        assets.release_scope(scope)?;
        let record = self.records.get_mut(&id).unwrap();
        record.state = PartitionState::Unloaded;
        record.scope = None;
        record.tickets.clear();
        record.ready_tickets.clear();
        record.chunk = None;
        record.io_request = None;
        record.assets_ready = false;
        record.chunk_required = false;
        Ok(())
    }

    fn dependency_closure(
        &self,
        desired: &BTreeSet<PartitionId>,
    ) -> Result<BTreeSet<PartitionId>, WorldPartitionError> {
        let mut closure = BTreeSet::new();
        let mut stack: Vec<_> = desired.iter().copied().collect();
        while let Some(id) = stack.pop() {
            let record = self
                .records
                .get(&id)
                .ok_or(WorldPartitionError::UnknownPartition)?;
            if closure.insert(id) {
                stack.extend(record.manifest.dependencies.iter().copied());
            }
        }
        Ok(closure)
    }

    fn validate_budget(&self, closure: &BTreeSet<PartitionId>) -> Result<(), WorldPartitionError> {
        validate_budget_records(&self.config, &self.records, closure)
    }

    fn visit_order(
        &self,
        id: PartitionId,
        closure: &BTreeSet<PartitionId>,
        visited: &mut BTreeSet<PartitionId>,
        order: &mut Vec<PartitionId>,
    ) -> Result<(), WorldPartitionError> {
        if !visited.insert(id) {
            return Ok(());
        }
        for dependency in &self.records[&id].manifest.dependencies {
            if closure.contains(dependency) {
                self.visit_order(*dependency, closure, visited, order)?;
            }
        }
        order.push(id);
        Ok(())
    }

    fn dependency_depth(&self, id: PartitionId) -> usize {
        let mut max_depth = 0usize;
        let mut stack = vec![(id, 0usize)];
        while let Some((current, depth)) = stack.pop() {
            max_depth = max_depth.max(depth);
            for dependency in &self.records[&current].manifest.dependencies {
                stack.push((*dependency, depth.saturating_add(1)));
            }
        }
        max_depth
    }
}

fn validate_budget_records(
    config: &WorldPartitionConfig,
    records: &BTreeMap<PartitionId, PartitionRecord>,
    closure: &BTreeSet<PartitionId>,
) -> Result<(), WorldPartitionError> {
    if closure.len() > config.max_active_partitions {
        return Err(WorldPartitionError::ActivePartitionCapacity);
    }
    let mut objects = 0usize;
    let mut bytes = 0usize;
    for id in closure {
        let manifest = &records
            .get(id)
            .ok_or(WorldPartitionError::UnknownPartition)?
            .manifest;
        objects = objects
            .checked_add(manifest.estimated_objects)
            .ok_or(WorldPartitionError::ActiveObjectCapacity)?;
        bytes = bytes
            .checked_add(manifest.estimated_bytes)
            .ok_or(WorldPartitionError::ActiveByteCapacity)?;
    }
    if objects > config.max_active_objects {
        return Err(WorldPartitionError::ActiveObjectCapacity);
    }
    if bytes > config.max_active_bytes {
        return Err(WorldPartitionError::ActiveByteCapacity);
    }
    Ok(())
}

fn validate_manifest(
    manifest: &PartitionManifest,
    config: &WorldPartitionConfig,
) -> Result<(), WorldPartitionError> {
    if !manifest.id.is_valid()
        || !manifest.bounds.is_valid()
        || manifest.estimated_objects == 0
        || manifest.estimated_bytes == 0
    {
        return Err(WorldPartitionError::InvalidManifest);
    }
    if manifest.dependencies.len() > config.max_dependencies_per_partition {
        return Err(WorldPartitionError::DependencyCapacity);
    }
    if manifest.assets.len() > config.max_assets_per_partition {
        return Err(WorldPartitionError::AssetCapacity);
    }
    let dependencies: BTreeSet<_> = manifest.dependencies.iter().copied().collect();
    let assets: BTreeSet<_> = manifest.assets.iter().copied().collect();
    if dependencies.len() != manifest.dependencies.len()
        || dependencies.contains(&manifest.id)
        || assets.len() != manifest.assets.len()
    {
        return Err(WorldPartitionError::InvalidManifest);
    }
    Ok(())
}

fn bounds_intersect(first: PartitionBounds, second: PartitionBounds, margin: u64) -> bool {
    (0..3).all(|axis| {
        let margin = i128::from(margin);
        i128::from(first.min[axis]) <= i128::from(second.max[axis]) + margin
            && i128::from(first.max[axis]) >= i128::from(second.min[axis]) - margin
    })
}

fn bounds_distance_squared(first: PartitionBounds, second: PartitionBounds) -> u128 {
    let mut distance = 0u128;
    for axis in 0..3 {
        let delta = if first.max[axis] < second.min[axis] {
            i128::from(second.min[axis]) - i128::from(first.max[axis])
        } else if second.max[axis] < first.min[axis] {
            i128::from(first.min[axis]) - i128::from(second.max[axis])
        } else {
            0
        };
        let delta = u128::try_from(delta).unwrap_or(u128::MAX);
        distance = distance.saturating_add(delta.saturating_mul(delta));
    }
    distance
}

fn validate_graph(
    graph: &BTreeMap<PartitionId, Vec<PartitionId>>,
) -> Result<(), WorldPartitionError> {
    for dependencies in graph.values() {
        if dependencies
            .iter()
            .any(|dependency| !graph.contains_key(dependency))
        {
            return Err(WorldPartitionError::UnknownDependency);
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in graph.keys() {
        let mut stack = vec![(*root, false)];
        while let Some((id, exiting)) = stack.pop() {
            if exiting {
                visiting.remove(&id);
                visited.insert(id);
                continue;
            }
            if visited.contains(&id) {
                continue;
            }
            if !visiting.insert(id) {
                return Err(WorldPartitionError::DependencyCycle);
            }
            stack.push((id, true));
            for dependency in graph[&id].iter().rev() {
                if visiting.contains(dependency) {
                    return Err(WorldPartitionError::DependencyCycle);
                }
                if !visited.contains(dependency) {
                    stack.push((*dependency, false));
                }
            }
        }
    }
    Ok(())
}

fn chunk_size(chunk: &PartitionChunk) -> Result<usize, WorldPartitionError> {
    let mut bytes = 0usize;
    for object in &chunk.objects {
        bytes = bytes
            .checked_add(64)
            .ok_or(WorldPartitionError::ActiveByteCapacity)?;
        for fields in object.components.values() {
            for value in fields.values() {
                bytes = bytes
                    .checked_add(8)
                    .and_then(|bytes| bytes.checked_add(value.len()))
                    .ok_or(WorldPartitionError::ActiveByteCapacity)?;
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        asset::{ArtifactId, AssetId, AssetRegistration, AssetServerConfig},
        scene::{
            AuthoringObjectId, PrefabInstancePath, RuntimeObjectKey, SceneConfig, SceneInstanceId,
            SourceAssetId,
        },
        world::{WorldConfig, WorldId},
    };
    use voplay_protocol::{EngineId, Handle};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn world_id() -> WorldId {
        WorldId {
            engine: EngineId {
                index: 1,
                generation: 1,
            },
            handle: handle(2),
        }
    }

    fn scene_id() -> SceneInstanceId {
        SceneInstanceId {
            world: world_id(),
            handle: handle(3),
        }
    }

    fn object(partition: PartitionId, local: u64) -> SceneObjectSpec {
        SceneObjectSpec {
            key: RuntimeObjectKey {
                root_scene_instance: scene_id(),
                prefab_path: PrefabInstancePath::root(),
                authoring: AuthoringObjectId {
                    source_asset: SourceAssetId([partition.0 as u8; 16]),
                    local_object_id: local,
                },
            },
            object_type: 1,
            schema_version: 1,
            partition: partition.0,
            components: BTreeMap::new(),
            references: Vec::new(),
        }
    }

    fn manifest(
        id: u64,
        dependencies: Vec<PartitionId>,
        assets: Vec<AssetRef>,
    ) -> PartitionManifest {
        PartitionManifest {
            id: PartitionId(id),
            bounds: PartitionBounds {
                min: [(id as i64 - 1) * 100, 0, 0],
                max: [(id as i64 - 1) * 100 + 10, 10, 10],
            },
            priority: 0,
            dependencies,
            assets,
            estimated_objects: 4,
            estimated_bytes: 1024,
        }
    }

    fn asset_server() -> (AssetServer, Vec<AssetRef>) {
        let engine = world_id().engine;
        let mut server =
            AssetServer::new(engine, handle(20), AssetServerConfig::default()).unwrap();
        let refs = server
            .register_batch(
                (1u8..=2)
                    .map(|index| AssetRegistration {
                        asset_id: AssetId([index; 16]),
                        asset_type: u64::from(index),
                        source_revision: 1,
                        artifact_id: ArtifactId([index + 10; 16]),
                        dependencies: Vec::new(),
                    })
                    .collect(),
            )
            .unwrap();
        (server, refs)
    }

    fn finish_asset_work(partitions: &mut WorldPartition, assets: &mut AssetServer) {
        while let Some(work) = assets.poll_work() {
            let results = assets.complete(&work).unwrap();
            partitions.observe_results(&results).unwrap();
        }
    }

    #[test]
    fn dependency_graph_and_active_budget_are_validated_atomically() {
        let mut partitions = WorldPartition::new(WorldPartitionConfig::default()).unwrap();
        assert_eq!(
            partitions.register_batch(vec![
                manifest(1, vec![PartitionId(2)], Vec::new()),
                manifest(2, vec![PartitionId(1)], Vec::new()),
            ]),
            Err(WorldPartitionError::DependencyCycle)
        );
        assert_eq!(partitions.state(PartitionId(1)), None);

        let mut config = WorldPartitionConfig::default();
        config.max_active_objects = 2;
        let mut partitions = WorldPartition::new(config).unwrap();
        let mut first = manifest(1, Vec::new(), Vec::new());
        first.estimated_objects = 1;
        let mut second = manifest(2, vec![PartitionId(1)], Vec::new());
        second.estimated_objects = 2;
        partitions.register_batch(vec![first, second]).unwrap();
        assert_eq!(
            partitions.load_order(&BTreeSet::from([PartitionId(2)])),
            Err(WorldPartitionError::ActiveObjectCapacity)
        );
    }

    #[test]
    fn asset_ready_activation_unload_and_reload_preserve_partition_contract() {
        let (mut assets, refs) = asset_server();
        let mut partitions = WorldPartition::new(WorldPartitionConfig::default()).unwrap();
        partitions
            .register_batch(vec![
                manifest(1, Vec::new(), vec![refs[0]]),
                manifest(2, vec![PartitionId(1)], vec![refs[1]]),
            ])
            .unwrap();
        assert_eq!(
            partitions
                .load_order(&BTreeSet::from([PartitionId(2)]))
                .unwrap(),
            vec![PartitionId(1), PartitionId(2)]
        );
        partitions
            .begin_load(PartitionId(1), &mut assets, 100)
            .unwrap();
        partitions
            .begin_load(PartitionId(2), &mut assets, 100)
            .unwrap();
        finish_asset_work(&mut partitions, &mut assets);
        assert_eq!(
            partitions.state(PartitionId(1)),
            Some(PartitionState::Ready)
        );
        assert_eq!(
            partitions.state(PartitionId(2)),
            Some(PartitionState::Ready)
        );

        let first = object(PartitionId(1), 1);
        let second = object(PartitionId(2), 2);
        let second_key = second.key.clone();
        partitions
            .install_chunk(PartitionChunk {
                id: PartitionId(1),
                objects: vec![first],
                overrides: Vec::new(),
            })
            .unwrap();
        partitions
            .install_chunk(PartitionChunk {
                id: PartitionId(2),
                objects: vec![second.clone()],
                overrides: Vec::new(),
            })
            .unwrap();
        let mut world = World::new(world_id(), WorldConfig::default()).unwrap();
        let mut scene = SceneInstance::new(scene_id(), SceneConfig::default()).unwrap();
        partitions
            .activate(
                &BTreeSet::from([PartitionId(2)]),
                &mut scene,
                &mut world,
                &BTreeMap::new(),
            )
            .unwrap();
        let old_entity = scene.entity(&second_key).unwrap();
        assert_eq!(world.live_entities(), 2);

        partitions
            .activate(
                &BTreeSet::from([PartitionId(1)]),
                &mut scene,
                &mut world,
                &BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(world.live_entities(), 1);
        partitions.unload(PartitionId(2), &mut assets).unwrap();
        partitions
            .begin_load(PartitionId(2), &mut assets, 200)
            .unwrap();
        finish_asset_work(&mut partitions, &mut assets);
        partitions
            .install_chunk(PartitionChunk {
                id: PartitionId(2),
                objects: vec![second],
                overrides: Vec::new(),
            })
            .unwrap();
        partitions
            .activate(
                &BTreeSet::from([PartitionId(2)]),
                &mut scene,
                &mut world,
                &BTreeMap::new(),
            )
            .unwrap();
        let new_entity = scene.entity(&second_key).unwrap();
        assert_eq!(old_entity.entity.index, new_entity.entity.index);
        assert_ne!(old_entity.entity.generation, new_entity.entity.generation);
    }

    #[test]
    fn scene_activation_failure_keeps_partition_scene_and_world_state_unchanged() {
        let mut partitions = WorldPartition::new(WorldPartitionConfig::default()).unwrap();
        partitions
            .register_batch(vec![
                manifest(1, Vec::new(), Vec::new()),
                manifest(2, Vec::new(), Vec::new()),
            ])
            .unwrap();
        let (mut assets, _) = asset_server();
        partitions
            .begin_load(PartitionId(1), &mut assets, 100)
            .unwrap();
        partitions
            .begin_load(PartitionId(2), &mut assets, 100)
            .unwrap();
        let first = object(PartitionId(1), 1);
        let mut duplicate = first.clone();
        duplicate.partition = 2;
        partitions
            .install_chunk(PartitionChunk {
                id: PartitionId(1),
                objects: vec![first],
                overrides: Vec::new(),
            })
            .unwrap();
        partitions
            .install_chunk(PartitionChunk {
                id: PartitionId(2),
                objects: vec![duplicate],
                overrides: Vec::new(),
            })
            .unwrap();
        let mut world = World::new(world_id(), WorldConfig::default()).unwrap();
        let mut scene = SceneInstance::new(scene_id(), SceneConfig::default()).unwrap();
        assert_eq!(
            partitions.activate(
                &BTreeSet::from([PartitionId(1), PartitionId(2)]),
                &mut scene,
                &mut world,
                &BTreeMap::new(),
            ),
            Err(WorldPartitionError::Scene(SceneError::DuplicateObject))
        );
        assert_eq!(partitions.source_revision(), 0);
        assert_eq!(
            partitions.state(PartitionId(1)),
            Some(PartitionState::Ready)
        );
        assert_eq!(
            partitions.state(PartitionId(2)),
            Some(PartitionState::Ready)
        );
        assert_eq!(scene.source_revision(), 0);
        assert_eq!(world.live_entities(), 0);
    }

    #[test]
    fn scoped_request_batch_capacity_failure_consumes_no_scope_ticket_or_lease() {
        let engine = world_id().engine;
        let mut config = AssetServerConfig::default();
        config.max_tickets = 1;
        let mut assets = AssetServer::new(engine, handle(20), config).unwrap();
        let refs = assets
            .register_batch(
                (1u8..=2)
                    .map(|index| AssetRegistration {
                        asset_id: AssetId([index; 16]),
                        asset_type: u64::from(index),
                        source_revision: 1,
                        artifact_id: ArtifactId([index + 10; 16]),
                        dependencies: Vec::new(),
                    })
                    .collect(),
            )
            .unwrap();
        assert_eq!(
            assets.create_scope_with_requests(AssetScopeKind::Scene, &refs, 100),
            Err(AssetError::TicketCapacity)
        );
        let (scope, tickets) = assets
            .create_scope_with_requests(AssetScopeKind::Scene, &refs[..1], 100)
            .unwrap();
        assert_eq!(scope.handle.index, 0);
        assert_eq!(tickets[0].handle.index, 0);
        let work = assets.poll_work().unwrap();
        let results = assets.complete(&work).unwrap();
        assert_eq!(results.len(), 1);
        assets.close_scope(scope).unwrap();
        assets.release_ticket(tickets[0]).unwrap();
        assets.release_scope(scope).unwrap();
    }

    #[test]
    fn spatial_interest_uses_load_unload_hysteresis_without_boundary_thrash() {
        let mut config = WorldPartitionConfig::default();
        config.load_margin = 0;
        config.unload_margin = 20;
        let mut partitions = WorldPartition::new(config).unwrap();
        let mut item = manifest(1, Vec::new(), Vec::new());
        item.bounds = PartitionBounds {
            min: [0, 0, 0],
            max: [10, 10, 10],
        };
        partitions.register_batch(vec![item]).unwrap();
        let near = InterestVolume {
            bounds: PartitionBounds {
                min: [10, 0, 0],
                max: [10, 0, 0],
            },
            priority: 0,
        };
        let initial = partitions.plan_interest(&[near]).unwrap();
        assert_eq!(initial.begin_load, vec![PartitionId(1)]);
        let (mut assets, _) = asset_server();
        partitions
            .begin_load(PartitionId(1), &mut assets, 100)
            .unwrap();
        assert_eq!(
            partitions.state(PartitionId(1)),
            Some(PartitionState::Ready)
        );

        let within_hysteresis = InterestVolume {
            bounds: PartitionBounds {
                min: [30, 0, 0],
                max: [30, 0, 0],
            },
            priority: 0,
        };
        let retained = partitions.plan_interest(&[within_hysteresis]).unwrap();
        assert!(retained.desired.contains(&PartitionId(1)));
        assert!(retained.unload.is_empty());

        let outside = InterestVolume {
            bounds: PartitionBounds {
                min: [31, 0, 0],
                max: [31, 0, 0],
            },
            priority: 0,
        };
        let unload = partitions.plan_interest(&[outside]).unwrap();
        assert!(unload.desired.is_empty());
        assert_eq!(unload.unload, vec![PartitionId(1)]);
    }

    #[test]
    fn dependency_inherits_priority_and_concurrent_slots_defer_stable_order() {
        let mut config = WorldPartitionConfig::default();
        config.max_concurrent_loads = 1;
        let mut partitions = WorldPartition::new(config).unwrap();
        let bounds = PartitionBounds {
            min: [0, 0, 0],
            max: [10, 10, 10],
        };
        let mut dependency = manifest(1, Vec::new(), Vec::new());
        dependency.bounds = bounds;
        dependency.priority = 0;
        let mut high = manifest(2, vec![PartitionId(1)], Vec::new());
        high.bounds = bounds;
        high.priority = 100;
        let mut medium = manifest(3, Vec::new(), Vec::new());
        medium.bounds = bounds;
        medium.priority = 50;
        partitions
            .register_batch(vec![medium, high, dependency])
            .unwrap();
        let plan = partitions
            .plan_interest(&[InterestVolume {
                bounds,
                priority: 0,
            }])
            .unwrap();
        assert_eq!(plan.begin_load, vec![PartitionId(1)]);
        assert_eq!(plan.deferred_load, vec![PartitionId(2), PartitionId(3)]);
        assert_eq!(
            plan.desired,
            BTreeSet::from([PartitionId(1), PartitionId(2), PartitionId(3)])
        );
    }

    #[test]
    fn priority_budget_selection_is_stable_across_registration_order() {
        let mut config = WorldPartitionConfig::default();
        config.max_active_objects = 2;
        let bounds = PartitionBounds {
            min: [0, 0, 0],
            max: [10, 10, 10],
        };
        let make_manifests = || {
            [(1, 1), (2, 10), (3, 5)]
                .into_iter()
                .map(|(id, priority)| {
                    let mut item = manifest(id, Vec::new(), Vec::new());
                    item.bounds = bounds;
                    item.priority = priority;
                    item.estimated_objects = 1;
                    item
                })
                .collect::<Vec<_>>()
        };
        let mut first = WorldPartition::new(config).unwrap();
        first.register_batch(make_manifests()).unwrap();
        let mut reversed = make_manifests();
        reversed.reverse();
        let mut second = WorldPartition::new(config).unwrap();
        second.register_batch(reversed).unwrap();
        let interest = InterestVolume {
            bounds,
            priority: 0,
        };
        let first_plan = first.plan_interest(&[interest]).unwrap();
        let second_plan = second.plan_interest(&[interest]).unwrap();
        assert_eq!(first_plan, second_plan);
        assert_eq!(
            first_plan.desired,
            BTreeSet::from([PartitionId(2), PartitionId(3)])
        );
        assert_eq!(first_plan.budget_excluded, vec![PartitionId(1)]);
    }

    #[test]
    fn failed_dependency_blocks_dependents_and_plans_dependent_first_cleanup() {
        let (mut assets, refs) = asset_server();
        let mut partitions = WorldPartition::new(WorldPartitionConfig::default()).unwrap();
        let bounds = PartitionBounds {
            min: [0, 0, 0],
            max: [10, 10, 10],
        };
        let mut dependency = manifest(1, Vec::new(), vec![refs[0]]);
        dependency.bounds = bounds;
        let mut dependent = manifest(2, vec![PartitionId(1)], Vec::new());
        dependent.bounds = bounds;
        partitions
            .register_batch(vec![dependency, dependent])
            .unwrap();
        partitions
            .begin_load(PartitionId(1), &mut assets, 10)
            .unwrap();
        partitions
            .begin_load(PartitionId(2), &mut assets, 10)
            .unwrap();
        let expired = assets.expire(10);
        partitions.observe_results(&expired).unwrap();
        assert_eq!(
            partitions.state(PartitionId(1)),
            Some(PartitionState::Failed)
        );
        assert_eq!(
            partitions.state(PartitionId(2)),
            Some(PartitionState::Ready)
        );
        let plan = partitions
            .plan_interest(&[InterestVolume {
                bounds,
                priority: 0,
            }])
            .unwrap();
        assert!(plan.desired.is_empty());
        assert!(plan.begin_load.is_empty());
        assert_eq!(plan.blocked_by_failure, vec![PartitionId(2)]);
        assert_eq!(plan.unload, vec![PartitionId(2), PartitionId(1)]);
    }
}
