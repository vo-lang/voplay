use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlDomain {
    Render,
    Audio,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ControlKind {
    RenderView,
    RenderTarget,
    RenderGraph,
    AudioBus,
    PersistentAudioSource,
}

impl ControlKind {
    pub const fn domain(self) -> ControlDomain {
        match self {
            Self::RenderView | Self::RenderTarget | Self::RenderGraph => ControlDomain::Render,
            Self::AudioBus | Self::PersistentAudioSource => ControlDomain::Audio,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StableControlRef {
    pub engine: EngineId,
    pub kind: ControlKind,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlRefState {
    Desired,
    Realizing,
    Live { endpoint_generation: Handle },
    Tombstone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealizationOutcome {
    Live,
    TransientFailure,
    PermanentFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetirementFence {
    pub last_render_revision: u64,
    pub last_domain_frame: u64,
    pub last_event_sequence: u64,
    pub last_audio_event_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetirementProgress {
    pub render_revision: u64,
    pub domain_frame: u64,
    pub event_sequence: u64,
    pub audio_event_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineControlConfig {
    pub max_leases: usize,
    pub max_refs: usize,
    pub max_ops_per_transaction: usize,
    pub max_descriptor_bytes: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for EngineControlConfig {
    fn default() -> Self {
        Self {
            max_leases: 64,
            max_refs: 4096,
            max_ops_per_transaction: 256,
            max_descriptor_bytes: 1024 * 1024,
            max_snapshot_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    InvalidConfig,
    InvalidProducer,
    LeaseCapacity,
    InvalidLease,
    WrongEngine,
    WrongDomain,
    TransactionCapacity,
    BaseRevisionMismatch,
    DuplicateTransaction,
    DuplicateRetirement,
    InvalidStableRef,
    RefCapacity,
    RevisionExhausted,
    WrongLogicGeneration,
    WrongEndpointGeneration,
    SnapshotCapacity,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlSnapshotState {
    Desired,
    Tombstone { fence: RetirementFence },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlSnapshotEntry {
    pub stable: StableControlRef,
    pub descriptor: Vec<u8>,
    pub state: ControlSnapshotState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlStateSnapshot {
    pub engine: EngineId,
    pub domain: ControlDomain,
    pub revision: u64,
    pub last_transaction_id: u64,
    pub entries: Vec<ControlSnapshotEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProvisionalControlRef {
    lease: Handle,
    token: u32,
    kind: ControlKind,
}

#[derive(Clone, Debug)]
enum ControlOp {
    Create {
        provisional: ProvisionalControlRef,
        descriptor: Vec<u8>,
        dependencies: Vec<DescriptorDependency>,
    },
    Retire {
        stable: StableControlRef,
        fence: RetirementFence,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ControlDependencyRef {
    Provisional { token: u32, kind: ControlKind },
    Stable(StableControlRef),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DescriptorDependency {
    pub offset: usize,
    pub reference: ControlDependencyRef,
}

pub(crate) const STABLE_BINDING_BYTES: usize = 10;

pub(crate) fn write_stable_binding(
    descriptor: &mut [u8],
    offset: usize,
    stable: StableControlRef,
) -> Result<(), ControlError> {
    let end = offset
        .checked_add(STABLE_BINDING_BYTES)
        .ok_or(ControlError::InvalidStableRef)?;
    let target = descriptor
        .get_mut(offset..end)
        .ok_or(ControlError::InvalidStableRef)?;
    target[0] = 2;
    target[1] = control_kind_tag(stable.kind);
    target[2..6].copy_from_slice(&stable.handle.index.to_le_bytes());
    target[6..10].copy_from_slice(&stable.handle.generation.to_le_bytes());
    Ok(())
}

pub(crate) const fn control_kind_tag(kind: ControlKind) -> u8 {
    match kind {
        ControlKind::RenderView => 1,
        ControlKind::RenderTarget => 2,
        ControlKind::AudioBus => 3,
        ControlKind::PersistentAudioSource => 4,
        ControlKind::RenderGraph => 5,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlRefAllocatorLease {
    engine: EngineId,
    producer: Handle,
    logic_generation: Handle,
    lease: Handle,
    capacity: u32,
}

impl ControlRefAllocatorLease {
    pub const fn engine(self) -> EngineId {
        self.engine
    }

    pub const fn producer(self) -> Handle {
        self.producer
    }

    pub const fn logic_generation(self) -> Handle {
        self.logic_generation
    }

    pub const fn lease(self) -> Handle {
        self.lease
    }

    pub const fn capacity(self) -> u32 {
        self.capacity
    }

    pub(crate) const fn from_wire_parts(
        engine: EngineId,
        producer: Handle,
        logic_generation: Handle,
        lease: Handle,
        capacity: u32,
    ) -> Self {
        Self {
            engine,
            producer,
            logic_generation,
            lease,
            capacity,
        }
    }

    pub fn begin(
        self,
        domain: ControlDomain,
        transaction_id: u64,
        base_revision: u64,
    ) -> ControlTxnBuilder {
        ControlTxnBuilder {
            lease: self,
            domain,
            transaction_id,
            base_revision,
            next_token: 0,
            ops: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ControlTxnIdentity {
    pub(crate) engine: EngineId,
    pub(crate) lease: Handle,
    pub(crate) domain: ControlDomain,
    pub(crate) transaction_id: u64,
}

#[derive(Clone)]
pub struct ControlTxnBuilder {
    lease: ControlRefAllocatorLease,
    domain: ControlDomain,
    transaction_id: u64,
    base_revision: u64,
    next_token: u32,
    ops: Vec<ControlOp>,
}

impl ControlTxnBuilder {
    pub const fn domain(&self) -> ControlDomain {
        self.domain
    }

    pub(crate) const fn identity(&self) -> ControlTxnIdentity {
        ControlTxnIdentity {
            engine: self.lease.engine,
            lease: self.lease.lease,
            domain: self.domain,
            transaction_id: self.transaction_id,
        }
    }

    pub fn create(&mut self, kind: ControlKind, descriptor: Vec<u8>) -> Result<u32, ControlError> {
        self.create_with_dependencies(kind, descriptor, Vec::new())
    }

    pub(crate) fn create_with_dependencies(
        &mut self,
        kind: ControlKind,
        descriptor: Vec<u8>,
        dependencies: Vec<DescriptorDependency>,
    ) -> Result<u32, ControlError> {
        if kind.domain() != self.domain {
            return Err(ControlError::WrongDomain);
        }
        if self.next_token == self.lease.capacity {
            return Err(ControlError::LeaseCapacity);
        }
        let token = self.next_token;
        self.next_token += 1;
        self.ops.push(ControlOp::Create {
            provisional: ProvisionalControlRef {
                lease: self.lease.lease,
                token,
                kind,
            },
            descriptor,
            dependencies,
        });
        Ok(token)
    }

    pub fn retire(
        &mut self,
        stable: StableControlRef,
        fence: RetirementFence,
    ) -> Result<(), ControlError> {
        if stable.engine != self.lease.engine {
            return Err(ControlError::WrongEngine);
        }
        if stable.kind.domain() != self.domain {
            return Err(ControlError::WrongDomain);
        }
        self.ops.push(ControlOp::Retire { stable, fence });
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Promotion {
    token: u32,
    stable: StableControlRef,
}

#[derive(Debug)]
pub struct ControlCommitAck {
    pub transaction_id: u64,
    pub control_revision: u64,
    logic_generation: Handle,
    promotions: Vec<Promotion>,
}

impl ControlCommitAck {
    pub fn publish_at_safe_point(
        self,
        logic_generation: Handle,
    ) -> Result<ControlCommitted, (ControlError, Self)> {
        if logic_generation != self.logic_generation {
            return Err((ControlError::WrongLogicGeneration, self));
        }
        Ok(ControlCommitted {
            transaction_id: self.transaction_id,
            control_revision: self.control_revision,
            promotions: self
                .promotions
                .into_iter()
                .map(|promotion| (promotion.token, promotion.stable))
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlCommitted {
    pub transaction_id: u64,
    pub control_revision: u64,
    pub promotions: Vec<(u32, StableControlRef)>,
}

#[derive(Clone, Debug)]
struct RefSlot {
    generation: u32,
    kind: ControlKind,
    descriptor: Vec<u8>,
    state: ControlRefState,
    retirement_fence: Option<RetirementFence>,
}

#[derive(Clone, Copy, Debug)]
struct LeaseRecord {
    producer: Handle,
    logic_generation: Handle,
    capacity: u32,
}

#[derive(Clone)]
pub struct EngineControlStore {
    engine: EngineId,
    config: EngineControlConfig,
    next_lease: u32,
    leases: BTreeMap<Handle, LeaseRecord>,
    render_refs: Vec<Option<RefSlot>>,
    audio_refs: Vec<Option<RefSlot>>,
    render_generations: Vec<u32>,
    audio_generations: Vec<u32>,
    render_revision: u64,
    audio_revision: u64,
    last_render_transaction: u64,
    last_audio_transaction: u64,
    render_endpoint_generation: Option<Handle>,
    audio_endpoint_generation: Option<Handle>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineControlOwnerSnapshot {
    pub engine: EngineId,
    pub leases: usize,
    pub render_refs: usize,
    pub audio_refs: usize,
    pub render_revision: u64,
    pub audio_revision: u64,
    pub render_endpoint_bound: bool,
    pub audio_endpoint_bound: bool,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineControlShutdownReport {
    pub before: EngineControlOwnerSnapshot,
    pub after: EngineControlOwnerSnapshot,
}

impl EngineControlStore {
    pub fn new(engine: EngineId, config: EngineControlConfig) -> Result<Self, ControlError> {
        if !engine.is_valid()
            || config.max_leases == 0
            || config.max_refs == 0
            || config.max_refs > u32::MAX as usize
            || config.max_ops_per_transaction == 0
            || config.max_descriptor_bytes == 0
            || config.max_snapshot_bytes == 0
        {
            return Err(ControlError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            next_lease: 0,
            leases: BTreeMap::new(),
            render_refs: Vec::new(),
            audio_refs: Vec::new(),
            render_generations: Vec::new(),
            audio_generations: Vec::new(),
            render_revision: 0,
            audio_revision: 0,
            last_render_transaction: 0,
            last_audio_transaction: 0,
            render_endpoint_generation: None,
            audio_endpoint_generation: None,
            closed: false,
        })
    }

    pub fn issue_lease(
        &mut self,
        producer: Handle,
        logic_generation: Handle,
        capacity: u32,
    ) -> Result<ControlRefAllocatorLease, ControlError> {
        self.ensure_open()?;
        if !producer.is_valid() || !logic_generation.is_valid() || capacity == 0 {
            return Err(ControlError::InvalidProducer);
        }
        if self.leases.len() == self.config.max_leases {
            return Err(ControlError::LeaseCapacity);
        }
        let lease = Handle {
            index: self.next_lease,
            generation: 1,
        };
        self.next_lease = self
            .next_lease
            .checked_add(1)
            .ok_or(ControlError::LeaseCapacity)?;
        self.leases.insert(
            lease,
            LeaseRecord {
                producer,
                logic_generation,
                capacity,
            },
        );
        Ok(ControlRefAllocatorLease {
            engine: self.engine,
            producer,
            logic_generation,
            lease,
            capacity,
        })
    }

    pub fn owner_snapshot(&self) -> EngineControlOwnerSnapshot {
        EngineControlOwnerSnapshot {
            engine: self.engine,
            leases: self.leases.len(),
            render_refs: self.render_refs.iter().flatten().count(),
            audio_refs: self.audio_refs.iter().flatten().count(),
            render_revision: self.render_revision,
            audio_revision: self.audio_revision,
            render_endpoint_bound: self.render_endpoint_generation.is_some(),
            audio_endpoint_bound: self.audio_endpoint_generation.is_some(),
            closed: self.closed,
        }
    }

    pub(crate) fn clear_owned_state(&mut self) {
        self.next_lease = 0;
        self.leases.clear();
        self.render_refs = Vec::new();
        self.audio_refs = Vec::new();
        self.render_generations = Vec::new();
        self.audio_generations = Vec::new();
        self.render_revision = 0;
        self.audio_revision = 0;
        self.last_render_transaction = 0;
        self.last_audio_transaction = 0;
        self.render_endpoint_generation = None;
        self.audio_endpoint_generation = None;
        self.closed = true;
    }

    pub fn shutdown(&mut self) -> EngineControlShutdownReport {
        let before = self.owner_snapshot();
        self.clear_owned_state();
        EngineControlShutdownReport {
            before,
            after: self.owner_snapshot(),
        }
    }

    pub const fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub const fn audio_revision(&self) -> u64 {
        self.audio_revision
    }

    pub const fn last_transaction(&self, domain: ControlDomain) -> u64 {
        match domain {
            ControlDomain::Render => self.last_render_transaction,
            ControlDomain::Audio => self.last_audio_transaction,
        }
    }

    pub fn commit(&mut self, builder: ControlTxnBuilder) -> Result<ControlCommitAck, ControlError> {
        self.ensure_open()?;
        if builder.lease.engine != self.engine {
            return Err(ControlError::WrongEngine);
        }
        let lease = self
            .leases
            .get(&builder.lease.lease)
            .copied()
            .ok_or(ControlError::InvalidLease)?;
        if lease.producer != builder.lease.producer
            || lease.logic_generation != builder.lease.logic_generation
            || lease.capacity != builder.lease.capacity
        {
            return Err(ControlError::InvalidLease);
        }
        let (revision, last_transaction) = match builder.domain {
            ControlDomain::Render => (self.render_revision, self.last_render_transaction),
            ControlDomain::Audio => (self.audio_revision, self.last_audio_transaction),
        };
        if builder.base_revision != revision {
            return Err(ControlError::BaseRevisionMismatch);
        }
        if builder.transaction_id <= last_transaction {
            return Err(ControlError::DuplicateTransaction);
        }
        let descriptor_bytes = builder.ops.iter().try_fold(0_usize, |total, op| {
            total.checked_add(match op {
                ControlOp::Create { descriptor, .. } => descriptor.len(),
                ControlOp::Retire { .. } => 0,
            })
        });
        if builder.ops.len() > self.config.max_ops_per_transaction
            || descriptor_bytes
                .map(|bytes| bytes > self.config.max_descriptor_bytes)
                .unwrap_or(true)
        {
            return Err(ControlError::TransactionCapacity);
        }
        let create_count = builder
            .ops
            .iter()
            .filter(|op| matches!(op, ControlOp::Create { .. }))
            .count();
        if self
            .render_refs
            .iter()
            .flatten()
            .count()
            .saturating_add(self.audio_refs.iter().flatten().count())
            .saturating_add(create_count)
            > self.config.max_refs
        {
            return Err(ControlError::RefCapacity);
        }
        let created = builder
            .ops
            .iter()
            .filter_map(|op| match op {
                ControlOp::Create { provisional, .. } => {
                    Some((provisional.token, provisional.kind))
                }
                ControlOp::Retire { .. } => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut retired = BTreeSet::new();
        for op in &builder.ops {
            match op {
                ControlOp::Create {
                    provisional,
                    descriptor,
                    dependencies,
                } => {
                    if provisional.lease != builder.lease.lease
                        || provisional.kind.domain() != builder.domain
                    {
                        return Err(ControlError::InvalidLease);
                    }
                    for dependency in dependencies {
                        dependency
                            .offset
                            .checked_add(STABLE_BINDING_BYTES)
                            .filter(|end| *end <= descriptor.len())
                            .ok_or(ControlError::InvalidStableRef)?;
                        match dependency.reference {
                            ControlDependencyRef::Stable(stable) => {
                                self.validate_stable(stable, builder.domain)?;
                            }
                            ControlDependencyRef::Provisional { token, kind } => {
                                if created.get(&token) != Some(&kind)
                                    || kind.domain() != builder.domain
                                {
                                    return Err(ControlError::InvalidStableRef);
                                }
                            }
                        }
                    }
                }
                ControlOp::Retire { stable, .. } => {
                    self.validate_stable(*stable, builder.domain)?;
                    if !retired.insert(*stable) {
                        return Err(ControlError::DuplicateRetirement);
                    }
                }
            }
        }
        let new_revision = revision
            .checked_add(1)
            .ok_or(ControlError::RevisionExhausted)?;
        let domain_ref_count = self.refs(builder.domain).iter().flatten().count();
        let mut available_indices = self
            .refs(builder.domain)
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.is_none().then_some(index))
            .collect::<Vec<_>>();
        available_indices.reverse();
        let mut next_append_index = self.refs(builder.domain).len();
        let mut promotions = Vec::with_capacity(create_count);
        for op in &builder.ops {
            if let ControlOp::Create { provisional, .. } = op {
                let index = if let Some(index) = available_indices.pop() {
                    index
                } else {
                    let index = next_append_index;
                    next_append_index = next_append_index
                        .checked_add(1)
                        .ok_or(ControlError::RefCapacity)?;
                    index
                };
                let generation = self
                    .generations(builder.domain)
                    .get(index)
                    .copied()
                    .unwrap_or(1);
                promotions.push(Promotion {
                    token: provisional.token,
                    stable: StableControlRef {
                        engine: self.engine,
                        kind: provisional.kind,
                        handle: Handle {
                            index: u32::try_from(
                                index.checked_add(1).ok_or(ControlError::RefCapacity)?,
                            )
                            .map_err(|_| ControlError::RefCapacity)?,
                            generation,
                        },
                    },
                });
            }
        }
        for op in builder.ops {
            match op {
                ControlOp::Create {
                    provisional,
                    mut descriptor,
                    dependencies,
                } => {
                    for dependency in dependencies {
                        let stable = match dependency.reference {
                            ControlDependencyRef::Stable(stable) => stable,
                            ControlDependencyRef::Provisional { token, .. } => {
                                promotions
                                    .iter()
                                    .find(|promotion| promotion.token == token)
                                    .expect("validated provisional dependency has a promotion")
                                    .stable
                            }
                        };
                        write_stable_binding(&mut descriptor, dependency.offset, stable)
                            .expect("validated descriptor dependency range remains valid");
                    }
                    let stable = promotions
                        .iter()
                        .find(|promotion| promotion.token == provisional.token)
                        .expect("every create has a promotion")
                        .stable;
                    let slot = Some(RefSlot {
                        generation: stable.handle.generation,
                        kind: provisional.kind,
                        descriptor,
                        state: ControlRefState::Desired,
                        retirement_fence: None,
                    });
                    let index = stable.handle.index as usize - 1;
                    if index == self.refs(builder.domain).len() {
                        self.refs_mut(builder.domain).push(slot);
                        self.generations_mut(builder.domain)
                            .push(stable.handle.generation);
                    } else {
                        self.refs_mut(builder.domain)[index] = slot;
                    }
                }
                ControlOp::Retire { stable, fence } => {
                    let slot = self.refs_mut(builder.domain)[stable.handle.index as usize - 1]
                        .as_mut()
                        .expect("validated stable ref remains present");
                    slot.state = ControlRefState::Tombstone;
                    slot.retirement_fence = Some(fence);
                }
            }
        }
        debug_assert_eq!(
            self.refs(builder.domain).iter().flatten().count(),
            domain_ref_count + create_count
        );
        self.leases.remove(&builder.lease.lease);
        match builder.domain {
            ControlDomain::Render => {
                self.render_revision = new_revision;
                self.last_render_transaction = builder.transaction_id;
            }
            ControlDomain::Audio => {
                self.audio_revision = new_revision;
                self.last_audio_transaction = builder.transaction_id;
            }
        }
        Ok(ControlCommitAck {
            transaction_id: builder.transaction_id,
            control_revision: new_revision,
            logic_generation: builder.lease.logic_generation,
            promotions,
        })
    }

    pub fn state(&self, stable: StableControlRef) -> Option<ControlRefState> {
        self.ref_slot(stable).map(|slot| slot.state)
    }

    pub fn descriptor(&self, stable: StableControlRef) -> Option<&[u8]> {
        self.ref_slot(stable).map(|slot| slot.descriptor.as_slice())
    }

    pub fn record_realization(
        &mut self,
        stable: StableControlRef,
        endpoint_generation: Handle,
        outcome: RealizationOutcome,
    ) -> Result<ControlRefState, ControlError> {
        self.ensure_open()?;
        let domain = stable.kind.domain();
        self.observe_endpoint_generation(domain, endpoint_generation)?;
        self.validate_stable(stable, domain)?;
        let slot = self.refs_mut(domain)[stable.handle.index as usize - 1]
            .as_mut()
            .expect("validated stable ref remains present");
        slot.state = match outcome {
            RealizationOutcome::Live => ControlRefState::Live {
                endpoint_generation,
            },
            RealizationOutcome::TransientFailure => ControlRefState::Realizing,
            RealizationOutcome::PermanentFailure => {
                slot.retirement_fence = Some(RetirementFence {
                    last_render_revision: 0,
                    last_domain_frame: 0,
                    last_event_sequence: 0,
                    last_audio_event_sequence: 0,
                });
                ControlRefState::Tombstone
            }
        };
        Ok(slot.state)
    }

    pub fn observe_endpoint_generation(
        &mut self,
        domain: ControlDomain,
        endpoint_generation: Handle,
    ) -> Result<(), ControlError> {
        self.ensure_open()?;
        if !endpoint_generation.is_valid() {
            return Err(ControlError::InvalidStableRef);
        }
        match self.endpoint_generation(domain) {
            Some(expected) if expected != endpoint_generation => {
                if endpoint_generation.index != expected.index
                    || endpoint_generation.generation <= expected.generation
                {
                    return Err(ControlError::WrongEndpointGeneration);
                }
                self.restart_endpoint(domain, expected, endpoint_generation)?;
            }
            None => self.set_endpoint_generation(domain, Some(endpoint_generation)),
            Some(_) => {}
        }
        Ok(())
    }

    pub fn begin_realization(&mut self, stable: StableControlRef) -> Result<(), ControlError> {
        self.ensure_open()?;
        let domain = stable.kind.domain();
        self.validate_stable(stable, domain)?;
        self.refs_mut(domain)[stable.handle.index as usize - 1]
            .as_mut()
            .expect("validated stable ref remains present")
            .state = ControlRefState::Realizing;
        Ok(())
    }

    pub fn restart_endpoint(
        &mut self,
        domain: ControlDomain,
        old_generation: Handle,
        replacement_generation: Handle,
    ) -> Result<(), ControlError> {
        self.ensure_open()?;
        self.preflight_endpoint_restart(domain, old_generation, replacement_generation)?;
        let retired = self
            .refs(domain)
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .filter(|slot| slot.state == ControlRefState::Tombstone)
                    .map(|_| index)
            })
            .collect::<Vec<_>>();
        if retired
            .iter()
            .any(|index| self.generations(domain)[*index].checked_add(1).is_none())
        {
            return Err(ControlError::RevisionExhausted);
        }
        self.set_endpoint_generation(domain, Some(replacement_generation));
        for index in 0..self.refs(domain).len() {
            let state = self.refs(domain)[index].as_ref().map(|slot| slot.state);
            match state {
                Some(ControlRefState::Tombstone) => {
                    let next_generation = self.generations(domain)[index] + 1;
                    self.refs_mut(domain)[index] = None;
                    self.generations_mut(domain)[index] = next_generation;
                }
                Some(ControlRefState::Live {
                    endpoint_generation,
                }) if endpoint_generation == old_generation => {
                    self.refs_mut(domain)[index]
                        .as_mut()
                        .expect("live slot remains present")
                        .state = ControlRefState::Realizing;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn preflight_endpoint_restart(
        &self,
        domain: ControlDomain,
        old_generation: Handle,
        replacement_generation: Handle,
    ) -> Result<(), ControlError> {
        self.ensure_open()?;
        if !replacement_generation.is_valid()
            || replacement_generation.index != old_generation.index
            || replacement_generation.generation <= old_generation.generation
            || self
                .endpoint_generation(domain)
                .is_some_and(|current| current != old_generation)
        {
            return Err(ControlError::WrongEndpointGeneration);
        }
        Ok(())
    }

    pub fn retirement_ready(&self, stable: StableControlRef, progress: RetirementProgress) -> bool {
        let Some(slot) = self.ref_slot(stable) else {
            return false;
        };
        let Some(fence) = slot.retirement_fence else {
            return false;
        };
        slot.state == ControlRefState::Tombstone
            && progress.render_revision >= fence.last_render_revision
            && progress.domain_frame >= fence.last_domain_frame
            && progress.event_sequence >= fence.last_event_sequence
            && progress.audio_event_sequence >= fence.last_audio_event_sequence
    }

    pub fn finalize_retirement(
        &mut self,
        stable: StableControlRef,
        progress: RetirementProgress,
    ) -> Result<(), ControlError> {
        self.ensure_open()?;
        if !self.retirement_ready(stable, progress) {
            return Err(ControlError::InvalidStableRef);
        }
        let domain = stable.kind.domain();
        let index = stable.handle.index as usize - 1;
        let generation = self.generations(domain)[index]
            .checked_add(1)
            .ok_or(ControlError::RevisionExhausted)?;
        self.refs_mut(domain)[index] = None;
        self.generations_mut(domain)[index] = generation;
        Ok(())
    }

    pub fn finalize_ready_retirements(
        &mut self,
        domain: ControlDomain,
        progress: RetirementProgress,
    ) -> Result<Vec<StableControlRef>, ControlError> {
        self.ensure_open()?;
        let ready = self
            .refs(domain)
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let slot = slot.as_ref()?;
                let stable = StableControlRef {
                    engine: self.engine,
                    kind: slot.kind,
                    handle: Handle {
                        index: index as u32 + 1,
                        generation: slot.generation,
                    },
                };
                self.retirement_ready(stable, progress).then_some(stable)
            })
            .collect::<Vec<_>>();
        if ready.iter().any(|stable| {
            self.generations(domain)[stable.handle.index as usize - 1]
                .checked_add(1)
                .is_none()
        }) {
            return Err(ControlError::RevisionExhausted);
        }
        for stable in &ready {
            self.finalize_retirement(*stable, progress)
                .expect("retirement readiness and generation were preflighted");
        }
        Ok(ready)
    }

    pub fn snapshot(&self, domain: ControlDomain) -> Result<ControlStateSnapshot, ControlError> {
        self.ensure_open()?;
        let mut bytes = 0_usize;
        let mut entries = Vec::new();
        for (index, slot) in self
            .refs(domain)
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.as_ref().map(|slot| (index, slot)))
        {
            bytes = bytes
                .checked_add(slot.descriptor.len())
                .filter(|bytes| *bytes <= self.config.max_snapshot_bytes)
                .ok_or(ControlError::SnapshotCapacity)?;
            let state = match slot.state {
                ControlRefState::Tombstone => ControlSnapshotState::Tombstone {
                    fence: slot
                        .retirement_fence
                        .expect("tombstone created by retire has a fence"),
                },
                ControlRefState::Desired
                | ControlRefState::Realizing
                | ControlRefState::Live { .. } => ControlSnapshotState::Desired,
            };
            entries.push(ControlSnapshotEntry {
                stable: StableControlRef {
                    engine: self.engine,
                    kind: slot.kind,
                    handle: Handle {
                        index: index as u32 + 1,
                        generation: slot.generation,
                    },
                },
                descriptor: slot.descriptor.clone(),
                state,
            });
        }
        let (revision, last_transaction_id) = match domain {
            ControlDomain::Render => (self.render_revision, self.last_render_transaction),
            ControlDomain::Audio => (self.audio_revision, self.last_audio_transaction),
        };
        Ok(ControlStateSnapshot {
            engine: self.engine,
            domain,
            revision,
            last_transaction_id,
            entries,
        })
    }

    pub fn adopt_snapshot(&mut self, snapshot: &ControlStateSnapshot) -> Result<(), ControlError> {
        self.ensure_open()?;
        if snapshot.engine != self.engine
            || snapshot.revision == 0
            || snapshot.last_transaction_id == 0
            || snapshot.entries.len() > self.config.max_refs
        {
            return Err(ControlError::SnapshotCapacity);
        }
        let current_revision = match snapshot.domain {
            ControlDomain::Render => self.render_revision,
            ControlDomain::Audio => self.audio_revision,
        };
        if current_revision != 0 && snapshot.revision == current_revision {
            let current = self.snapshot(snapshot.domain)?;
            if current == *snapshot {
                return Ok(());
            }
            if !is_tombstone_update(&current, snapshot) {
                return Err(ControlError::BaseRevisionMismatch);
            }
        }
        if current_revision != 0 && snapshot.revision < current_revision {
            return Err(ControlError::BaseRevisionMismatch);
        }
        let highest = snapshot
            .entries
            .iter()
            .map(|entry| entry.stable.handle.index)
            .max()
            .unwrap_or(0) as usize;
        if highest > self.config.max_refs
            || snapshot.entries.iter().any(|entry| {
                entry.stable.engine != self.engine
                    || entry.stable.kind.domain() != snapshot.domain
                    || entry.stable.handle.index == 0
                    || entry.stable.handle.generation == 0
            })
        {
            return Err(ControlError::InvalidStableRef);
        }
        let other_ref_count = match snapshot.domain {
            ControlDomain::Render => self.audio_refs.iter().flatten().count(),
            ControlDomain::Audio => self.render_refs.iter().flatten().count(),
        };
        if other_ref_count.saturating_add(snapshot.entries.len()) > self.config.max_refs {
            return Err(ControlError::RefCapacity);
        }
        let mut identities = BTreeSet::new();
        let mut descriptor_bytes = 0_usize;
        for entry in &snapshot.entries {
            descriptor_bytes = descriptor_bytes
                .checked_add(entry.descriptor.len())
                .filter(|bytes| *bytes <= self.config.max_snapshot_bytes)
                .ok_or(ControlError::SnapshotCapacity)?;
            if !identities.insert(entry.stable.handle.index) {
                return Err(ControlError::InvalidStableRef);
            }
        }
        {
            let refs = self.refs_mut(snapshot.domain);
            refs.clear();
            refs.resize_with(highest, || None);
        }
        {
            let generations = self.generations_mut(snapshot.domain);
            generations.clear();
            generations.resize(highest, 1);
        }
        for entry in &snapshot.entries {
            let retirement_fence = match &entry.state {
                ControlSnapshotState::Desired => None,
                ControlSnapshotState::Tombstone { fence } => Some(*fence),
            };
            let index = entry.stable.handle.index as usize - 1;
            self.refs_mut(snapshot.domain)[index] = Some(RefSlot {
                generation: entry.stable.handle.generation,
                kind: entry.stable.kind,
                descriptor: entry.descriptor.clone(),
                state: match &entry.state {
                    ControlSnapshotState::Desired => ControlRefState::Desired,
                    ControlSnapshotState::Tombstone { .. } => ControlRefState::Tombstone,
                },
                retirement_fence,
            });
            self.generations_mut(snapshot.domain)[index] = entry.stable.handle.generation;
        }
        match snapshot.domain {
            ControlDomain::Render => {
                self.render_revision = snapshot.revision;
                self.last_render_transaction = snapshot.last_transaction_id;
            }
            ControlDomain::Audio => {
                self.audio_revision = snapshot.revision;
                self.last_audio_transaction = snapshot.last_transaction_id;
            }
        }
        self.leases.clear();
        self.set_endpoint_generation(snapshot.domain, None);
        Ok(())
    }

    fn validate_stable(
        &self,
        stable: StableControlRef,
        domain: ControlDomain,
    ) -> Result<(), ControlError> {
        if stable.engine != self.engine || stable.kind.domain() != domain {
            return Err(ControlError::InvalidStableRef);
        }
        self.ref_slot(stable)
            .filter(|slot| slot.state != ControlRefState::Tombstone)
            .ok_or(ControlError::InvalidStableRef)?;
        Ok(())
    }

    fn ref_slot(&self, stable: StableControlRef) -> Option<&RefSlot> {
        if stable.engine != self.engine {
            return None;
        }
        let slot = self
            .refs(stable.kind.domain())
            .get(stable.handle.index.checked_sub(1)? as usize)?
            .as_ref()?;
        if stable.handle.generation != slot.generation || stable.kind != slot.kind {
            return None;
        }
        Some(slot)
    }

    fn refs(&self, domain: ControlDomain) -> &Vec<Option<RefSlot>> {
        match domain {
            ControlDomain::Render => &self.render_refs,
            ControlDomain::Audio => &self.audio_refs,
        }
    }

    fn refs_mut(&mut self, domain: ControlDomain) -> &mut Vec<Option<RefSlot>> {
        match domain {
            ControlDomain::Render => &mut self.render_refs,
            ControlDomain::Audio => &mut self.audio_refs,
        }
    }

    fn generations(&self, domain: ControlDomain) -> &Vec<u32> {
        match domain {
            ControlDomain::Render => &self.render_generations,
            ControlDomain::Audio => &self.audio_generations,
        }
    }

    fn generations_mut(&mut self, domain: ControlDomain) -> &mut Vec<u32> {
        match domain {
            ControlDomain::Render => &mut self.render_generations,
            ControlDomain::Audio => &mut self.audio_generations,
        }
    }

    fn endpoint_generation(&self, domain: ControlDomain) -> Option<Handle> {
        match domain {
            ControlDomain::Render => self.render_endpoint_generation,
            ControlDomain::Audio => self.audio_endpoint_generation,
        }
    }

    fn set_endpoint_generation(&mut self, domain: ControlDomain, generation: Option<Handle>) {
        match domain {
            ControlDomain::Render => self.render_endpoint_generation = generation,
            ControlDomain::Audio => self.audio_endpoint_generation = generation,
        }
    }

    fn ensure_open(&self) -> Result<(), ControlError> {
        if self.closed {
            Err(ControlError::Closed)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn is_tombstone_update(
    current: &ControlStateSnapshot,
    incoming: &ControlStateSnapshot,
) -> bool {
    if current.engine != incoming.engine
        || current.domain != incoming.domain
        || current.revision != incoming.revision
        || current.last_transaction_id != incoming.last_transaction_id
    {
        return false;
    }
    let incoming_entries = incoming
        .entries
        .iter()
        .map(|entry| (entry.stable, entry))
        .collect::<BTreeMap<_, _>>();
    current
        .entries
        .iter()
        .all(|entry| match incoming_entries.get(&entry.stable) {
            Some(incoming) if *incoming == entry => true,
            Some(incoming)
                if entry.descriptor == incoming.descriptor
                    && matches!(entry.state, ControlSnapshotState::Desired)
                    && matches!(incoming.state, ControlSnapshotState::Tombstone { .. }) =>
            {
                true
            }
            None if matches!(entry.state, ControlSnapshotState::Tombstone { .. }) => true,
            _ => false,
        })
        && incoming.entries.iter().all(|entry| {
            current.entries.iter().any(|current| {
                current.stable == entry.stable
                    && (current == entry
                        || current.descriptor == entry.descriptor
                            && matches!(current.state, ControlSnapshotState::Desired)
                            && matches!(entry.state, ControlSnapshotState::Tombstone { .. }))
            })
        })
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

    fn store(engine: u32) -> EngineControlStore {
        EngineControlStore::new(
            handle(engine),
            EngineControlConfig {
                max_leases: 4,
                max_refs: 4,
                max_ops_per_transaction: 4,
                max_descriptor_bytes: 16,
                max_snapshot_bytes: 32,
            },
        )
        .unwrap()
    }

    #[test]
    fn stable_ref_only_emerges_after_matching_logic_safe_point() {
        let mut store = store(1);
        let logic_generation = handle(7);
        let lease = store.issue_lease(handle(3), logic_generation, 2).unwrap();
        let mut transaction = lease.begin(ControlDomain::Render, 1, 0);
        let token = transaction
            .create(ControlKind::RenderView, b"view".to_vec())
            .unwrap();
        let ack = store.commit(transaction).unwrap();
        assert_eq!(ack.control_revision, 1);
        let (error, ack) = ack.publish_at_safe_point(handle(8)).unwrap_err();
        assert_eq!(error, ControlError::WrongLogicGeneration);
        let committed = ack.publish_at_safe_point(logic_generation).unwrap();
        assert_eq!(committed.promotions.len(), 1);
        assert_eq!(token, 0);
    }

    #[test]
    fn desired_state_survives_endpoint_restart_with_same_stable_ref() {
        let mut store = store(1);
        let logic = handle(7);
        let lease = store.issue_lease(handle(3), logic, 1).unwrap();
        let mut transaction = lease.begin(ControlDomain::Render, 1, 0);
        transaction
            .create(ControlKind::RenderView, b"view".to_vec())
            .unwrap();
        let stable = store
            .commit(transaction)
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap()
            .promotions[0]
            .1;
        assert_eq!(store.state(stable), Some(ControlRefState::Desired));
        store.begin_realization(stable).unwrap();
        assert_eq!(store.state(stable), Some(ControlRefState::Realizing));
        store
            .record_realization(stable, handle(20), RealizationOutcome::Live)
            .unwrap();
        store
            .restart_endpoint(
                ControlDomain::Render,
                handle(20),
                Handle {
                    index: 20,
                    generation: 2,
                },
            )
            .unwrap();
        assert_eq!(store.state(stable), Some(ControlRefState::Realizing));
        assert_eq!(store.descriptor(stable), Some(b"view".as_slice()));
        store
            .record_realization(
                stable,
                Handle {
                    index: 20,
                    generation: 2,
                },
                RealizationOutcome::Live,
            )
            .unwrap();
        assert_eq!(
            store.state(stable),
            Some(ControlRefState::Live {
                endpoint_generation: Handle {
                    index: 20,
                    generation: 2,
                }
            })
        );
    }

    #[test]
    fn wrong_engine_refs_and_failed_preflight_leave_revision_unchanged() {
        let mut alpha = store(1);
        let mut beta = store(2);
        let logic = handle(7);
        let lease = alpha.issue_lease(handle(3), logic, 1).unwrap();
        let mut transaction = lease.begin(ControlDomain::Audio, 1, 0);
        transaction
            .create(ControlKind::AudioBus, b"bus".to_vec())
            .unwrap();
        let stable = alpha
            .commit(transaction)
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap()
            .promotions[0]
            .1;
        let lease = beta.issue_lease(handle(3), logic, 1).unwrap();
        let mut transaction = lease.begin(ControlDomain::Audio, 1, 0);
        assert_eq!(
            transaction.retire(
                stable,
                RetirementFence {
                    last_render_revision: 0,
                    last_domain_frame: 0,
                    last_event_sequence: 0,
                    last_audio_event_sequence: 0,
                }
            ),
            Err(ControlError::WrongEngine)
        );
        assert_eq!(beta.audio_revision, 0);
    }

    #[test]
    fn tombstone_waits_for_every_retirement_fence_dimension() {
        let mut store = store(1);
        let logic = handle(7);
        let lease = store.issue_lease(handle(3), logic, 1).unwrap();
        let mut create = lease.begin(ControlDomain::Render, 1, 0);
        create
            .create(ControlKind::RenderTarget, b"target".to_vec())
            .unwrap();
        let stable = store
            .commit(create)
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap()
            .promotions[0]
            .1;
        let lease = store.issue_lease(handle(3), logic, 1).unwrap();
        let mut retire = lease.begin(ControlDomain::Render, 2, 1);
        retire
            .retire(
                stable,
                RetirementFence {
                    last_render_revision: 4,
                    last_domain_frame: 5,
                    last_event_sequence: 6,
                    last_audio_event_sequence: 0,
                },
            )
            .unwrap();
        store.commit(retire).unwrap();
        assert_eq!(store.state(stable), Some(ControlRefState::Tombstone));
        assert!(!store.retirement_ready(
            stable,
            RetirementProgress {
                render_revision: 4,
                domain_frame: 5,
                event_sequence: 5,
                audio_event_sequence: 0,
            }
        ));
        assert!(store.retirement_ready(
            stable,
            RetirementProgress {
                render_revision: 4,
                domain_frame: 5,
                event_sequence: 6,
                audio_event_sequence: 0,
            }
        ));
    }

    #[test]
    fn control_snapshot_preserves_stable_desired_state_without_realized_handle() {
        let mut store = store(1);
        let logic = handle(7);
        let lease = store.issue_lease(handle(3), logic, 1).unwrap();
        let mut create = lease.begin(ControlDomain::Render, 9, 0);
        create
            .create(ControlKind::RenderView, b"main-view".to_vec())
            .unwrap();
        let stable = store
            .commit(create)
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap()
            .promotions[0]
            .1;
        store.begin_realization(stable).unwrap();
        store
            .record_realization(stable, handle(20), RealizationOutcome::Live)
            .unwrap();
        assert_eq!(
            store.snapshot(ControlDomain::Render).unwrap(),
            ControlStateSnapshot {
                engine: handle(1),
                domain: ControlDomain::Render,
                revision: 1,
                last_transaction_id: 9,
                entries: vec![ControlSnapshotEntry {
                    stable,
                    descriptor: b"main-view".to_vec(),
                    state: ControlSnapshotState::Desired,
                }],
            }
        );
    }

    #[test]
    fn snapshot_budget_failure_does_not_change_control_state() {
        let mut store = EngineControlStore::new(
            handle(1),
            EngineControlConfig {
                max_leases: 2,
                max_refs: 2,
                max_ops_per_transaction: 2,
                max_descriptor_bytes: 16,
                max_snapshot_bytes: 2,
            },
        )
        .unwrap();
        let logic = handle(7);
        let lease = store.issue_lease(handle(3), logic, 1).unwrap();
        let mut create = lease.begin(ControlDomain::Audio, 1, 0);
        create
            .create(ControlKind::AudioBus, b"music".to_vec())
            .unwrap();
        let stable = store
            .commit(create)
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap()
            .promotions[0]
            .1;
        assert_eq!(
            store.snapshot(ControlDomain::Audio),
            Err(ControlError::SnapshotCapacity)
        );
        assert_eq!(store.state(stable), Some(ControlRefState::Desired));
        assert_eq!(store.descriptor(stable), Some(b"music".as_slice()));
    }
}
