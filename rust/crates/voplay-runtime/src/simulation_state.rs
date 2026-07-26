use std::collections::BTreeMap;

use crate::world::WorldId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationStateConfig {
    pub max_resources: usize,
    pub max_resource_bytes: usize,
    pub max_total_resource_bytes: usize,
    pub max_stage_events: usize,
    pub max_event_bytes: usize,
    pub max_total_event_bytes: usize,
}

impl Default for SimulationStateConfig {
    fn default() -> Self {
        Self {
            max_resources: 4096,
            max_resource_bytes: 1024 * 1024,
            max_total_resource_bytes: 16 * 1024 * 1024,
            max_stage_events: 16_384,
            max_event_bytes: 256 * 1024,
            max_total_event_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimulationResourceOp {
    Set { resource: u32, value: Vec<u8> },
    Remove { resource: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationEvent {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulationStateSnapshot {
    pub schema_version: u32,
    pub world: WorldId,
    pub revision: u64,
    pub resources: BTreeMap<u32, Vec<u8>>,
    pub readable_events: Vec<SimulationEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimulationStateError {
    Closed,
    InvalidConfig,
    WrongWorld,
    InvalidResource,
    ResourceNotFound,
    ResourceCapacity,
    ResourceValueCapacity,
    ResourceTotalCapacity,
    InvalidEvent,
    EventCapacity,
    EventValueCapacity,
    EventTotalCapacity,
    RevisionExhausted,
    SnapshotMalformed,
}

pub struct SimulationStateCommit {
    resources: BTreeMap<u32, Vec<u8>>,
    readable_events: Vec<SimulationEvent>,
    revision: u64,
}

impl SimulationStateCommit {
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

pub struct SimulationState {
    world: WorldId,
    config: SimulationStateConfig,
    revision: u64,
    resources: BTreeMap<u32, Vec<u8>>,
    readable_events: Vec<SimulationEvent>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationStateOwnerSnapshot {
    pub closed: bool,
    pub resources: usize,
    pub resource_bytes: usize,
    pub events: usize,
    pub event_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationStateShutdownReport {
    pub released_resources: usize,
    pub released_resource_bytes: usize,
    pub released_events: usize,
    pub released_event_bytes: usize,
}

impl SimulationState {
    pub fn new(
        world: WorldId,
        config: SimulationStateConfig,
    ) -> Result<Self, SimulationStateError> {
        validate_config(world, config)?;
        Ok(Self {
            world,
            config,
            revision: 0,
            resources: BTreeMap::new(),
            readable_events: Vec::new(),
            closed: false,
        })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn resource(&self, resource: u32) -> Option<&[u8]> {
        self.resources.get(&resource).map(Vec::as_slice)
    }

    pub fn events(&self, kind: u32) -> impl Iterator<Item = &SimulationEvent> {
        self.readable_events
            .iter()
            .filter(move |event| event.kind == kind)
    }

    pub fn prepare_stage(
        &self,
        resource_ops: Vec<SimulationResourceOp>,
        emitted_events: Vec<SimulationEvent>,
    ) -> Result<SimulationStateCommit, SimulationStateError> {
        self.ensure_open()?;
        let mut resources = self.resources.clone();
        for op in resource_ops {
            match op {
                SimulationResourceOp::Set { resource, value } => {
                    if resource == 0 {
                        return Err(SimulationStateError::InvalidResource);
                    }
                    if value.len() > self.config.max_resource_bytes {
                        return Err(SimulationStateError::ResourceValueCapacity);
                    }
                    resources.insert(resource, value);
                }
                SimulationResourceOp::Remove { resource } => {
                    if resource == 0 {
                        return Err(SimulationStateError::InvalidResource);
                    }
                    if resources.remove(&resource).is_none() {
                        return Err(SimulationStateError::ResourceNotFound);
                    }
                }
            }
        }
        validate_resources(&resources, self.config)?;
        validate_events(&emitted_events, self.config)?;
        let changed = resources != self.resources || emitted_events != self.readable_events;
        let revision = if changed {
            self.revision
                .checked_add(1)
                .ok_or(SimulationStateError::RevisionExhausted)?
        } else {
            self.revision
        };
        Ok(SimulationStateCommit {
            resources,
            readable_events: emitted_events,
            revision,
        })
    }

    pub fn commit(&mut self, commit: SimulationStateCommit) -> Result<(), SimulationStateError> {
        self.ensure_open()?;
        self.resources = commit.resources;
        self.readable_events = commit.readable_events;
        self.revision = commit.revision;
        Ok(())
    }

    pub fn deterministic_digest(&self) -> u64 {
        let mut digest = 0xcbf29ce484222325u64;
        digest = fnv_mix(digest, &self.revision.to_le_bytes());
        for (resource, value) in &self.resources {
            digest = fnv_mix(digest, &resource.to_le_bytes());
            digest = fnv_mix(digest, &(value.len() as u64).to_le_bytes());
            digest = fnv_mix(digest, value);
        }
        for event in &self.readable_events {
            digest = fnv_mix(digest, &event.kind.to_le_bytes());
            digest = fnv_mix(digest, &(event.bytes.len() as u64).to_le_bytes());
            digest = fnv_mix(digest, &event.bytes);
        }
        digest
    }

    pub fn snapshot(&self) -> SimulationStateSnapshot {
        SimulationStateSnapshot {
            schema_version: 1,
            world: self.world,
            revision: self.revision,
            resources: self.resources.clone(),
            readable_events: self.readable_events.clone(),
        }
    }

    pub fn owner_snapshot(&self) -> SimulationStateOwnerSnapshot {
        SimulationStateOwnerSnapshot {
            closed: self.closed,
            resources: self.resources.len(),
            resource_bytes: self.resources.values().map(Vec::len).sum(),
            events: self.readable_events.len(),
            event_bytes: self
                .readable_events
                .iter()
                .map(|event| event.bytes.len())
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> SimulationStateShutdownReport {
        if self.closed {
            return SimulationStateShutdownReport {
                released_resources: 0,
                released_resource_bytes: 0,
                released_events: 0,
                released_event_bytes: 0,
            };
        }
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.resources.clear();
        self.readable_events = Vec::new();
        SimulationStateShutdownReport {
            released_resources: snapshot.resources,
            released_resource_bytes: snapshot.resource_bytes,
            released_events: snapshot.events,
            released_event_bytes: snapshot.event_bytes,
        }
    }

    pub fn restore(
        snapshot: SimulationStateSnapshot,
        expected_world: WorldId,
        config: SimulationStateConfig,
    ) -> Result<Self, SimulationStateError> {
        validate_config(expected_world, config)?;
        if snapshot.schema_version != 1 {
            return Err(SimulationStateError::SnapshotMalformed);
        }
        if snapshot.world != expected_world {
            return Err(SimulationStateError::WrongWorld);
        }
        validate_resources(&snapshot.resources, config)?;
        validate_events(&snapshot.readable_events, config)?;
        Ok(Self {
            world: expected_world,
            config,
            revision: snapshot.revision,
            resources: snapshot.resources,
            readable_events: snapshot.readable_events,
            closed: false,
        })
    }

    fn ensure_open(&self) -> Result<(), SimulationStateError> {
        if self.closed {
            Err(SimulationStateError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_config(
    world: WorldId,
    config: SimulationStateConfig,
) -> Result<(), SimulationStateError> {
    if !world.is_valid()
        || config.max_resources == 0
        || config.max_resource_bytes == 0
        || config.max_total_resource_bytes == 0
        || config.max_stage_events == 0
        || config.max_event_bytes == 0
        || config.max_total_event_bytes == 0
    {
        return Err(SimulationStateError::InvalidConfig);
    }
    Ok(())
}

fn validate_resources(
    resources: &BTreeMap<u32, Vec<u8>>,
    config: SimulationStateConfig,
) -> Result<(), SimulationStateError> {
    if resources.len() > config.max_resources || resources.keys().any(|resource| *resource == 0) {
        return Err(SimulationStateError::ResourceCapacity);
    }
    let bytes = resources.values().try_fold(0usize, |total, value| {
        if value.len() > config.max_resource_bytes {
            return None;
        }
        total.checked_add(value.len())
    });
    if bytes.is_none_or(|bytes| bytes > config.max_total_resource_bytes) {
        return Err(SimulationStateError::ResourceTotalCapacity);
    }
    Ok(())
}

fn validate_events(
    events: &[SimulationEvent],
    config: SimulationStateConfig,
) -> Result<(), SimulationStateError> {
    if events.len() > config.max_stage_events {
        return Err(SimulationStateError::EventCapacity);
    }
    let bytes = events.iter().try_fold(0usize, |total, event| {
        if event.kind == 0 || event.bytes.len() > config.max_event_bytes {
            return None;
        }
        total.checked_add(event.bytes.len())
    });
    if bytes.is_none_or(|bytes| bytes > config.max_total_event_bytes) {
        return Err(SimulationStateError::EventTotalCapacity);
    }
    Ok(())
}

fn fnv_mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;

    fn world() -> WorldId {
        WorldId {
            engine: Handle {
                index: 1,
                generation: 1,
            },
            handle: Handle {
                index: 2,
                generation: 1,
            },
        }
    }

    #[test]
    fn failed_stage_is_atomic_and_unchanged_stage_does_not_advance_revision() {
        let mut state = SimulationState::new(world(), SimulationStateConfig::default()).unwrap();
        let commit = state
            .prepare_stage(
                vec![SimulationResourceOp::Set {
                    resource: 1,
                    value: vec![1, 2],
                }],
                vec![SimulationEvent {
                    kind: 7,
                    bytes: vec![3],
                }],
            )
            .unwrap();
        state.commit(commit).unwrap();
        let digest = state.deterministic_digest();
        assert_eq!(state.revision(), 1);

        assert!(matches!(
            state.prepare_stage(
                vec![SimulationResourceOp::Remove { resource: 99 }],
                Vec::new(),
            ),
            Err(SimulationStateError::ResourceNotFound)
        ));
        assert_eq!(state.deterministic_digest(), digest);
        let unchanged = state
            .prepare_stage(
                Vec::new(),
                vec![SimulationEvent {
                    kind: 7,
                    bytes: vec![3],
                }],
            )
            .unwrap();
        assert_eq!(unchanged.revision(), 1);
    }

    #[test]
    fn snapshot_restore_preserves_digest_and_rejects_wrong_world() {
        let mut state = SimulationState::new(world(), SimulationStateConfig::default()).unwrap();
        let commit = state
            .prepare_stage(
                vec![SimulationResourceOp::Set {
                    resource: 4,
                    value: vec![5, 6],
                }],
                Vec::new(),
            )
            .unwrap();
        state.commit(commit).unwrap();
        let snapshot = state.snapshot();
        let restored =
            SimulationState::restore(snapshot.clone(), world(), SimulationStateConfig::default())
                .unwrap();
        assert_eq!(
            restored.deterministic_digest(),
            state.deterministic_digest()
        );

        let wrong = WorldId {
            engine: world().engine,
            handle: Handle {
                index: 3,
                generation: 1,
            },
        };
        assert!(matches!(
            SimulationState::restore(snapshot, wrong, SimulationStateConfig::default()),
            Err(SimulationStateError::WrongWorld)
        ));
    }
}
