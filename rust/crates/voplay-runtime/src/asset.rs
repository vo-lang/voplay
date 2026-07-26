use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssetId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssetRef {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetNodeHandle {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AssetScopeKind {
    Engine,
    Scene,
    Mode,
    Temporary,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssetScopeId {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssetTicket {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetRegistration {
    pub asset_id: AssetId,
    pub asset_type: u64,
    pub source_revision: u64,
    pub artifact_id: ArtifactId,
    pub dependencies: Vec<AssetId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuNodeState {
    Requested,
    CpuReady,
    Reloading,
    Failed,
    Evicted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetTicketOutcome {
    Ready,
    Cancelled,
    TimedOut,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetTicketResult {
    pub ticket: AssetTicket,
    pub asset_ref: AssetRef,
    pub outcome: AssetTicketOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetWork {
    pub ticket: AssetTicket,
    pub asset_ref: AssetRef,
    pub asset_id: AssetId,
    pub source_revision: u64,
    pub artifact_id: ArtifactId,
    pub endpoint_generation: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetInspectionEntry {
    pub asset_ref: AssetRef,
    pub asset_id: AssetId,
    pub asset_type: u64,
    pub source_revision: u64,
    pub artifact_id: ArtifactId,
    pub dependencies: Vec<AssetId>,
    pub state: CpuNodeState,
    pub lease_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetServerConfig {
    pub max_assets: usize,
    pub max_dependencies_per_asset: usize,
    pub max_scopes: usize,
    pub max_tickets: usize,
}

impl Default for AssetServerConfig {
    fn default() -> Self {
        Self {
            max_assets: 100_000,
            max_dependencies_per_asset: 256,
            max_scopes: 4096,
            max_tickets: 65_536,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetError {
    InvalidConfig,
    WrongEngine,
    InvalidIdentity,
    AssetCapacity,
    DuplicateAsset,
    UnknownAsset,
    DependencyCapacity,
    UnknownDependency,
    DependencyCycle,
    StaleSourceRevision,
    ScopeCapacity,
    UnknownScope,
    ScopeClosed,
    ScopeNotClosed,
    ScopeInUse,
    TicketCapacity,
    UnknownTicket,
    TicketNotDispatched,
    TicketNotTerminal,
    TicketTerminal,
    StaleEndpoint,
    LateCompletion,
    GenerationExhausted,
    Closed,
    ResidencyCapacity,
    UnknownResidency,
    UploadNotDispatched,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResidencyKey {
    pub device: Handle,
    pub usage: u64,
    pub format: u32,
    pub quality: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResidencyHandle {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidencyState {
    UploadQueued,
    Uploading,
    Ready,
    Failed,
    Evicted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResidencyConfig {
    pub max_records: usize,
    pub max_ready_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UploadWork {
    pub asset_ref: AssetRef,
    pub artifact_id: ArtifactId,
    pub key: ResidencyKey,
    pub endpoint_generation: Handle,
    pub handle: ResidencyHandle,
    pub byte_cost: usize,
}

#[derive(Debug)]
struct ResidencyRecord {
    artifact_id: ArtifactId,
    state: ResidencyState,
    handle: Handle,
    byte_cost: usize,
}

pub struct ResidencyStore {
    engine: EngineId,
    endpoint_generation: Handle,
    config: ResidencyConfig,
    records: BTreeMap<(AssetRef, ResidencyKey), ResidencyRecord>,
    next_handle: u32,
    ready_bytes: usize,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResidencyStoreOwnerSnapshot {
    pub closed: bool,
    pub records: usize,
    pub queued_uploads: usize,
    pub inflight_uploads: usize,
    pub ready_records: usize,
    pub ready_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResidencyStoreShutdownReport {
    pub released_records: usize,
    pub cancelled_queued_uploads: usize,
    pub abandoned_inflight_uploads: usize,
    pub released_ready_records: usize,
    pub released_ready_bytes: usize,
}

impl ResidencyStore {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: ResidencyConfig,
    ) -> Result<Self, AssetError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_records == 0
            || config.max_ready_bytes == 0
        {
            return Err(AssetError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            records: BTreeMap::new(),
            next_handle: 0,
            ready_bytes: 0,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn endpoint_generation(&self) -> Handle {
        self.endpoint_generation
    }

    pub fn owner_snapshot(&self) -> ResidencyStoreOwnerSnapshot {
        ResidencyStoreOwnerSnapshot {
            closed: self.closed,
            records: self.records.len(),
            queued_uploads: self
                .records
                .values()
                .filter(|record| record.state == ResidencyState::UploadQueued)
                .count(),
            inflight_uploads: self
                .records
                .values()
                .filter(|record| record.state == ResidencyState::Uploading)
                .count(),
            ready_records: self
                .records
                .values()
                .filter(|record| record.state == ResidencyState::Ready)
                .count(),
            ready_bytes: self.ready_bytes,
        }
    }

    pub fn shutdown(&mut self) -> ResidencyStoreShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.records.clear();
        self.ready_bytes = 0;
        ResidencyStoreShutdownReport {
            released_records: snapshot.records,
            cancelled_queued_uploads: snapshot.queued_uploads,
            abandoned_inflight_uploads: snapshot.inflight_uploads,
            released_ready_records: snapshot.ready_records,
            released_ready_bytes: snapshot.ready_bytes,
        }
    }

    pub fn request(
        &mut self,
        asset_ref: AssetRef,
        artifact_id: ArtifactId,
        key: ResidencyKey,
        byte_cost: usize,
    ) -> Result<ResidencyHandle, AssetError> {
        self.ensure_open()?;
        if asset_ref.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if artifact_id.0 == [0; 16] || !key.device.is_valid() || byte_cost == 0 {
            return Err(AssetError::InvalidIdentity);
        }
        if let Some(record) = self.records.get_mut(&(asset_ref, key)) {
            if record.artifact_id != artifact_id {
                let generation = record
                    .handle
                    .generation
                    .checked_add(1)
                    .ok_or(AssetError::GenerationExhausted)?;
                if record.state == ResidencyState::Ready {
                    self.ready_bytes -= record.byte_cost;
                }
                record.handle.generation = generation;
                record.artifact_id = artifact_id;
                record.byte_cost = byte_cost;
                record.state = ResidencyState::UploadQueued;
            } else if matches!(
                record.state,
                ResidencyState::Failed | ResidencyState::Evicted
            ) {
                record.state = ResidencyState::UploadQueued;
            }
            let handle = record.handle;
            return Ok(ResidencyHandle {
                engine: self.engine,
                endpoint_generation: self.endpoint_generation,
                handle,
            });
        }
        if self.records.len() == self.config.max_records {
            return Err(AssetError::ResidencyCapacity);
        }
        let index = self.next_handle;
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        let handle = Handle {
            index,
            generation: 1,
        };
        self.records.insert(
            (asset_ref, key),
            ResidencyRecord {
                artifact_id,
                state: ResidencyState::UploadQueued,
                handle,
                byte_cost,
            },
        );
        Ok(self.public_handle(handle))
    }

    pub fn poll_upload(&mut self) -> Option<UploadWork> {
        if self.closed {
            return None;
        }
        let ((asset_ref, key), record) = self
            .records
            .iter_mut()
            .find(|(_, record)| record.state == ResidencyState::UploadQueued)?;
        record.state = ResidencyState::Uploading;
        Some(UploadWork {
            asset_ref: *asset_ref,
            artifact_id: record.artifact_id,
            key: *key,
            endpoint_generation: self.endpoint_generation,
            handle: ResidencyHandle {
                engine: self.engine,
                endpoint_generation: self.endpoint_generation,
                handle: record.handle,
            },
            byte_cost: record.byte_cost,
        })
    }

    pub fn complete_upload(
        &mut self,
        work: UploadWork,
        success: bool,
    ) -> Result<ResidencyState, AssetError> {
        self.ensure_open()?;
        if work.asset_ref.engine != self.engine || work.handle.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if work.endpoint_generation != self.endpoint_generation
            || work.handle.endpoint_generation != self.endpoint_generation
        {
            return Err(AssetError::StaleEndpoint);
        }
        let record = self
            .records
            .get_mut(&(work.asset_ref, work.key))
            .ok_or(AssetError::UnknownResidency)?;
        if record.artifact_id != work.artifact_id || record.handle != work.handle.handle {
            return Err(AssetError::LateCompletion);
        }
        if record.state != ResidencyState::Uploading {
            return Err(AssetError::UploadNotDispatched);
        }
        if !success {
            record.state = ResidencyState::Failed;
            return Ok(record.state);
        }
        let ready_bytes = self
            .ready_bytes
            .checked_add(record.byte_cost)
            .filter(|bytes| *bytes <= self.config.max_ready_bytes)
            .ok_or(AssetError::ResidencyCapacity)?;
        self.ready_bytes = ready_bytes;
        record.state = ResidencyState::Ready;
        Ok(record.state)
    }

    pub fn state(&self, asset_ref: AssetRef, key: ResidencyKey) -> Option<ResidencyState> {
        self.records
            .get(&(asset_ref, key))
            .map(|record| record.state)
    }

    pub fn device_lost(&mut self, device: Handle) -> usize {
        if self.closed {
            return 0;
        }
        let mut evicted = 0;
        for ((_, key), record) in &mut self.records {
            if key.device == device && record.state != ResidencyState::Evicted {
                if record.state == ResidencyState::Ready {
                    self.ready_bytes -= record.byte_cost;
                }
                record.state = ResidencyState::Evicted;
                evicted += 1;
            }
        }
        evicted
    }

    pub fn release(&mut self, asset_ref: AssetRef, key: ResidencyKey) -> Result<(), AssetError> {
        self.ensure_open()?;
        if asset_ref.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        let record = self
            .records
            .remove(&(asset_ref, key))
            .ok_or(AssetError::UnknownResidency)?;
        if record.state == ResidencyState::Ready {
            self.ready_bytes = self
                .ready_bytes
                .checked_sub(record.byte_cost)
                .ok_or(AssetError::UnknownResidency)?;
        }
        Ok(())
    }

    pub fn restart_endpoint(&mut self) -> Result<(), AssetError> {
        self.ensure_open()?;
        let generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        if self
            .records
            .values()
            .any(|record| record.handle.generation == u32::MAX)
        {
            return Err(AssetError::GenerationExhausted);
        }
        self.endpoint_generation.generation = generation;
        self.ready_bytes = 0;
        for record in self.records.values_mut() {
            record.handle.generation += 1;
            record.state = ResidencyState::UploadQueued;
        }
        Ok(())
    }

    fn public_handle(&self, handle: Handle) -> ResidencyHandle {
        ResidencyHandle {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle,
        }
    }

    fn ensure_open(&self) -> Result<(), AssetError> {
        if self.closed {
            Err(AssetError::Closed)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
struct AssetNode {
    stable: AssetRef,
    registration: AssetRegistration,
    pending_reload: Option<AssetRegistration>,
    reload_fallback_ready: bool,
    state: CpuNodeState,
    leases: usize,
}

#[derive(Debug)]
struct ScopeRecord {
    kind: AssetScopeKind,
    open: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TicketState {
    Pending,
    Dispatched,
    Terminal,
}

#[derive(Debug)]
struct TicketRecord {
    asset_ref: AssetRef,
    scope: AssetScopeId,
    deadline_millis: u64,
    state: TicketState,
}

pub struct AssetServer {
    engine: EngineId,
    endpoint_generation: Handle,
    config: AssetServerConfig,
    nodes: BTreeMap<AssetId, AssetNode>,
    refs: Vec<AssetId>,
    scopes: BTreeMap<AssetScopeId, ScopeRecord>,
    tickets: BTreeMap<AssetTicket, TicketRecord>,
    next_scope: u32,
    next_ticket: u32,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetServerOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub closed: bool,
    pub assets: usize,
    pub scopes: usize,
    pub open_scopes: usize,
    pub tickets: usize,
    pub pending_tickets: usize,
    pub dispatched_tickets: usize,
    pub terminal_tickets: usize,
    pub asset_leases: usize,
    pub pending_reloads: usize,
}

impl AssetServer {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: AssetServerConfig,
    ) -> Result<Self, AssetError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_assets == 0
            || config.max_assets > u32::MAX as usize
            || config.max_dependencies_per_asset == 0
            || config.max_scopes == 0
            || config.max_tickets == 0
        {
            return Err(AssetError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            nodes: BTreeMap::new(),
            refs: Vec::new(),
            scopes: BTreeMap::new(),
            tickets: BTreeMap::new(),
            next_scope: 0,
            next_ticket: 0,
            closed: false,
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn endpoint_generation(&self) -> Handle {
        self.endpoint_generation
    }

    pub fn owner_snapshot(&self) -> AssetServerOwnerSnapshot {
        let mut pending_tickets = 0;
        let mut dispatched_tickets = 0;
        let mut terminal_tickets = 0;
        for ticket in self.tickets.values() {
            match ticket.state {
                TicketState::Pending => pending_tickets += 1,
                TicketState::Dispatched => dispatched_tickets += 1,
                TicketState::Terminal => terminal_tickets += 1,
            }
        }
        AssetServerOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            closed: self.closed,
            assets: self.nodes.len(),
            scopes: self.scopes.len(),
            open_scopes: self.scopes.values().filter(|scope| scope.open).count(),
            tickets: self.tickets.len(),
            pending_tickets,
            dispatched_tickets,
            terminal_tickets,
            asset_leases: self.nodes.values().map(|node| node.leases).sum(),
            pending_reloads: self
                .nodes
                .values()
                .filter(|node| node.pending_reload.is_some())
                .count(),
        }
    }

    pub fn register_batch(
        &mut self,
        registrations: Vec<AssetRegistration>,
    ) -> Result<Vec<AssetRef>, AssetError> {
        self.ensure_open()?;
        if self.nodes.len().saturating_add(registrations.len()) > self.config.max_assets {
            return Err(AssetError::AssetCapacity);
        }
        let mut graph = self.graph();
        let mut ids = BTreeSet::new();
        for item in &registrations {
            validate_registration(item, &self.config)?;
            if self.nodes.contains_key(&item.asset_id) || !ids.insert(item.asset_id) {
                return Err(AssetError::DuplicateAsset);
            }
            graph.insert(item.asset_id, item.dependencies.clone());
        }
        validate_graph(&graph)?;
        let mut result = Vec::with_capacity(registrations.len());
        for registration in registrations {
            let stable = AssetRef {
                engine: self.engine,
                handle: Handle {
                    index: self.refs.len() as u32,
                    generation: 1,
                },
            };
            self.refs.push(registration.asset_id);
            self.nodes.insert(
                registration.asset_id,
                AssetNode {
                    stable,
                    registration,
                    pending_reload: None,
                    reload_fallback_ready: false,
                    state: CpuNodeState::Requested,
                    leases: 0,
                },
            );
            result.push(stable);
        }
        Ok(result)
    }

    pub fn hot_reload(&mut self, registration: AssetRegistration) -> Result<AssetRef, AssetError> {
        self.ensure_open()?;
        validate_registration(&registration, &self.config)?;
        let current = self
            .nodes
            .get(&registration.asset_id)
            .ok_or(AssetError::UnknownAsset)?;
        let newest_revision = current
            .pending_reload
            .as_ref()
            .map_or(current.registration.source_revision, |pending| {
                pending.source_revision
            });
        if registration.source_revision <= newest_revision {
            return Err(AssetError::StaleSourceRevision);
        }
        let stable = current.stable;
        let mut graph = self.graph();
        graph.insert(registration.asset_id, registration.dependencies.clone());
        validate_graph(&graph)?;
        let node = self.nodes.get_mut(&registration.asset_id).unwrap();
        if node.pending_reload.is_none() {
            node.reload_fallback_ready = node.state == CpuNodeState::CpuReady;
        }
        node.pending_reload = Some(registration);
        node.state = CpuNodeState::Reloading;
        for ticket in self.tickets.values_mut() {
            if ticket.asset_ref == stable && ticket.state == TicketState::Dispatched {
                ticket.state = TicketState::Pending;
            }
        }
        Ok(stable)
    }

    pub fn resolve_node(&self, asset_ref: AssetRef) -> Result<AssetNodeHandle, AssetError> {
        self.node(asset_ref)?;
        Ok(AssetNodeHandle {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            handle: asset_ref.handle,
        })
    }

    pub fn node_state(&self, asset_ref: AssetRef) -> Option<CpuNodeState> {
        self.node(asset_ref).ok().map(|node| node.state)
    }

    pub fn inspection_revision(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        hash_asset_u32(&mut hash, self.endpoint_generation.index);
        hash_asset_u32(&mut hash, self.endpoint_generation.generation);
        for entry in self.inspection_assets() {
            hash_asset_bytes(&mut hash, &entry.asset_id.0);
            hash_asset_u32(&mut hash, entry.asset_ref.handle.index);
            hash_asset_u32(&mut hash, entry.asset_ref.handle.generation);
            hash_asset_u64(&mut hash, entry.asset_type);
            hash_asset_u64(&mut hash, entry.source_revision);
            hash_asset_bytes(&mut hash, &entry.artifact_id.0);
            hash_asset_u32(&mut hash, cpu_state_tag(entry.state));
            hash_asset_u64(&mut hash, entry.lease_count as u64);
            for dependency in entry.dependencies {
                hash_asset_bytes(&mut hash, &dependency.0);
            }
        }
        hash
    }

    pub fn inspection_assets(&self) -> Vec<AssetInspectionEntry> {
        self.nodes
            .iter()
            .map(|(asset_id, node)| {
                let mut dependencies = node.registration.dependencies.clone();
                dependencies.sort_unstable();
                AssetInspectionEntry {
                    asset_ref: node.stable,
                    asset_id: *asset_id,
                    asset_type: node.registration.asset_type,
                    source_revision: node.registration.source_revision,
                    artifact_id: node.registration.artifact_id,
                    dependencies,
                    state: node.state,
                    lease_count: node.leases,
                }
            })
            .collect()
    }

    pub fn create_scope(&mut self, kind: AssetScopeKind) -> Result<AssetScopeId, AssetError> {
        self.ensure_open()?;
        if self.scopes.len() == self.config.max_scopes {
            return Err(AssetError::ScopeCapacity);
        }
        let index = self.next_scope;
        self.next_scope = self
            .next_scope
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        let scope = AssetScopeId {
            engine: self.engine,
            handle: Handle {
                index,
                generation: 1,
            },
        };
        self.scopes.insert(scope, ScopeRecord { kind, open: true });
        Ok(scope)
    }

    pub fn create_scope_with_requests(
        &mut self,
        kind: AssetScopeKind,
        asset_refs: &[AssetRef],
        deadline_millis: u64,
    ) -> Result<(AssetScopeId, Vec<AssetTicket>), AssetError> {
        self.ensure_open()?;
        if self.scopes.len() == self.config.max_scopes {
            return Err(AssetError::ScopeCapacity);
        }
        if self.tickets.len().saturating_add(asset_refs.len()) > self.config.max_tickets {
            return Err(AssetError::TicketCapacity);
        }
        let mut unique = BTreeSet::new();
        for asset_ref in asset_refs {
            self.node(*asset_ref)?;
            if !unique.insert(*asset_ref) {
                return Err(AssetError::InvalidIdentity);
            }
        }
        let scope_index = self.next_scope;
        let next_scope = self
            .next_scope
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        let next_ticket = self
            .next_ticket
            .checked_add(u32::try_from(asset_refs.len()).map_err(|_| AssetError::TicketCapacity)?)
            .ok_or(AssetError::GenerationExhausted)?;
        let scope = AssetScopeId {
            engine: self.engine,
            handle: Handle {
                index: scope_index,
                generation: 1,
            },
        };
        self.next_scope = next_scope;
        self.scopes.insert(scope, ScopeRecord { kind, open: true });
        let mut tickets = Vec::with_capacity(asset_refs.len());
        for (offset, asset_ref) in asset_refs.iter().enumerate() {
            let ticket = AssetTicket {
                engine: self.engine,
                handle: Handle {
                    index: self.next_ticket + offset as u32,
                    generation: 1,
                },
            };
            self.node_mut(*asset_ref)?.leases += 1;
            self.tickets.insert(
                ticket,
                TicketRecord {
                    asset_ref: *asset_ref,
                    scope,
                    deadline_millis,
                    state: TicketState::Pending,
                },
            );
            tickets.push(ticket);
        }
        self.next_ticket = next_ticket;
        Ok((scope, tickets))
    }

    pub fn scope_kind(&self, scope: AssetScopeId) -> Option<AssetScopeKind> {
        self.scopes.get(&scope).map(|scope| scope.kind)
    }

    pub fn scope_is_open(&self, scope: AssetScopeId) -> Option<bool> {
        self.scopes.get(&scope).map(|scope| scope.open)
    }

    pub fn request(
        &mut self,
        scope: AssetScopeId,
        asset_ref: AssetRef,
        deadline_millis: u64,
    ) -> Result<AssetTicket, AssetError> {
        self.ensure_open()?;
        self.validate_scope(scope)?;
        self.node(asset_ref)?;
        if self.tickets.len() == self.config.max_tickets {
            return Err(AssetError::TicketCapacity);
        }
        let index = self.next_ticket;
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        let ticket = AssetTicket {
            engine: self.engine,
            handle: Handle {
                index,
                generation: 1,
            },
        };
        self.node_mut(asset_ref)?.leases += 1;
        self.tickets.insert(
            ticket,
            TicketRecord {
                asset_ref,
                scope,
                deadline_millis,
                state: TicketState::Pending,
            },
        );
        Ok(ticket)
    }

    pub fn poll_work(&mut self) -> Option<AssetWork> {
        let ticket = self
            .tickets
            .iter()
            .find(|(_, record)| record.state == TicketState::Pending)
            .map(|(ticket, _)| *ticket)?;
        let asset_ref = self.tickets[&ticket].asset_ref;
        self.tickets.get_mut(&ticket).unwrap().state = TicketState::Dispatched;
        let node = self.node(asset_ref).ok()?;
        let registration = node.pending_reload.as_ref().unwrap_or(&node.registration);
        Some(AssetWork {
            ticket,
            asset_ref,
            asset_id: registration.asset_id,
            source_revision: registration.source_revision,
            artifact_id: registration.artifact_id,
            endpoint_generation: self.endpoint_generation,
        })
    }

    pub fn complete(&mut self, work: &AssetWork) -> Result<Vec<AssetTicketResult>, AssetError> {
        self.ensure_open()?;
        if work.asset_ref.engine != self.engine || work.ticket.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if work.endpoint_generation != self.endpoint_generation {
            return Err(AssetError::StaleEndpoint);
        }
        let node = self.node(work.asset_ref)?;
        let expected = node.pending_reload.as_ref().unwrap_or(&node.registration);
        if expected.source_revision != work.source_revision
            || expected.artifact_id != work.artifact_id
        {
            return Err(AssetError::LateCompletion);
        }
        let ticket = self
            .tickets
            .get(&work.ticket)
            .ok_or(AssetError::UnknownTicket)?;
        if ticket.state != TicketState::Dispatched || ticket.asset_ref != work.asset_ref {
            return Err(AssetError::TicketNotDispatched);
        }
        let node = self.node_mut(work.asset_ref)?;
        if let Some(registration) = node.pending_reload.take() {
            node.registration = registration;
        }
        node.reload_fallback_ready = false;
        node.state = CpuNodeState::CpuReady;
        let tickets = self
            .tickets
            .iter()
            .filter(|(_, record)| {
                record.asset_ref == work.asset_ref
                    && matches!(record.state, TicketState::Pending | TicketState::Dispatched)
            })
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        Ok(tickets
            .into_iter()
            .map(|ticket| {
                self.finish_ticket(ticket, AssetTicketOutcome::Ready)
                    .unwrap()
            })
            .collect())
    }

    pub fn fail(&mut self, work: &AssetWork) -> Result<Vec<AssetTicketResult>, AssetError> {
        self.ensure_open()?;
        if work.asset_ref.engine != self.engine || work.ticket.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if work.endpoint_generation != self.endpoint_generation {
            return Err(AssetError::StaleEndpoint);
        }
        let node = self.node(work.asset_ref)?;
        let expected = node.pending_reload.as_ref().unwrap_or(&node.registration);
        if expected.asset_id != work.asset_id
            || expected.source_revision != work.source_revision
            || expected.artifact_id != work.artifact_id
        {
            return Err(AssetError::LateCompletion);
        }
        let ticket = self
            .tickets
            .get(&work.ticket)
            .ok_or(AssetError::UnknownTicket)?;
        if ticket.state != TicketState::Dispatched || ticket.asset_ref != work.asset_ref {
            return Err(AssetError::TicketNotDispatched);
        }
        let node = self.node_mut(work.asset_ref)?;
        if node.pending_reload.take().is_some() && node.reload_fallback_ready {
            node.state = CpuNodeState::CpuReady;
        } else {
            node.state = CpuNodeState::Failed;
        }
        node.reload_fallback_ready = false;
        let tickets = self
            .tickets
            .iter()
            .filter(|(_, record)| {
                record.asset_ref == work.asset_ref
                    && matches!(record.state, TicketState::Pending | TicketState::Dispatched)
            })
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        Ok(tickets
            .into_iter()
            .map(|ticket| {
                self.finish_ticket(ticket, AssetTicketOutcome::Failed)
                    .unwrap()
            })
            .collect())
    }

    pub fn cancel(&mut self, ticket: AssetTicket) -> Result<AssetTicketResult, AssetError> {
        self.finish_ticket(ticket, AssetTicketOutcome::Cancelled)
    }

    pub fn expire(&mut self, now_millis: u64) -> Vec<AssetTicketResult> {
        let tickets = self
            .tickets
            .iter()
            .filter(|(_, record)| {
                record.state != TicketState::Terminal && record.deadline_millis <= now_millis
            })
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        tickets
            .into_iter()
            .filter_map(|ticket| {
                self.finish_ticket(ticket, AssetTicketOutcome::TimedOut)
                    .ok()
            })
            .collect()
    }

    pub fn close_scope(
        &mut self,
        scope: AssetScopeId,
    ) -> Result<Vec<AssetTicketResult>, AssetError> {
        self.validate_scope(scope)?;
        self.scopes.get_mut(&scope).unwrap().open = false;
        let tickets = self
            .tickets
            .iter()
            .filter(|(_, record)| record.scope == scope && record.state != TicketState::Terminal)
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        Ok(tickets
            .into_iter()
            .filter_map(|ticket| {
                self.finish_ticket(ticket, AssetTicketOutcome::Cancelled)
                    .ok()
            })
            .collect())
    }

    pub fn release_ticket(&mut self, ticket: AssetTicket) -> Result<(), AssetError> {
        if ticket.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if self
            .tickets
            .get(&ticket)
            .ok_or(AssetError::UnknownTicket)?
            .state
            != TicketState::Terminal
        {
            return Err(AssetError::TicketNotTerminal);
        }
        self.tickets.remove(&ticket);
        Ok(())
    }

    pub fn release_scope(&mut self, scope: AssetScopeId) -> Result<(), AssetError> {
        if scope.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        if self
            .scopes
            .get(&scope)
            .ok_or(AssetError::UnknownScope)?
            .open
        {
            return Err(AssetError::ScopeNotClosed);
        }
        if self.tickets.values().any(|ticket| ticket.scope == scope) {
            return Err(AssetError::ScopeInUse);
        }
        self.scopes.remove(&scope);
        Ok(())
    }

    pub fn restart_endpoint(&mut self) -> Result<(), AssetError> {
        let generation = self
            .endpoint_generation
            .generation
            .checked_add(1)
            .ok_or(AssetError::GenerationExhausted)?;
        self.endpoint_generation.generation = generation;
        for ticket in self.tickets.values_mut() {
            if ticket.state == TicketState::Dispatched {
                ticket.state = TicketState::Pending;
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Vec<AssetTicketResult> {
        self.closed = true;
        let tickets = self
            .tickets
            .iter()
            .filter(|(_, record)| record.state != TicketState::Terminal)
            .map(|(ticket, _)| *ticket)
            .collect::<Vec<_>>();
        let results = tickets
            .into_iter()
            .filter_map(|ticket| self.finish_ticket(ticket, AssetTicketOutcome::Closed).ok())
            .collect();
        self.tickets.clear();
        self.scopes.clear();
        self.nodes.clear();
        self.refs.clear();
        results
    }

    fn finish_ticket(
        &mut self,
        ticket: AssetTicket,
        outcome: AssetTicketOutcome,
    ) -> Result<AssetTicketResult, AssetError> {
        if ticket.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        let record = self
            .tickets
            .get_mut(&ticket)
            .ok_or(AssetError::UnknownTicket)?;
        if record.state == TicketState::Terminal {
            return Err(AssetError::TicketTerminal);
        }
        record.state = TicketState::Terminal;
        let asset_ref = record.asset_ref;
        let node = self.node_mut(asset_ref)?;
        node.leases -= 1;
        if node.leases == 0 {
            if node.state == CpuNodeState::Reloading && node.reload_fallback_ready {
                node.pending_reload = None;
                node.reload_fallback_ready = false;
                node.state = CpuNodeState::CpuReady;
            } else if matches!(
                node.state,
                CpuNodeState::Requested | CpuNodeState::Reloading
            ) {
                node.pending_reload = None;
                node.reload_fallback_ready = false;
                node.state = CpuNodeState::Evicted;
            }
        }
        Ok(AssetTicketResult {
            ticket,
            asset_ref,
            outcome,
        })
    }

    fn validate_scope(&self, scope: AssetScopeId) -> Result<(), AssetError> {
        if scope.engine != self.engine {
            return Err(AssetError::WrongEngine);
        }
        self.scopes
            .get(&scope)
            .ok_or(AssetError::UnknownScope)?
            .open
            .then_some(())
            .ok_or(AssetError::ScopeClosed)
    }

    fn node(&self, asset_ref: AssetRef) -> Result<&AssetNode, AssetError> {
        if asset_ref.engine != self.engine || !asset_ref.handle.is_valid() {
            return Err(AssetError::WrongEngine);
        }
        let id = self
            .refs
            .get(asset_ref.handle.index as usize)
            .ok_or(AssetError::UnknownAsset)?;
        let node = self.nodes.get(id).ok_or(AssetError::UnknownAsset)?;
        (node.stable == asset_ref)
            .then_some(node)
            .ok_or(AssetError::UnknownAsset)
    }

    fn node_mut(&mut self, asset_ref: AssetRef) -> Result<&mut AssetNode, AssetError> {
        self.node(asset_ref)?;
        let id = self.refs[asset_ref.handle.index as usize];
        Ok(self.nodes.get_mut(&id).unwrap())
    }

    fn graph(&self) -> BTreeMap<AssetId, Vec<AssetId>> {
        self.nodes
            .iter()
            .map(|(id, node)| (*id, node.registration.dependencies.clone()))
            .collect()
    }

    fn ensure_open(&self) -> Result<(), AssetError> {
        (!self.closed).then_some(()).ok_or(AssetError::Closed)
    }
}

fn validate_registration(
    item: &AssetRegistration,
    config: &AssetServerConfig,
) -> Result<(), AssetError> {
    if item.asset_id.0 == [0; 16]
        || item.artifact_id.0 == [0; 16]
        || item.asset_type == 0
        || item.source_revision == 0
    {
        return Err(AssetError::InvalidIdentity);
    }
    if item.dependencies.len() > config.max_dependencies_per_asset {
        return Err(AssetError::DependencyCapacity);
    }
    let unique = item.dependencies.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != item.dependencies.len() || unique.contains(&item.asset_id) {
        return Err(AssetError::DependencyCycle);
    }
    Ok(())
}

fn validate_graph(graph: &BTreeMap<AssetId, Vec<AssetId>>) -> Result<(), AssetError> {
    if graph
        .values()
        .flatten()
        .any(|dependency| !graph.contains_key(dependency))
    {
        return Err(AssetError::UnknownDependency);
    }
    fn visit(
        id: AssetId,
        graph: &BTreeMap<AssetId, Vec<AssetId>>,
        active: &mut BTreeSet<AssetId>,
        done: &mut BTreeSet<AssetId>,
    ) -> Result<(), AssetError> {
        if done.contains(&id) {
            return Ok(());
        }
        if !active.insert(id) {
            return Err(AssetError::DependencyCycle);
        }
        for dependency in &graph[&id] {
            visit(*dependency, graph, active, done)?;
        }
        active.remove(&id);
        done.insert(id);
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    for id in graph.keys() {
        visit(*id, graph, &mut active, &mut done)?;
    }
    Ok(())
}

fn cpu_state_tag(state: CpuNodeState) -> u32 {
    match state {
        CpuNodeState::Requested => 1,
        CpuNodeState::CpuReady => 2,
        CpuNodeState::Reloading => 3,
        CpuNodeState::Failed => 4,
        CpuNodeState::Evicted => 5,
    }
}

fn hash_asset_u32(hash: &mut u64, value: u32) {
    hash_asset_bytes(hash, &value.to_le_bytes());
}

fn hash_asset_u64(hash: &mut u64, value: u64) {
    hash_asset_bytes(hash, &value.to_le_bytes());
}

fn hash_asset_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = (*hash).wrapping_mul(0x100000001b3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn id(value: u8) -> AssetId {
        AssetId([value; 16])
    }

    fn registration(value: u8, revision: u64, dependencies: Vec<AssetId>) -> AssetRegistration {
        AssetRegistration {
            asset_id: id(value),
            asset_type: 1,
            source_revision: revision,
            artifact_id: ArtifactId([value.wrapping_add(revision as u8); 16]),
            dependencies,
        }
    }

    fn server() -> AssetServer {
        AssetServer::new(
            handle(1),
            handle(10),
            AssetServerConfig {
                max_assets: 8,
                max_dependencies_per_asset: 4,
                max_scopes: 4,
                max_tickets: 8,
            },
        )
        .unwrap()
    }

    fn key(device: u32) -> ResidencyKey {
        ResidencyKey {
            device: handle(device),
            usage: 1,
            format: 2,
            quality: 3,
        }
    }

    #[test]
    fn dag_errors_and_reload_cycle_leave_graph_and_stable_ref_unchanged() {
        let mut server = server();
        assert_eq!(
            server.register_batch(vec![registration(1, 1, vec![id(9)])]),
            Err(AssetError::UnknownDependency)
        );
        let refs = server
            .register_batch(vec![
                registration(1, 1, Vec::new()),
                registration(2, 1, vec![id(1)]),
            ])
            .unwrap();
        assert_eq!(
            server.hot_reload(registration(1, 2, vec![id(2)])),
            Err(AssetError::DependencyCycle)
        );
        assert_eq!(server.node_state(refs[0]), Some(CpuNodeState::Requested));
    }

    #[test]
    fn concurrent_requests_share_ready_and_scope_timeout_cancel_are_terminal() {
        let mut server = server();
        let asset_ref = server
            .register_batch(vec![registration(1, 1, Vec::new())])
            .unwrap()[0];
        let scope = server.create_scope(AssetScopeKind::Scene).unwrap();
        let first = server.request(scope, asset_ref, 100).unwrap();
        let second = server.request(scope, asset_ref, 5).unwrap();
        let work = server.poll_work().unwrap();
        assert_eq!(work.ticket, first);
        let ready = server.complete(&work).unwrap();
        assert_eq!(ready.len(), 2);
        assert!(ready.iter().any(|result| result.ticket == second));
        assert_eq!(server.cancel(first), Err(AssetError::TicketTerminal));
        assert_eq!(server.node_state(asset_ref), Some(CpuNodeState::CpuReady));

        let expiring = server.request(scope, asset_ref, 5).unwrap();
        assert_eq!(server.expire(5)[0].ticket, expiring);
        let closing = server.request(scope, asset_ref, 100).unwrap();
        assert_eq!(server.close_scope(scope).unwrap()[0].ticket, closing);
        assert_eq!(
            server.request(scope, asset_ref, 100),
            Err(AssetError::ScopeClosed)
        );
    }

    #[test]
    fn terminal_ticket_and_closed_scope_release_recover_concurrent_quotas() {
        let mut server = AssetServer::new(
            handle(1),
            handle(10),
            AssetServerConfig {
                max_assets: 1,
                max_dependencies_per_asset: 1,
                max_scopes: 1,
                max_tickets: 2,
            },
        )
        .unwrap();
        let asset_ref = server
            .register_batch(vec![registration(1, 1, Vec::new())])
            .unwrap()[0];
        let scope = server.create_scope(AssetScopeKind::Scene).unwrap();
        assert_eq!(server.release_scope(scope), Err(AssetError::ScopeNotClosed));
        let first = server.request(scope, asset_ref, 10).unwrap();
        let second = server.request(scope, asset_ref, 10).unwrap();
        assert_eq!(
            server.request(scope, asset_ref, 10),
            Err(AssetError::TicketCapacity)
        );
        assert_eq!(
            server.release_ticket(first),
            Err(AssetError::TicketNotTerminal)
        );
        let work = server.poll_work().unwrap();
        let results = server.complete(&work).unwrap();
        assert_eq!(results.len(), 2);
        server.release_ticket(first).unwrap();
        server.release_ticket(second).unwrap();

        let third = server.request(scope, asset_ref, 10).unwrap();
        assert_ne!(third, first);
        server.cancel(third).unwrap();
        server.close_scope(scope).unwrap();
        assert_eq!(server.release_scope(scope), Err(AssetError::ScopeInUse));
        server.release_ticket(third).unwrap();
        server.release_scope(scope).unwrap();
        assert!(server.create_scope(AssetScopeKind::Mode).is_ok());
    }

    #[test]
    fn reload_and_endpoint_restart_keep_ref_and_reject_late_work() {
        let mut server = server();
        let asset_ref = server
            .register_batch(vec![registration(1, 1, Vec::new())])
            .unwrap()[0];
        let scope = server.create_scope(AssetScopeKind::Engine).unwrap();
        let ticket = server.request(scope, asset_ref, 100).unwrap();
        let old_work = server.poll_work().unwrap();
        let old_node = server.resolve_node(asset_ref).unwrap();
        assert_eq!(
            server.hot_reload(registration(1, 2, Vec::new())).unwrap(),
            asset_ref
        );
        assert_eq!(server.complete(&old_work), Err(AssetError::LateCompletion));
        let reload_work = server.poll_work().unwrap();
        server.restart_endpoint().unwrap();
        assert_eq!(
            server.complete(&reload_work),
            Err(AssetError::StaleEndpoint)
        );
        let new_node = server.resolve_node(asset_ref).unwrap();
        assert_eq!(new_node.handle, old_node.handle);
        assert_ne!(new_node.endpoint_generation, old_node.endpoint_generation);
        let current = server.poll_work().unwrap();
        assert_eq!(server.complete(&current).unwrap()[0].ticket, ticket);
    }

    #[test]
    fn shutdown_closes_pending_without_waiting_for_worker() {
        let mut server = server();
        let asset_ref = server
            .register_batch(vec![registration(1, 1, Vec::new())])
            .unwrap()[0];
        let scope = server.create_scope(AssetScopeKind::Temporary).unwrap();
        let ticket = server.request(scope, asset_ref, 100).unwrap();
        assert_eq!(server.shutdown()[0].ticket, ticket);
        assert_eq!(server.poll_work(), None);
        assert_eq!(
            server.request(scope, asset_ref, 100),
            Err(AssetError::Closed)
        );
    }

    #[test]
    fn residency_isolated_by_device_generation_and_device_lost_scope() {
        let asset_ref = AssetRef {
            engine: handle(1),
            handle: handle(7),
        };
        let mut store = ResidencyStore::new(
            handle(1),
            handle(20),
            ResidencyConfig {
                max_records: 4,
                max_ready_bytes: 16,
            },
        )
        .unwrap();
        store
            .request(asset_ref, ArtifactId([1; 16]), key(1), 4)
            .unwrap();
        store
            .request(asset_ref, ArtifactId([1; 16]), key(2), 4)
            .unwrap();
        let first = store.poll_upload().unwrap();
        let second = store.poll_upload().unwrap();
        store.complete_upload(first, true).unwrap();
        store.complete_upload(second, true).unwrap();
        assert_eq!(store.device_lost(handle(1)), 1);
        assert_eq!(
            store.state(asset_ref, key(1)),
            Some(ResidencyState::Evicted)
        );
        assert_eq!(store.state(asset_ref, key(2)), Some(ResidencyState::Ready));
    }

    #[test]
    fn artifact_change_and_endpoint_restart_reject_late_upload_and_requeue_current() {
        let asset_ref = AssetRef {
            engine: handle(1),
            handle: handle(7),
        };
        let mut store = ResidencyStore::new(
            handle(1),
            handle(20),
            ResidencyConfig {
                max_records: 2,
                max_ready_bytes: 16,
            },
        )
        .unwrap();
        let old_handle = store
            .request(asset_ref, ArtifactId([1; 16]), key(1), 4)
            .unwrap();
        let old_work = store.poll_upload().unwrap();
        let reload_handle = store
            .request(asset_ref, ArtifactId([2; 16]), key(1), 4)
            .unwrap();
        assert_ne!(old_handle.handle, reload_handle.handle);
        assert_eq!(
            store.complete_upload(old_work, true),
            Err(AssetError::LateCompletion)
        );
        let reload_work = store.poll_upload().unwrap();
        store.restart_endpoint().unwrap();
        assert_eq!(
            store.complete_upload(reload_work, true),
            Err(AssetError::StaleEndpoint)
        );
        let current = store.poll_upload().unwrap();
        assert_ne!(
            current.handle.endpoint_generation,
            old_handle.endpoint_generation
        );
        assert_eq!(
            store.complete_upload(current, true),
            Ok(ResidencyState::Ready)
        );
    }
}
