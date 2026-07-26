use std::collections::BTreeMap;

use voplay_protocol::{EngineId, Handle};

use crate::{RenderCommitAck, RenderEntity, RenderOp, RenderStateSnapshot, RenderStateTransaction};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PresentationDomainId {
    pub engine: EngineId,
    pub handle: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTransient {
    pub domain: PresentationDomainId,
    pub frame_id: u64,
    pub required_render_revision: u64,
    pub required_control_revision: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOutboxConfig {
    pub max_pending_ops: usize,
    pub max_pending_bytes: usize,
    pub max_snapshot_objects: usize,
    pub max_snapshot_bytes: usize,
    pub max_domains: usize,
    pub max_transient_bytes: usize,
}

impl Default for RenderOutboxConfig {
    fn default() -> Self {
        Self {
            max_pending_ops: 131_072,
            max_pending_bytes: 4 * 1024 * 1024,
            max_snapshot_objects: 1_000_000,
            max_snapshot_bytes: 64 * 1024 * 1024,
            max_domains: 64,
            max_transient_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxError {
    Closed,
    InvalidConfig,
    WrongEngine,
    InvalidEntity,
    EntityAlreadyExists,
    EntityNotFound,
    CommitIdExhausted,
    RevisionExhausted,
    InflightMismatch,
    AckRevisionMismatch,
    ChannelEpochExhausted,
    InvalidDomain,
    DomainCapacity,
    TransientCapacity,
    FrameSequence,
    SnapshotNotRequired,
    SnapshotCapacity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurablePoll {
    Idle,
    Inflight,
    SnapshotRequired,
}

#[derive(Clone, Debug)]
struct PendingValue {
    base: Option<Vec<u8>>,
    final_value: Option<Vec<u8>>,
}

pub struct RenderOutbox {
    engine: EngineId,
    config: RenderOutboxConfig,
    channel_epoch: u64,
    last_acked_revision: u64,
    next_commit_id: u64,
    acked_world: BTreeMap<RenderEntity, Vec<u8>>,
    inflight: Option<RenderStateTransaction>,
    snapshot_inflight: Option<RenderStateSnapshot>,
    pending: BTreeMap<RenderEntity, PendingValue>,
    pending_source_simulation_revision: u64,
    pending_required_control_revision: u64,
    snapshot_required: bool,
    transient: BTreeMap<PresentationDomainId, RenderTransient>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOutboxOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub channel_epoch: u64,
    pub last_acked_revision: u64,
    pub acked_objects: usize,
    pub pending_ops: usize,
    pub pending_bytes: usize,
    pub transaction_inflight: bool,
    pub snapshot_inflight: bool,
    pub snapshot_required: bool,
    pub transient_domains: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOutboxShutdownReport {
    pub released_acked_objects: usize,
    pub released_pending_ops: usize,
    pub released_pending_bytes: usize,
    pub released_transaction_inflight: bool,
    pub released_snapshot_inflight: bool,
    pub released_transient_domains: usize,
    pub released_transient_bytes: usize,
}

impl RenderOutbox {
    pub fn new(engine: EngineId, config: RenderOutboxConfig) -> Result<Self, OutboxError> {
        if !engine.is_valid()
            || config.max_pending_ops == 0
            || config.max_pending_bytes == 0
            || config.max_snapshot_objects == 0
            || config.max_snapshot_bytes == 0
            || config.max_domains == 0
            || config.max_transient_bytes == 0
        {
            return Err(OutboxError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            channel_epoch: 1,
            last_acked_revision: 0,
            next_commit_id: 1,
            acked_world: BTreeMap::new(),
            inflight: None,
            snapshot_inflight: None,
            pending: BTreeMap::new(),
            pending_source_simulation_revision: 0,
            pending_required_control_revision: 0,
            snapshot_required: false,
            transient: BTreeMap::new(),
            closed: false,
        })
    }

    pub const fn channel_epoch(&self) -> u64 {
        self.channel_epoch
    }

    pub const fn last_acked_revision(&self) -> u64 {
        self.last_acked_revision
    }

    pub fn owner_snapshot(&self) -> RenderOutboxOwnerSnapshot {
        RenderOutboxOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            channel_epoch: self.channel_epoch,
            last_acked_revision: self.last_acked_revision,
            acked_objects: self.acked_world.len(),
            pending_ops: self.pending.len(),
            pending_bytes: pending_bytes(&self.pending),
            transaction_inflight: self.inflight.is_some(),
            snapshot_inflight: self.snapshot_inflight.is_some(),
            snapshot_required: self.snapshot_required,
            transient_domains: self.transient.len(),
        }
    }

    pub fn shutdown(&mut self) -> RenderOutboxShutdownReport {
        if self.closed {
            return RenderOutboxShutdownReport {
                released_acked_objects: 0,
                released_pending_ops: 0,
                released_pending_bytes: 0,
                released_transaction_inflight: false,
                released_snapshot_inflight: false,
                released_transient_domains: 0,
                released_transient_bytes: 0,
            };
        }
        let report = RenderOutboxShutdownReport {
            released_acked_objects: self.acked_world.len(),
            released_pending_ops: self.pending.len(),
            released_pending_bytes: pending_bytes(&self.pending),
            released_transaction_inflight: self.inflight.is_some(),
            released_snapshot_inflight: self.snapshot_inflight.is_some(),
            released_transient_domains: self.transient.len(),
            released_transient_bytes: self
                .transient
                .values()
                .map(|transient| transient.bytes.len())
                .fold(0, usize::saturating_add),
        };
        self.closed = true;
        self.acked_world.clear();
        self.inflight = None;
        self.snapshot_inflight = None;
        self.pending.clear();
        self.pending_source_simulation_revision = 0;
        self.pending_required_control_revision = 0;
        self.snapshot_required = false;
        self.transient.clear();
        report
    }

    pub fn enqueue(
        &mut self,
        source_simulation_revision: u64,
        required_control_revision: u64,
        ops: Vec<RenderOp>,
    ) -> Result<(), OutboxError> {
        self.ensure_open()?;
        if self.snapshot_required {
            return Ok(());
        }
        let mut staged_pending = self.pending.clone();
        for op in ops {
            let entity = op.entity();
            if entity.engine != self.engine {
                return Err(OutboxError::WrongEngine);
            }
            if !entity.entity.is_valid() {
                return Err(OutboxError::InvalidEntity);
            }
            let current = self.projected_value_from(&staged_pending, entity);
            let next = match op {
                RenderOp::Spawn { value, .. } => {
                    if current.is_some() {
                        return Err(OutboxError::EntityAlreadyExists);
                    }
                    Some(value)
                }
                RenderOp::Upsert { value, .. } => {
                    if current.is_none() {
                        return Err(OutboxError::EntityNotFound);
                    }
                    Some(value)
                }
                RenderOp::Despawn { .. } => {
                    if current.is_none() {
                        return Err(OutboxError::EntityNotFound);
                    }
                    None
                }
            };
            let base = staged_pending
                .get(&entity)
                .map(|pending| pending.base.clone())
                .unwrap_or(current);
            if next == base {
                staged_pending.remove(&entity);
            } else {
                staged_pending.insert(
                    entity,
                    PendingValue {
                        base,
                        final_value: next,
                    },
                );
            }
        }
        if staged_pending.len() > self.config.max_pending_ops
            || pending_bytes(&staged_pending) > self.config.max_pending_bytes
        {
            self.pending.clear();
            self.snapshot_required = true;
        } else {
            self.pending = staged_pending;
            self.pending_source_simulation_revision = self
                .pending_source_simulation_revision
                .max(source_simulation_revision);
            self.pending_required_control_revision = self
                .pending_required_control_revision
                .max(required_control_revision);
        }
        Ok(())
    }

    pub fn poll_durable(
        &mut self,
    ) -> Result<Result<RenderStateTransaction, DurablePoll>, OutboxError> {
        self.ensure_open()?;
        if self.inflight.is_some() || self.snapshot_inflight.is_some() {
            return Ok(Err(DurablePoll::Inflight));
        }
        if self.snapshot_required {
            return Ok(Err(DurablePoll::SnapshotRequired));
        }
        if self.pending.is_empty() {
            return Ok(Err(DurablePoll::Idle));
        }
        let commit_id = self.next_commit_id;
        let next_commit_id = self
            .next_commit_id
            .checked_add(1)
            .ok_or(OutboxError::CommitIdExhausted)?;
        let new_revision = self
            .last_acked_revision
            .checked_add(1)
            .ok_or(OutboxError::RevisionExhausted)?;
        self.next_commit_id = next_commit_id;
        let ops = std::mem::take(&mut self.pending)
            .into_iter()
            .map(
                |(entity, pending)| match (pending.base, pending.final_value) {
                    (None, Some(value)) => RenderOp::Spawn { entity, value },
                    (Some(_), Some(value)) => RenderOp::Upsert { entity, value },
                    (Some(_), None) => RenderOp::Despawn { entity },
                    (None, None) => unreachable!("no-op projections are removed during enqueue"),
                },
            )
            .collect();
        let transaction = RenderStateTransaction {
            engine: self.engine,
            channel_epoch: self.channel_epoch,
            commit_id,
            base_revision: self.last_acked_revision,
            new_revision,
            required_control_revision: self.pending_required_control_revision,
            source_simulation_revision: self.pending_source_simulation_revision,
            ops,
        };
        self.pending_required_control_revision = 0;
        self.pending_source_simulation_revision = 0;
        self.inflight = Some(transaction.clone());
        Ok(Ok(transaction))
    }

    pub fn acknowledge(&mut self, ack: RenderCommitAck) -> Result<(), OutboxError> {
        self.ensure_open()?;
        let transaction = self.inflight.take().ok_or(OutboxError::InflightMismatch)?;
        if ack.commit_id != transaction.commit_id {
            self.inflight = Some(transaction);
            return Err(OutboxError::InflightMismatch);
        }
        if ack.revision != transaction.new_revision {
            self.inflight = Some(transaction);
            return Err(OutboxError::AckRevisionMismatch);
        }
        for op in transaction.ops {
            match op {
                RenderOp::Spawn { entity, value } | RenderOp::Upsert { entity, value } => {
                    self.acked_world.insert(entity, value);
                }
                RenderOp::Despawn { entity } => {
                    self.acked_world.remove(&entity);
                }
            }
        }
        self.last_acked_revision = ack.revision;
        Ok(())
    }

    pub fn restart_renderer(&mut self) -> Result<(), OutboxError> {
        self.ensure_open()?;
        let next_epoch = self
            .channel_epoch
            .checked_add(1)
            .ok_or(OutboxError::ChannelEpochExhausted)?;
        self.channel_epoch = next_epoch;
        self.last_acked_revision = 0;
        self.acked_world.clear();
        self.inflight = None;
        self.snapshot_inflight = None;
        self.pending.clear();
        self.snapshot_required = true;
        self.transient.clear();
        Ok(())
    }

    pub fn begin_renderer_recovery(
        &mut self,
        source_simulation_revision: u64,
        required_control_revision: u64,
    ) -> Result<RenderStateSnapshot, OutboxError> {
        self.ensure_open()?;
        let projection = self.compact_projection();
        if projection.len() > self.config.max_snapshot_objects
            || pending_value_bytes(&projection) > self.config.max_snapshot_bytes
        {
            return Err(OutboxError::SnapshotCapacity);
        }
        let revision = if let Some(snapshot) = &self.snapshot_inflight {
            snapshot.revision
        } else if self.inflight.is_some() || !self.pending.is_empty() {
            self.last_acked_revision
                .checked_add(1)
                .ok_or(OutboxError::RevisionExhausted)?
        } else {
            self.last_acked_revision
        };
        self.restart_renderer()?;
        Ok(self
            .prepare_snapshot(
                projection,
                revision,
                source_simulation_revision,
                required_control_revision,
            )?
            .clone())
    }

    pub fn prepare_snapshot(
        &mut self,
        world: BTreeMap<RenderEntity, Vec<u8>>,
        revision: u64,
        source_simulation_revision: u64,
        required_control_revision: u64,
    ) -> Result<&RenderStateSnapshot, OutboxError> {
        self.ensure_open()?;
        if !self.snapshot_required {
            return Err(OutboxError::SnapshotNotRequired);
        }
        if self.inflight.is_some() || self.snapshot_inflight.is_some() {
            return Err(OutboxError::InflightMismatch);
        }
        if world
            .keys()
            .any(|entity| entity.engine != self.engine || !entity.entity.is_valid())
        {
            return Err(OutboxError::InvalidEntity);
        }
        let bytes = world
            .values()
            .try_fold(0_usize, |total, value| total.checked_add(value.len()));
        if world.len() > self.config.max_snapshot_objects
            || bytes
                .map(|bytes| bytes > self.config.max_snapshot_bytes)
                .unwrap_or(true)
        {
            return Err(OutboxError::SnapshotCapacity);
        }
        let commit_id = self.next_commit_id;
        self.next_commit_id = self
            .next_commit_id
            .checked_add(1)
            .ok_or(OutboxError::CommitIdExhausted)?;
        self.snapshot_inflight = Some(RenderStateSnapshot {
            engine: self.engine,
            channel_epoch: self.channel_epoch,
            commit_id,
            revision,
            required_control_revision,
            source_simulation_revision,
            objects: world,
        });
        Ok(self.snapshot_inflight.as_ref().unwrap())
    }

    pub fn acknowledge_snapshot(&mut self, ack: RenderCommitAck) -> Result<(), OutboxError> {
        self.ensure_open()?;
        let snapshot = self
            .snapshot_inflight
            .take()
            .ok_or(OutboxError::InflightMismatch)?;
        if ack.commit_id != snapshot.commit_id {
            self.snapshot_inflight = Some(snapshot);
            return Err(OutboxError::InflightMismatch);
        }
        if ack.revision != snapshot.revision {
            self.snapshot_inflight = Some(snapshot);
            return Err(OutboxError::AckRevisionMismatch);
        }
        self.acked_world = snapshot.objects;
        self.last_acked_revision = snapshot.revision;
        self.snapshot_required = false;
        Ok(())
    }

    pub fn stage_transient(&mut self, transient: RenderTransient) -> Result<(), OutboxError> {
        self.stage_transients(vec![transient])
    }

    pub fn stage_transients(
        &mut self,
        transients: Vec<RenderTransient>,
    ) -> Result<(), OutboxError> {
        self.ensure_open()?;
        let mut staged = self.transient.clone();
        for transient in transients {
            Self::stage_transient_into(self.engine, self.config, &mut staged, transient)?;
        }
        self.transient = staged;
        Ok(())
    }

    fn stage_transient_into(
        engine: EngineId,
        config: RenderOutboxConfig,
        staged: &mut BTreeMap<PresentationDomainId, RenderTransient>,
        transient: RenderTransient,
    ) -> Result<(), OutboxError> {
        if transient.domain.engine != engine || !transient.domain.handle.is_valid() {
            return Err(OutboxError::InvalidDomain);
        }
        if transient.bytes.len() > config.max_transient_bytes {
            return Err(OutboxError::TransientCapacity);
        }
        if let Some(current) = staged.get(&transient.domain) {
            if transient.frame_id <= current.frame_id {
                return Err(OutboxError::FrameSequence);
            }
        } else if staged.len() == config.max_domains {
            return Err(OutboxError::DomainCapacity);
        }
        staged.insert(transient.domain, transient);
        Ok(())
    }

    pub fn take_ready_transient(
        &mut self,
        domain: PresentationDomainId,
        render_revision: u64,
        control_revision: u64,
    ) -> Option<RenderTransient> {
        if self.closed || domain.engine != self.engine {
            return None;
        }
        let transient = self.transient.get(&domain)?;
        if transient.required_render_revision > render_revision
            || transient.required_control_revision > control_revision
        {
            return None;
        }
        self.transient.remove(&domain)
    }

    pub fn take_ready_transient_for_frame(
        &mut self,
        domain: PresentationDomainId,
        frame_id: u64,
        render_revision: u64,
        control_revision: u64,
    ) -> Option<RenderTransient> {
        if self.closed || domain.engine != self.engine {
            return None;
        }
        let transient = self.transient.get(&domain)?;
        if transient.required_render_revision > render_revision
            || transient.required_control_revision > control_revision
            || transient.frame_id > frame_id
        {
            return None;
        }
        let transient = self.transient.remove(&domain)?;
        (transient.frame_id == frame_id).then_some(transient)
    }

    pub fn take_transient(&mut self, domain: PresentationDomainId) -> Option<RenderTransient> {
        if self.closed || domain.engine != self.engine {
            return None;
        }
        self.transient.remove(&domain)
    }

    pub const fn has_snapshot_inflight(&self) -> bool {
        self.snapshot_inflight.is_some()
    }

    fn projected_value_from(
        &self,
        pending_values: &BTreeMap<RenderEntity, PendingValue>,
        entity: RenderEntity,
    ) -> Option<Vec<u8>> {
        if let Some(pending) = pending_values.get(&entity) {
            return pending.final_value.clone();
        }
        if let Some(transaction) = &self.inflight {
            if let Some(op) = transaction.ops.iter().find(|op| op.entity() == entity) {
                return match op {
                    RenderOp::Spawn { value, .. } | RenderOp::Upsert { value, .. } => {
                        Some(value.clone())
                    }
                    RenderOp::Despawn { .. } => None,
                };
            }
        }
        self.acked_world.get(&entity).cloned()
    }

    fn compact_projection(&self) -> BTreeMap<RenderEntity, Vec<u8>> {
        let mut projection = self
            .snapshot_inflight
            .as_ref()
            .map(|snapshot| snapshot.objects.clone())
            .unwrap_or_else(|| self.acked_world.clone());
        if let Some(transaction) = &self.inflight {
            apply_projection_ops(&mut projection, &transaction.ops);
        }
        for (entity, pending) in &self.pending {
            match &pending.final_value {
                Some(value) => {
                    projection.insert(*entity, value.clone());
                }
                None => {
                    projection.remove(entity);
                }
            }
        }
        projection
    }

    fn ensure_open(&self) -> Result<(), OutboxError> {
        if self.closed {
            Err(OutboxError::Closed)
        } else {
            Ok(())
        }
    }
}

fn pending_bytes(pending: &BTreeMap<RenderEntity, PendingValue>) -> usize {
    pending
        .values()
        .map(|pending| pending.final_value.as_ref().map_or(0, Vec::len))
        .fold(0_usize, usize::saturating_add)
}

fn pending_value_bytes(values: &BTreeMap<RenderEntity, Vec<u8>>) -> usize {
    values
        .values()
        .map(Vec::len)
        .fold(0usize, usize::saturating_add)
}

fn apply_projection_ops(projection: &mut BTreeMap<RenderEntity, Vec<u8>>, ops: &[RenderOp]) {
    for op in ops {
        match op {
            RenderOp::Spawn { entity, value } | RenderOp::Upsert { entity, value } => {
                projection.insert(*entity, value.clone());
            }
            RenderOp::Despawn { entity } => {
                projection.remove(entity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, EngineConfig};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn entity(engine: EngineId, index: u32) -> RenderEntity {
        RenderEntity {
            engine,
            entity: handle(index),
        }
    }

    fn outbox() -> RenderOutbox {
        RenderOutbox::new(
            handle(1),
            RenderOutboxConfig {
                max_pending_ops: 2,
                max_pending_bytes: 8,
                max_snapshot_objects: 8,
                max_snapshot_bytes: 32,
                max_domains: 2,
                max_transient_bytes: 4,
            },
        )
        .unwrap()
    }

    #[test]
    fn stable_projection_emits_zero_packet_and_compacts_actual_changes() {
        let mut outbox = outbox();
        let entity = entity(handle(1), 7);
        outbox
            .enqueue(
                1,
                0,
                vec![RenderOp::Spawn {
                    entity,
                    value: vec![1],
                }],
            )
            .unwrap();
        outbox
            .enqueue(
                2,
                0,
                vec![RenderOp::Upsert {
                    entity,
                    value: vec![2],
                }],
            )
            .unwrap();
        let transaction = outbox.poll_durable().unwrap().unwrap();
        assert_eq!(transaction.ops.len(), 1);
        assert_eq!(
            transaction.ops[0],
            RenderOp::Spawn {
                entity,
                value: vec![2]
            }
        );
        assert_eq!(transaction.source_simulation_revision, 2);
        assert_eq!(outbox.poll_durable(), Ok(Err(DurablePoll::Inflight)));
        outbox
            .acknowledge(RenderCommitAck {
                commit_id: transaction.commit_id,
                revision: transaction.new_revision,
            })
            .unwrap();
        outbox
            .enqueue(
                3,
                0,
                vec![RenderOp::Upsert {
                    entity,
                    value: vec![2],
                }],
            )
            .unwrap();
        assert_eq!(outbox.poll_durable(), Ok(Err(DurablePoll::Idle)));
    }

    #[test]
    fn budget_overflow_requests_snapshot_without_partial_transaction() {
        let mut outbox = outbox();
        outbox
            .enqueue(
                1,
                0,
                vec![
                    RenderOp::Spawn {
                        entity: entity(handle(1), 1),
                        value: vec![1],
                    },
                    RenderOp::Spawn {
                        entity: entity(handle(1), 2),
                        value: vec![2],
                    },
                    RenderOp::Spawn {
                        entity: entity(handle(1), 3),
                        value: vec![3],
                    },
                ],
            )
            .unwrap();
        assert_eq!(
            outbox.poll_durable(),
            Ok(Err(DurablePoll::SnapshotRequired))
        );
    }

    #[test]
    fn domains_keep_independent_latest_only_transient_slots() {
        let mut outbox = outbox();
        let first = PresentationDomainId {
            engine: handle(1),
            handle: handle(10),
        };
        let second = PresentationDomainId {
            engine: handle(1),
            handle: handle(11),
        };
        for frame_id in 1..=2 {
            outbox
                .stage_transient(RenderTransient {
                    domain: first,
                    frame_id,
                    required_render_revision: 0,
                    required_control_revision: 0,
                    bytes: vec![frame_id as u8],
                })
                .unwrap();
        }
        outbox
            .stage_transient(RenderTransient {
                domain: second,
                frame_id: 1,
                required_render_revision: 0,
                required_control_revision: 0,
                bytes: vec![9],
            })
            .unwrap();
        assert_eq!(
            outbox.take_ready_transient(first, 0, 0).unwrap().frame_id,
            2
        );
        assert_eq!(
            outbox.take_ready_transient(second, 0, 0).unwrap().bytes,
            vec![9]
        );
    }

    #[test]
    fn invalid_batch_does_not_leave_earlier_ops_in_pending_accumulator() {
        let mut outbox = outbox();
        assert_eq!(
            outbox.enqueue(
                1,
                0,
                vec![
                    RenderOp::Spawn {
                        entity: entity(handle(1), 1),
                        value: vec![1],
                    },
                    RenderOp::Upsert {
                        entity: entity(handle(1), 2),
                        value: vec![2],
                    },
                ],
            ),
            Err(OutboxError::EntityNotFound)
        );
        assert_eq!(outbox.poll_durable(), Ok(Err(DurablePoll::Idle)));
    }

    #[test]
    fn transient_waits_for_both_barriers_without_blocking_another_domain() {
        let mut outbox = outbox();
        let waiting = PresentationDomainId {
            engine: handle(1),
            handle: handle(10),
        };
        let ready = PresentationDomainId {
            engine: handle(1),
            handle: handle(11),
        };
        outbox
            .stage_transient(RenderTransient {
                domain: waiting,
                frame_id: 1,
                required_render_revision: 2,
                required_control_revision: 3,
                bytes: vec![1],
            })
            .unwrap();
        outbox
            .stage_transient(RenderTransient {
                domain: ready,
                frame_id: 1,
                required_render_revision: 0,
                required_control_revision: 0,
                bytes: vec![2],
            })
            .unwrap();
        assert!(outbox.take_ready_transient(waiting, 2, 2).is_none());
        assert_eq!(
            outbox.take_ready_transient(ready, 0, 0).unwrap().bytes,
            vec![2]
        );
        assert_eq!(
            outbox.take_ready_transient(waiting, 2, 3).unwrap().bytes,
            vec![1]
        );
    }

    #[test]
    fn outbox_transaction_and_renderer_ack_share_revision_contract() {
        let mut outbox = outbox();
        let mut renderer = Engine::new(
            handle(1),
            EngineConfig {
                max_render_objects: 4,
                max_transaction_ops: 4,
                max_transaction_bytes: 16,
                max_snapshot_objects: 8,
                max_snapshot_bytes: 32,
                max_staged_events: 4,
                max_staged_event_bytes: 16,
            },
        )
        .unwrap();
        renderer.set_render_control_revision(4);
        let entity = entity(handle(1), 1);
        outbox
            .enqueue(
                8,
                4,
                vec![RenderOp::Spawn {
                    entity,
                    value: vec![9],
                }],
            )
            .unwrap();
        let transaction = outbox.poll_durable().unwrap().unwrap();
        let ack = renderer.apply_render_state(transaction).unwrap();
        outbox.acknowledge(ack).unwrap();
        assert_eq!(renderer.render_revision(), 1);
        assert_eq!(outbox.last_acked_revision(), 1);
        assert_eq!(renderer.render_object(entity), Some([9].as_slice()));
    }

    #[test]
    fn renderer_restart_recovers_through_transmitted_snapshot_and_exact_ack() {
        let mut outbox = outbox();
        let mut renderer = Engine::new(
            handle(1),
            EngineConfig {
                max_render_objects: 8,
                max_transaction_ops: 4,
                max_transaction_bytes: 16,
                max_snapshot_objects: 8,
                max_snapshot_bytes: 32,
                max_staged_events: 4,
                max_staged_event_bytes: 16,
            },
        )
        .unwrap();
        let entity = entity(handle(1), 1);
        outbox
            .enqueue(
                1,
                0,
                vec![RenderOp::Spawn {
                    entity,
                    value: vec![1],
                }],
            )
            .unwrap();
        let transaction = outbox.poll_durable().unwrap().unwrap();
        let ack = renderer.apply_render_state(transaction).unwrap();
        outbox.acknowledge(ack).unwrap();

        renderer.restart_renderer().unwrap();
        outbox.restart_renderer().unwrap();
        let mut world = BTreeMap::new();
        world.insert(entity, vec![7]);
        let snapshot = outbox.prepare_snapshot(world, 8, 8, 0).unwrap().clone();
        assert_eq!(outbox.poll_durable(), Ok(Err(DurablePoll::Inflight)));
        let ack = renderer.apply_render_snapshot(snapshot).unwrap();
        outbox.acknowledge_snapshot(ack).unwrap();
        assert_eq!(renderer.render_revision(), 8);
        assert_eq!(outbox.last_acked_revision(), 8);
        assert_eq!(renderer.render_object(entity), Some([7].as_slice()));

        outbox
            .enqueue(
                9,
                0,
                vec![RenderOp::Upsert {
                    entity,
                    value: vec![9],
                }],
            )
            .unwrap();
        let transaction = outbox.poll_durable().unwrap().unwrap();
        assert_eq!(transaction.base_revision, 8);
        let ack = renderer.apply_render_state(transaction).unwrap();
        outbox.acknowledge(ack).unwrap();
        assert_eq!(renderer.render_object(entity), Some([9].as_slice()));
    }

    #[test]
    fn rejected_snapshot_ack_preserves_inflight_and_recovery_requirement() {
        let mut outbox = outbox();
        outbox.restart_renderer().unwrap();
        let snapshot = outbox
            .prepare_snapshot(BTreeMap::new(), 3, 3, 0)
            .unwrap()
            .clone();
        assert_eq!(
            outbox.acknowledge_snapshot(RenderCommitAck {
                commit_id: snapshot.commit_id + 1,
                revision: snapshot.revision,
            }),
            Err(OutboxError::InflightMismatch)
        );
        assert_eq!(outbox.poll_durable(), Ok(Err(DurablePoll::Inflight)));
        outbox
            .acknowledge_snapshot(RenderCommitAck {
                commit_id: snapshot.commit_id,
                revision: snapshot.revision,
            })
            .unwrap();
        assert_eq!(outbox.last_acked_revision(), 3);
    }
}
