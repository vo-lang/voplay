use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, VecDeque},
};

use voplay_protocol::{EngineId, EntityId};

#[cfg(feature = "animation")]
pub mod animation;
#[cfg(feature = "animation")]
pub mod animation_gpu;
#[cfg(feature = "animation")]
pub mod animation_presentation;
#[cfg(feature = "app-session")]
pub mod app_runtime_bridge;
#[cfg(feature = "app-session")]
pub mod app_session_adapter;
pub mod asset;
pub mod asset_endpoint;
pub mod audio;
pub mod audio_control;
#[cfg(feature = "audio")]
pub mod audio_device_adapter;
#[cfg(feature = "audio")]
pub mod audio_endpoint;
pub mod audio_mixer;
pub mod audio_pipeline;
pub mod audio_streaming;
pub mod buffer_lease;
pub mod control;
pub mod control_authority_wire;
pub mod control_wire;
pub mod device_hub;
#[cfg(feature = "inspection")]
pub mod editor_inspection;
#[cfg(feature = "inspection")]
pub mod editor_tools;
pub mod endpoint;
pub mod endpoint_group;
pub mod engine_error;
#[cfg(feature = "inspection")]
pub mod fault_injection;
#[cfg(feature = "inspection")]
pub mod fault_wire;
#[cfg(feature = "inspection")]
pub mod frame_debug_capture;
#[cfg(feature = "inspection")]
pub mod frame_debug_wire;
pub mod game_engine;
pub mod haptics;
pub mod haptics_wire;
pub mod headless_renderer;
pub mod hierarchy;
pub mod host_render_backend;
pub mod hosted_logic_endpoint;
pub mod input;
#[cfg(feature = "inspection")]
pub mod inspection;
#[cfg(feature = "inspection")]
pub mod inspection_endpoint;
pub mod instrumentation;
pub mod logic_endpoint;
pub mod logic_io;
#[cfg(feature = "inspection")]
pub mod native_frame_trace;
pub mod outbox;
#[cfg(feature = "pack")]
pub mod partition_chunk_codec;
#[cfg(feature = "pack")]
pub mod partition_io;
#[cfg(feature = "physics")]
pub mod physics;
#[cfg(feature = "render")]
pub mod platform_backend;
pub mod platform_haptics;
#[cfg(feature = "render")]
pub mod platform_recovery;
#[cfg(feature = "inspection")]
pub mod play_mode;
pub mod plugin;
pub mod presentation;
pub mod presentation_state;
pub mod profile;
pub mod query;
pub mod readback_wire;
pub mod render_asset_outbox;
pub mod render_commands;
pub mod render_control;
#[cfg(feature = "render")]
pub mod render_endpoint;
#[cfg(feature = "render")]
pub mod render_feature;
pub mod render_graph;
pub mod render_graph_wire;
pub mod render_readback;
pub mod render_wire;
pub mod render_world;
pub mod runtime;
pub mod scene;
pub mod schedule;
pub mod simulation_state;
pub mod sparse_store;
#[cfg(feature = "render2d")]
pub mod sprite_animation;
pub mod supervisor;
pub mod surface;
pub mod world;
#[cfg(feature = "pack")]
pub mod world_partition;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RenderEntity {
    pub engine: EngineId,
    pub entity: EntityId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub max_render_objects: usize,
    pub max_transaction_ops: usize,
    pub max_transaction_bytes: usize,
    pub max_snapshot_objects: usize,
    pub max_snapshot_bytes: usize,
    pub max_staged_events: usize,
    pub max_staged_event_bytes: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_render_objects: 1_000_000,
            max_transaction_ops: 131_072,
            max_transaction_bytes: 4 * 1024 * 1024,
            max_snapshot_objects: 1_000_000,
            max_snapshot_bytes: 64 * 1024 * 1024,
            max_staged_events: 4096,
            max_staged_event_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderOp {
    Spawn {
        entity: RenderEntity,
        value: Vec<u8>,
    },
    Despawn {
        entity: RenderEntity,
    },
    Upsert {
        entity: RenderEntity,
        value: Vec<u8>,
    },
}

impl RenderOp {
    fn entity(&self) -> RenderEntity {
        match self {
            Self::Spawn { entity, .. } | Self::Despawn { entity } | Self::Upsert { entity, .. } => {
                *entity
            }
        }
    }

    fn byte_len(&self) -> usize {
        match self {
            Self::Spawn { value, .. } | Self::Upsert { value, .. } => value.len(),
            Self::Despawn { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderStateTransaction {
    pub engine: EngineId,
    pub channel_epoch: u64,
    pub commit_id: u64,
    pub base_revision: u64,
    pub new_revision: u64,
    pub required_control_revision: u64,
    pub source_simulation_revision: u64,
    pub ops: Vec<RenderOp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderStateSnapshot {
    pub engine: EngineId,
    pub channel_epoch: u64,
    pub commit_id: u64,
    pub revision: u64,
    pub required_control_revision: u64,
    pub source_simulation_revision: u64,
    pub objects: BTreeMap<RenderEntity, Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderCommitAck {
    pub commit_id: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderEvent {
    pub engine: EngineId,
    pub sequence: u64,
    pub event_id: u64,
    pub min_render_revision: u64,
    pub required_control_revision: u64,
    pub deadline_millis: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderEventOutcome {
    Executed,
    DroppedBeforeDispatch,
    OutcomeUnknown,
    Failed,
    FailedControlUnavailable,
    DeadlineExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderEventResult {
    pub sequence: u64,
    pub event_id: u64,
    pub outcome: RenderEventOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderEventPoll {
    Dispatch(RenderEvent),
    Terminal(RenderEventResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    InvalidConfig,
    WrongEngine,
    StaleChannel,
    DuplicateCommit,
    BaseRevisionMismatch,
    InvalidRevision,
    ControlRevisionUnavailable,
    TransactionCapacity,
    SnapshotCapacity,
    DuplicateEntityOperation,
    InvalidEntity,
    EntityAlreadyExists,
    EntityNotFound,
    RenderObjectCapacity,
    EventSequence,
    EventCapacity,
    ChannelEpochExhausted,
    ClockRegression,
    Closed,
}

#[derive(Clone, Debug)]
struct StagedEvent {
    event: RenderEvent,
    dispatched: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineWorkCounters {
    pub render_transactions: u64,
    pub transaction_ops: u64,
    pub transaction_bytes: u64,
    pub render_snapshots: u64,
    pub snapshot_objects: u64,
    pub snapshot_bytes: u64,
    pub delta_frames: u64,
    pub delta_scanned_entities: u64,
    pub delta_encoded_entities: u64,
    pub delta_encoded_bytes: u64,
    pub delta_despawned_entities: u64,
    pub stable_delta_frames: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderEngineOwnerSnapshot {
    pub engine: EngineId,
    pub channel_epoch: u64,
    pub closed: bool,
    pub render_objects: usize,
    pub pending_upserts: usize,
    pub pending_spawns: usize,
    pub pending_despawns: usize,
    pub staged_events: usize,
    pub staged_event_bytes: usize,
    pub completed_events: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderEngineShutdownReport {
    pub channel_epoch: u64,
    pub released_render_objects: usize,
    pub released_pending_changes: usize,
    pub terminal_events: Vec<RenderEventResult>,
}

impl EngineWorkCounters {
    fn add(self, other: Self) -> Self {
        Self {
            render_transactions: self
                .render_transactions
                .saturating_add(other.render_transactions),
            transaction_ops: self.transaction_ops.saturating_add(other.transaction_ops),
            transaction_bytes: self
                .transaction_bytes
                .saturating_add(other.transaction_bytes),
            render_snapshots: self.render_snapshots.saturating_add(other.render_snapshots),
            snapshot_objects: self.snapshot_objects.saturating_add(other.snapshot_objects),
            snapshot_bytes: self.snapshot_bytes.saturating_add(other.snapshot_bytes),
            delta_frames: self.delta_frames.saturating_add(other.delta_frames),
            delta_scanned_entities: self
                .delta_scanned_entities
                .saturating_add(other.delta_scanned_entities),
            delta_encoded_entities: self
                .delta_encoded_entities
                .saturating_add(other.delta_encoded_entities),
            delta_encoded_bytes: self
                .delta_encoded_bytes
                .saturating_add(other.delta_encoded_bytes),
            delta_despawned_entities: self
                .delta_despawned_entities
                .saturating_add(other.delta_despawned_entities),
            stable_delta_frames: self
                .stable_delta_frames
                .saturating_add(other.stable_delta_frames),
        }
    }
}

pub struct Engine {
    id: EngineId,
    config: EngineConfig,
    channel_epoch: u64,
    render_revision: u64,
    render_control_revision: u64,
    last_commit_id: u64,
    render_world: BTreeMap<RenderEntity, Vec<u8>>,
    pending_render_upserts: BTreeSet<RenderEntity>,
    pending_render_spawns: BTreeSet<RenderEntity>,
    pending_render_despawns: BTreeSet<RenderEntity>,
    render_snapshot_required: bool,
    last_event_sequence: u64,
    now_millis: u64,
    staged_event_bytes: usize,
    staged_events: VecDeque<StagedEvent>,
    event_completions: BTreeMap<u64, RenderEventResult>,
    next_event_ack_sequence: u64,
    work_counters_enabled: bool,
    work_counters: Cell<EngineWorkCounters>,
    closed: bool,
}

impl Engine {
    pub fn new(id: EngineId, config: EngineConfig) -> Result<Self, EngineError> {
        if !id.is_valid()
            || config.max_render_objects == 0
            || config.max_transaction_ops == 0
            || config.max_transaction_bytes == 0
            || config.max_snapshot_objects == 0
            || config.max_snapshot_bytes == 0
            || config.max_staged_events == 0
            || config.max_staged_event_bytes == 0
        {
            return Err(EngineError::InvalidConfig);
        }
        Ok(Self {
            id,
            config,
            channel_epoch: 1,
            render_revision: 0,
            render_control_revision: 0,
            last_commit_id: 0,
            render_world: BTreeMap::new(),
            pending_render_upserts: BTreeSet::new(),
            pending_render_spawns: BTreeSet::new(),
            pending_render_despawns: BTreeSet::new(),
            render_snapshot_required: true,
            last_event_sequence: 0,
            now_millis: 0,
            staged_event_bytes: 0,
            staged_events: VecDeque::new(),
            event_completions: BTreeMap::new(),
            next_event_ack_sequence: 1,
            work_counters_enabled: false,
            work_counters: Cell::new(EngineWorkCounters::default()),
            closed: false,
        })
    }

    pub const fn id(&self) -> EngineId {
        self.id
    }

    pub fn owner_snapshot(&self) -> RenderEngineOwnerSnapshot {
        RenderEngineOwnerSnapshot {
            engine: self.id,
            channel_epoch: self.channel_epoch,
            closed: self.closed,
            render_objects: self.render_world.len(),
            pending_upserts: self.pending_render_upserts.len(),
            pending_spawns: self.pending_render_spawns.len(),
            pending_despawns: self.pending_render_despawns.len(),
            staged_events: self.staged_events.len(),
            staged_event_bytes: self.staged_event_bytes,
            completed_events: self.event_completions.len(),
        }
    }

    pub const fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub const fn channel_epoch(&self) -> u64 {
        self.channel_epoch
    }

    pub const fn render_control_revision(&self) -> u64 {
        self.render_control_revision
    }

    pub fn set_work_counters_enabled(&mut self, enabled: bool) {
        self.work_counters_enabled = enabled;
        if !enabled {
            self.work_counters.set(EngineWorkCounters::default());
        }
    }

    pub fn work_counters(&self) -> EngineWorkCounters {
        self.work_counters.get()
    }

    pub fn take_work_counters(&self) -> EngineWorkCounters {
        self.work_counters.replace(EngineWorkCounters::default())
    }

    fn record_work(&self, counters: EngineWorkCounters) {
        if self.work_counters_enabled {
            self.work_counters
                .set(self.work_counters.get().add(counters));
        }
    }

    pub fn render_object(&self, entity: RenderEntity) -> Option<&[u8]> {
        if entity.engine != self.id {
            return None;
        }
        self.render_world.get(&entity).map(Vec::as_slice)
    }

    pub fn set_render_control_revision(&mut self, revision: u64) {
        if self.closed {
            return;
        }
        self.render_control_revision = self.render_control_revision.max(revision);
    }

    pub fn apply_render_state(
        &mut self,
        transaction: RenderStateTransaction,
    ) -> Result<RenderCommitAck, EngineError> {
        self.ensure_open()?;
        self.validate_transaction(&transaction)?;
        self.preflight_ops(&transaction.ops)?;
        for op in &transaction.ops {
            match op {
                RenderOp::Spawn { entity, value } => {
                    let replaced = self.render_world.insert(*entity, value.clone());
                    debug_assert!(replaced.is_none());
                    let replaced_pending = self.pending_render_despawns.remove(entity);
                    if !replaced_pending {
                        self.pending_render_spawns.insert(*entity);
                    }
                    self.pending_render_upserts.insert(*entity);
                }
                RenderOp::Despawn { entity } => {
                    let removed = self.render_world.remove(entity);
                    debug_assert!(removed.is_some());
                    self.pending_render_upserts.remove(entity);
                    if !self.pending_render_spawns.remove(entity) {
                        self.pending_render_despawns.insert(*entity);
                    }
                }
                RenderOp::Upsert { entity, value } => {
                    let current = self
                        .render_world
                        .get_mut(entity)
                        .expect("preflight guarantees a live render object");
                    *current = value.clone();
                    self.pending_render_upserts.insert(*entity);
                }
            }
        }
        self.render_revision = transaction.new_revision;
        self.last_commit_id = transaction.commit_id;
        self.record_work(EngineWorkCounters {
            render_transactions: 1,
            transaction_ops: transaction.ops.len() as u64,
            transaction_bytes: transaction
                .ops
                .iter()
                .map(RenderOp::byte_len)
                .fold(0_u64, |total, bytes| total.saturating_add(bytes as u64)),
            ..EngineWorkCounters::default()
        });
        Ok(RenderCommitAck {
            commit_id: transaction.commit_id,
            revision: transaction.new_revision,
        })
    }

    pub fn apply_render_snapshot(
        &mut self,
        snapshot: RenderStateSnapshot,
    ) -> Result<RenderCommitAck, EngineError> {
        self.ensure_open()?;
        if snapshot.engine != self.id {
            return Err(EngineError::WrongEngine);
        }
        if snapshot.channel_epoch != self.channel_epoch {
            return Err(EngineError::StaleChannel);
        }
        if snapshot.commit_id <= self.last_commit_id {
            return Err(EngineError::DuplicateCommit);
        }
        if snapshot.revision < self.render_revision {
            return Err(EngineError::InvalidRevision);
        }
        if snapshot.required_control_revision > self.render_control_revision {
            return Err(EngineError::ControlRevisionUnavailable);
        }
        if snapshot.objects.len() > self.config.max_render_objects
            || snapshot.objects.len() > self.config.max_snapshot_objects
        {
            return Err(EngineError::SnapshotCapacity);
        }
        let bytes = snapshot
            .objects
            .values()
            .try_fold(0_usize, |total, value| total.checked_add(value.len()));
        if bytes
            .map(|bytes| bytes > self.config.max_snapshot_bytes)
            .unwrap_or(true)
        {
            return Err(EngineError::SnapshotCapacity);
        }
        if snapshot
            .objects
            .keys()
            .any(|entity| entity.engine != self.id || !entity.entity.is_valid())
        {
            return Err(EngineError::InvalidEntity);
        }
        let commit_id = snapshot.commit_id;
        let revision = snapshot.revision;
        let snapshot_objects = snapshot.objects.len() as u64;
        let snapshot_bytes = bytes.unwrap_or(usize::MAX) as u64;
        self.render_world = snapshot.objects;
        self.pending_render_upserts.clear();
        self.pending_render_spawns.clear();
        self.pending_render_despawns.clear();
        self.render_snapshot_required = true;
        self.render_revision = revision;
        self.last_commit_id = commit_id;
        self.record_work(EngineWorkCounters {
            render_snapshots: 1,
            snapshot_objects,
            snapshot_bytes,
            ..EngineWorkCounters::default()
        });
        Ok(RenderCommitAck {
            commit_id,
            revision,
        })
    }

    pub fn prepare_render_delta(&self) -> presentation::RenderFrameDelta {
        let full_snapshot = self.render_snapshot_required;
        let objects: Vec<(RenderEntity, Vec<u8>)> = if full_snapshot {
            self.render_world
                .iter()
                .map(|(entity, bytes)| (*entity, bytes.clone()))
                .collect()
        } else {
            self.pending_render_upserts
                .iter()
                .filter_map(|entity| {
                    self.render_world
                        .get(entity)
                        .map(|bytes| (*entity, bytes.clone()))
                })
                .collect()
        };
        let despawned = if full_snapshot {
            Vec::new()
        } else {
            self.pending_render_despawns.iter().copied().collect()
        };
        let encoded_bytes = objects.iter().fold(0_u64, |total, (_, bytes)| {
            total.saturating_add(bytes.len() as u64)
        });
        self.record_work(EngineWorkCounters {
            delta_frames: 1,
            delta_scanned_entities: if full_snapshot {
                self.render_world.len() as u64
            } else {
                self.pending_render_upserts.len() as u64
            },
            delta_encoded_entities: objects.len() as u64,
            delta_encoded_bytes: encoded_bytes,
            delta_despawned_entities: despawned.len() as u64,
            stable_delta_frames: u64::from(objects.is_empty() && despawned.is_empty()),
            ..EngineWorkCounters::default()
        });
        presentation::RenderFrameDelta {
            revision: self.render_revision,
            full_snapshot,
            objects,
            despawned,
        }
    }

    pub fn commit_render_delta(&mut self, revision: u64) -> Result<(), EngineError> {
        self.ensure_open()?;
        if revision != self.render_revision {
            return Err(EngineError::InvalidRevision);
        }
        self.pending_render_upserts.clear();
        self.pending_render_spawns.clear();
        self.pending_render_despawns.clear();
        self.render_snapshot_required = false;
        Ok(())
    }

    fn preflight_ops(&self, ops: &[RenderOp]) -> Result<(), EngineError> {
        let mut final_count = self.render_world.len();
        for op in ops {
            match op {
                RenderOp::Spawn { entity, .. } => {
                    if self.render_world.contains_key(entity) {
                        return Err(EngineError::EntityAlreadyExists);
                    }
                    final_count = final_count
                        .checked_add(1)
                        .ok_or(EngineError::RenderObjectCapacity)?;
                    if final_count > self.config.max_render_objects {
                        return Err(EngineError::RenderObjectCapacity);
                    }
                }
                RenderOp::Despawn { entity } => {
                    if !self.render_world.contains_key(entity) {
                        return Err(EngineError::EntityNotFound);
                    }
                    final_count -= 1;
                }
                RenderOp::Upsert { entity, .. } => {
                    if !self.render_world.contains_key(entity) {
                        return Err(EngineError::EntityNotFound);
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_transaction(
        &self,
        transaction: &RenderStateTransaction,
    ) -> Result<(), EngineError> {
        if transaction.engine != self.id {
            return Err(EngineError::WrongEngine);
        }
        if transaction.channel_epoch != self.channel_epoch {
            return Err(EngineError::StaleChannel);
        }
        if transaction.commit_id <= self.last_commit_id {
            return Err(EngineError::DuplicateCommit);
        }
        if transaction.base_revision != self.render_revision {
            return Err(EngineError::BaseRevisionMismatch);
        }
        if transaction.base_revision.checked_add(1) != Some(transaction.new_revision) {
            return Err(EngineError::InvalidRevision);
        }
        if transaction.required_control_revision > self.render_control_revision {
            return Err(EngineError::ControlRevisionUnavailable);
        }
        let bytes = transaction
            .ops
            .iter()
            .try_fold(0_usize, |total, op| total.checked_add(op.byte_len()));
        if transaction.ops.len() > self.config.max_transaction_ops
            || bytes
                .map(|bytes| bytes > self.config.max_transaction_bytes)
                .unwrap_or(true)
        {
            return Err(EngineError::TransactionCapacity);
        }
        let mut entities = BTreeSet::new();
        for op in &transaction.ops {
            if op.entity().engine != self.id || !op.entity().entity.is_valid() {
                return Err(EngineError::InvalidEntity);
            }
            if !entities.insert(op.entity()) {
                return Err(EngineError::DuplicateEntityOperation);
            }
        }
        Ok(())
    }

    pub fn stage_event(&mut self, event: RenderEvent) -> Result<(), EngineError> {
        self.ensure_open()?;
        if event.engine != self.id {
            return Err(EngineError::WrongEngine);
        }
        if self.last_event_sequence.checked_add(1) != Some(event.sequence) || event.event_id == 0 {
            return Err(EngineError::EventSequence);
        }
        let bytes = self
            .staged_event_bytes
            .checked_add(event.payload.len())
            .filter(|bytes| *bytes <= self.config.max_staged_event_bytes)
            .ok_or(EngineError::EventCapacity)?;
        if self
            .staged_events
            .len()
            .saturating_add(self.event_completions.len())
            == self.config.max_staged_events
        {
            return Err(EngineError::EventCapacity);
        }
        self.last_event_sequence = event.sequence;
        self.staged_event_bytes = bytes;
        self.staged_events.push_back(StagedEvent {
            event,
            dispatched: false,
        });
        Ok(())
    }

    pub fn advance_time(&mut self, now_millis: u64) -> Result<(), EngineError> {
        self.ensure_open()?;
        if now_millis < self.now_millis {
            return Err(EngineError::ClockRegression);
        }
        self.now_millis = now_millis;
        Ok(())
    }

    pub fn next_event_deadline_millis(&self) -> Option<u64> {
        self.staged_events
            .iter()
            .map(|staged| staged.event.deadline_millis)
            .min()
    }

    pub fn poll_event(&mut self) -> Option<RenderEventPoll> {
        if let Some(index) = self
            .staged_events
            .iter()
            .position(|staged| staged.event.deadline_millis <= self.now_millis)
        {
            let staged = self.staged_events.remove(index).unwrap();
            self.staged_event_bytes -= staged.event.payload.len();
            let result = RenderEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    RenderEventOutcome::OutcomeUnknown
                } else {
                    RenderEventOutcome::DeadlineExceeded
                },
            };
            self.event_completions.insert(result.sequence, result);
            return Some(RenderEventPoll::Terminal(result));
        }
        let staged = self.staged_events.iter_mut().find(|staged| {
            !staged.dispatched
                && staged.event.min_render_revision <= self.render_revision
                && staged.event.required_control_revision <= self.render_control_revision
        })?;
        staged.dispatched = true;
        Some(RenderEventPoll::Dispatch(staged.event.clone()))
    }

    pub fn complete_event(
        &mut self,
        sequence: u64,
        outcome: RenderEventOutcome,
    ) -> Result<RenderEventResult, EngineError> {
        self.ensure_open()?;
        let index = self
            .staged_events
            .iter()
            .position(|staged| staged.event.sequence == sequence)
            .ok_or(EngineError::EventSequence)?;
        if !self.staged_events[index].dispatched {
            return Err(EngineError::EventSequence);
        }
        let staged = self.staged_events.remove(index).unwrap();
        self.staged_event_bytes -= staged.event.payload.len();
        let result = RenderEventResult {
            sequence,
            event_id: staged.event.event_id,
            outcome: if staged.event.deadline_millis <= self.now_millis {
                RenderEventOutcome::OutcomeUnknown
            } else {
                outcome
            },
        };
        self.event_completions.insert(sequence, result);
        Ok(result)
    }

    pub fn fail_staged_event_control_unavailable(
        &mut self,
        sequence: u64,
    ) -> Result<RenderEventResult, EngineError> {
        self.ensure_open()?;
        let index = self
            .staged_events
            .iter()
            .position(|staged| staged.event.sequence == sequence)
            .ok_or(EngineError::EventSequence)?;
        if self.staged_events[index].dispatched {
            return Err(EngineError::EventSequence);
        }
        let staged = self.staged_events.remove(index).unwrap();
        self.staged_event_bytes -= staged.event.payload.len();
        let result = RenderEventResult {
            sequence,
            event_id: staged.event.event_id,
            outcome: RenderEventOutcome::FailedControlUnavailable,
        };
        self.event_completions.insert(sequence, result);
        Ok(result)
    }

    pub fn drain_contiguous_event_results(&mut self) -> Vec<RenderEventResult> {
        let mut results = Vec::new();
        while let Some(result) = self.event_completions.remove(&self.next_event_ack_sequence) {
            results.push(result);
            self.next_event_ack_sequence = self
                .next_event_ack_sequence
                .checked_add(1)
                .unwrap_or(u64::MAX);
        }
        results
    }

    pub fn restart_renderer(&mut self) -> Result<Vec<RenderEventResult>, EngineError> {
        self.ensure_open()?;
        self.channel_epoch = self
            .channel_epoch
            .checked_add(1)
            .ok_or(EngineError::ChannelEpochExhausted)?;
        self.render_revision = 0;
        self.render_world.clear();
        self.pending_render_upserts.clear();
        self.pending_render_spawns.clear();
        self.pending_render_despawns.clear();
        self.render_snapshot_required = true;
        self.staged_event_bytes = 0;
        let synthesized = self
            .staged_events
            .drain(..)
            .map(|staged| RenderEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    RenderEventOutcome::OutcomeUnknown
                } else {
                    RenderEventOutcome::DroppedBeforeDispatch
                },
            })
            .collect::<Vec<_>>();
        for result in synthesized {
            self.event_completions.insert(result.sequence, result);
        }
        Ok(self.drain_contiguous_event_results())
    }

    pub fn preflight_shutdown(&self) -> Result<(), EngineError> {
        if self.closed {
            return Ok(());
        }
        self.channel_epoch
            .checked_add(1)
            .ok_or(EngineError::ChannelEpochExhausted)?;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<RenderEngineShutdownReport, EngineError> {
        if self.closed {
            return Ok(RenderEngineShutdownReport {
                channel_epoch: self.channel_epoch,
                released_render_objects: 0,
                released_pending_changes: 0,
                terminal_events: Vec::new(),
            });
        }
        self.preflight_shutdown()?;
        let released_render_objects = self.render_world.len();
        let released_pending_changes = self
            .pending_render_upserts
            .len()
            .saturating_add(self.pending_render_spawns.len())
            .saturating_add(self.pending_render_despawns.len());
        for staged in self.staged_events.drain(..) {
            let result = RenderEventResult {
                sequence: staged.event.sequence,
                event_id: staged.event.event_id,
                outcome: if staged.dispatched {
                    RenderEventOutcome::OutcomeUnknown
                } else {
                    RenderEventOutcome::DroppedBeforeDispatch
                },
            };
            self.event_completions.insert(result.sequence, result);
        }
        let terminal_events = std::mem::take(&mut self.event_completions)
            .into_values()
            .collect();
        self.channel_epoch += 1;
        self.render_revision = 0;
        self.render_control_revision = 0;
        self.last_commit_id = 0;
        self.render_world.clear();
        self.pending_render_upserts.clear();
        self.pending_render_spawns.clear();
        self.pending_render_despawns.clear();
        self.render_snapshot_required = true;
        self.staged_event_bytes = 0;
        self.closed = true;
        Ok(RenderEngineShutdownReport {
            channel_epoch: self.channel_epoch,
            released_render_objects,
            released_pending_changes,
            terminal_events,
        })
    }

    fn ensure_open(&self) -> Result<(), EngineError> {
        if self.closed {
            Err(EngineError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn engine(index: u32) -> Engine {
        Engine::new(
            handle(index),
            EngineConfig {
                max_render_objects: 4,
                max_transaction_ops: 4,
                max_transaction_bytes: 16,
                max_snapshot_objects: 4,
                max_snapshot_bytes: 32,
                max_staged_events: 4,
                max_staged_event_bytes: 16,
            },
        )
        .unwrap()
    }

    fn transaction(engine: EngineId, ops: Vec<RenderOp>) -> RenderStateTransaction {
        RenderStateTransaction {
            engine,
            channel_epoch: 1,
            commit_id: 1,
            base_revision: 0,
            new_revision: 1,
            required_control_revision: 0,
            source_simulation_revision: 1,
            ops,
        }
    }

    fn entity(engine: EngineId, index: u32) -> RenderEntity {
        RenderEntity {
            engine,
            entity: handle(index),
        }
    }

    fn event(
        engine: EngineId,
        sequence: u64,
        min_render_revision: u64,
        required_control_revision: u64,
    ) -> RenderEvent {
        RenderEvent {
            engine,
            sequence,
            event_id: 100 + sequence,
            min_render_revision,
            required_control_revision,
            deadline_millis: 100,
            payload: vec![sequence as u8],
        }
    }

    #[test]
    fn lifecycle_and_value_apply_atomically() {
        let mut engine = engine(0);
        let entity = entity(engine.id(), 7);
        let mut tx = transaction(
            engine.id(),
            vec![
                RenderOp::Spawn {
                    entity,
                    value: b"first".to_vec(),
                },
                RenderOp::Upsert {
                    entity,
                    value: b"second".to_vec(),
                },
            ],
        );
        assert_eq!(
            engine.apply_render_state(tx.clone()),
            Err(EngineError::DuplicateEntityOperation)
        );
        assert_eq!(engine.render_revision(), 0);
        assert!(engine.render_object(entity).is_none());

        tx.ops.pop();
        assert_eq!(engine.apply_render_state(tx).unwrap().revision, 1);
        assert_eq!(engine.render_object(entity), Some(b"first".as_slice()));
    }

    #[test]
    fn two_engines_keep_world_and_revision_isolated() {
        let mut alpha = engine(0);
        let mut beta = engine(1);
        let entity = entity(alpha.id(), 2);
        alpha
            .apply_render_state(transaction(
                alpha.id(),
                vec![RenderOp::Spawn {
                    entity,
                    value: vec![1],
                }],
            ))
            .unwrap();
        assert_eq!(alpha.render_revision(), 1);
        assert_eq!(beta.render_revision(), 0);
        assert!(beta.render_object(entity).is_none());
        assert_eq!(
            beta.apply_render_state(transaction(
                beta.id(),
                vec![RenderOp::Spawn {
                    entity,
                    value: vec![2],
                }]
            )),
            Err(EngineError::InvalidEntity)
        );
        assert_eq!(
            beta.apply_render_state(transaction(alpha.id(), Vec::new())),
            Err(EngineError::WrongEngine)
        );
    }

    #[test]
    fn render_event_waits_for_both_revision_barriers() {
        let mut engine = engine(0);
        engine.stage_event(event(engine.id(), 1, 1, 2)).unwrap();
        assert!(engine.poll_event().is_none());
        engine
            .apply_render_state(transaction(engine.id(), Vec::new()))
            .unwrap();
        assert!(engine.poll_event().is_none());
        engine.set_render_control_revision(2);
        assert!(matches!(
            engine.poll_event(),
            Some(RenderEventPoll::Dispatch(RenderEvent { sequence: 1, .. }))
        ));
        assert!(engine.poll_event().is_none());
        assert_eq!(
            engine
                .complete_event(1, RenderEventOutcome::Executed)
                .unwrap()
                .outcome,
            RenderEventOutcome::Executed
        );
    }

    #[test]
    fn renderer_restart_reports_honest_one_shot_outcomes() {
        let mut engine = engine(0);
        engine.stage_event(event(engine.id(), 1, 0, 0)).unwrap();
        engine.stage_event(event(engine.id(), 2, 9, 0)).unwrap();
        engine.poll_event().unwrap();
        assert_eq!(
            engine.restart_renderer().unwrap(),
            vec![
                RenderEventResult {
                    sequence: 1,
                    event_id: 101,
                    outcome: RenderEventOutcome::OutcomeUnknown,
                },
                RenderEventResult {
                    sequence: 2,
                    event_id: 102,
                    outcome: RenderEventOutcome::DroppedBeforeDispatch,
                },
            ]
        );
    }

    #[test]
    fn renderer_epoch_exhaustion_preserves_last_good_state_and_events() {
        let mut engine = engine(0);
        engine.stage_event(event(engine.id(), 1, 0, 0)).unwrap();
        engine.channel_epoch = u64::MAX;
        assert_eq!(
            engine.restart_renderer(),
            Err(EngineError::ChannelEpochExhausted)
        );
        assert_eq!(engine.channel_epoch, u64::MAX);
        assert_eq!(engine.staged_events.len(), 1);
    }

    #[test]
    fn render_event_owner_deadline_and_control_failure_are_terminal_without_partial_admission() {
        let mut engine = engine(0);
        let mut foreign = event(handle(9), 1, 0, 0);
        foreign.payload = vec![1; 17];
        assert_eq!(engine.stage_event(foreign), Err(EngineError::WrongEngine));
        let mut oversized = event(engine.id(), 1, 0, 0);
        oversized.payload = vec![1; 17];
        assert_eq!(
            engine.stage_event(oversized),
            Err(EngineError::EventCapacity)
        );
        engine.stage_event(event(engine.id(), 1, 0, 0)).unwrap();
        assert_eq!(
            engine.fail_staged_event_control_unavailable(1).unwrap(),
            RenderEventResult {
                sequence: 1,
                event_id: 101,
                outcome: RenderEventOutcome::FailedControlUnavailable,
            }
        );
        let mut expiring = event(engine.id(), 2, 9, 0);
        expiring.deadline_millis = 5;
        engine.stage_event(expiring).unwrap();
        engine.advance_time(5).unwrap();
        assert_eq!(
            engine.poll_event(),
            Some(RenderEventPoll::Terminal(RenderEventResult {
                sequence: 2,
                event_id: 102,
                outcome: RenderEventOutcome::DeadlineExceeded,
            }))
        );
        assert_eq!(engine.advance_time(4), Err(EngineError::ClockRegression));
    }

    #[test]
    fn render_event_completion_window_only_acks_contiguous_terminal_sequences() {
        let mut engine = engine(0);
        engine.stage_event(event(engine.id(), 1, 0, 0)).unwrap();
        engine.stage_event(event(engine.id(), 2, 0, 0)).unwrap();
        assert!(matches!(
            engine.poll_event(),
            Some(RenderEventPoll::Dispatch(RenderEvent { sequence: 1, .. }))
        ));
        assert!(matches!(
            engine.poll_event(),
            Some(RenderEventPoll::Dispatch(RenderEvent { sequence: 2, .. }))
        ));
        engine
            .complete_event(2, RenderEventOutcome::Executed)
            .unwrap();
        assert!(engine.drain_contiguous_event_results().is_empty());
        engine
            .complete_event(1, RenderEventOutcome::Executed)
            .unwrap();
        assert_eq!(
            engine.drain_contiguous_event_results(),
            vec![
                RenderEventResult {
                    sequence: 1,
                    event_id: 101,
                    outcome: RenderEventOutcome::Executed,
                },
                RenderEventResult {
                    sequence: 2,
                    event_id: 102,
                    outcome: RenderEventOutcome::Executed,
                },
            ]
        );
    }

    #[test]
    fn render_snapshot_replaces_world_only_after_full_preflight() {
        let mut engine = engine(0);
        let old = entity(engine.id(), 1);
        engine
            .apply_render_state(transaction(
                engine.id(),
                vec![RenderOp::Spawn {
                    entity: old,
                    value: vec![1],
                }],
            ))
            .unwrap();

        let new = entity(engine.id(), 2);
        let mut objects = BTreeMap::new();
        objects.insert(new, vec![2, 3]);
        let ack = engine
            .apply_render_snapshot(RenderStateSnapshot {
                engine: engine.id(),
                channel_epoch: 1,
                commit_id: 2,
                revision: 7,
                required_control_revision: 0,
                source_simulation_revision: 7,
                objects,
            })
            .unwrap();
        assert_eq!(ack.revision, 7);
        assert!(engine.render_object(old).is_none());
        assert_eq!(engine.render_object(new), Some([2, 3].as_slice()));

        let foreign = entity(handle(9), 3);
        let mut malformed = BTreeMap::new();
        malformed.insert(foreign, vec![9]);
        assert_eq!(
            engine.apply_render_snapshot(RenderStateSnapshot {
                engine: engine.id(),
                channel_epoch: 1,
                commit_id: 3,
                revision: 8,
                required_control_revision: 0,
                source_simulation_revision: 8,
                objects: malformed,
            }),
            Err(EngineError::InvalidEntity)
        );
        assert_eq!(engine.render_revision(), 7);
        assert_eq!(engine.render_object(new), Some([2, 3].as_slice()));
    }
}
