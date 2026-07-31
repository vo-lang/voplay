use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, EntityId, Handle};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorldId {
    pub engine: EngineId,
    pub handle: Handle,
}

impl WorldId {
    pub const fn is_valid(self) -> bool {
        self.engine.is_valid() && self.handle.is_valid()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorldEntity {
    pub world: WorldId,
    pub entity: EntityId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldConfig {
    pub max_entities: usize,
    pub max_components_per_entity: usize,
    pub max_component_bytes: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            max_entities: 1_000_000,
            max_components_per_entity: 256,
            max_component_bytes: 1024 * 1024,
            max_snapshot_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorldCommand {
    Spawn {
        stable_key: u64,
        components: BTreeMap<u32, Vec<u8>>,
    },
    Despawn {
        entity: WorldEntity,
    },
    SetComponent {
        entity: WorldEntity,
        component: u32,
        value: Vec<u8>,
    },
    RemoveComponent {
        entity: WorldEntity,
        component: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorldChangeKind {
    Spawn { stable_key: u64 },
    Despawn,
    SetComponent { component: u32 },
    RemoveComponent { component: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldChange {
    pub revision: u64,
    pub entity: WorldEntity,
    pub kind: WorldChangeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldStageResult {
    pub revision: u64,
    pub spawned: Vec<(u64, WorldEntity)>,
    pub change_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldSnapshotSlot {
    pub generation: u32,
    pub stable_key: Option<u64>,
    pub components: BTreeMap<u32, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldSnapshot {
    pub schema_version: u32,
    pub world: WorldId,
    pub revision: u64,
    pub slots: Vec<WorldSnapshotSlot>,
    pub free: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorldError {
    Closed,
    InvalidConfig,
    EntityCapacity,
    InvalidEntity,
    DuplicateEntityCommand,
    DuplicateStableKey,
    StableKeyAlreadyLive,
    InvalidComponent,
    ComponentCapacity,
    ComponentTooLarge,
    ComponentNotFound,
    RevisionExhausted,
    TickSequence,
    GenerationExhausted,
    SnapshotCapacity,
    SnapshotMalformed,
    WrongWorld,
    RestoreRequiresEmpty,
}

#[derive(Debug)]
struct EntityState {
    stable_key: u64,
    components: BTreeMap<u32, Vec<u8>>,
}

#[derive(Debug)]
struct EntitySlot {
    generation: u32,
    state: Option<EntityState>,
}

pub struct World {
    id: WorldId,
    config: WorldConfig,
    revision: u64,
    slots: Vec<EntitySlot>,
    free: Vec<u32>,
    live: usize,
    stable_keys: BTreeMap<u64, WorldEntity>,
    changes: Vec<WorldChange>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldOwnerSnapshot {
    pub closed: bool,
    pub live_entities: usize,
    pub allocated_slots: usize,
    pub free_slots: usize,
    pub stable_keys: usize,
    pub pending_changes: usize,
    pub component_entries: usize,
    pub component_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldShutdownReport {
    pub released_entities: usize,
    pub released_slots: usize,
    pub released_stable_keys: usize,
    pub released_changes: usize,
    pub released_component_entries: usize,
    pub released_component_bytes: usize,
}

impl World {
    pub fn new(id: WorldId, config: WorldConfig) -> Result<Self, WorldError> {
        if !id.is_valid()
            || config.max_entities == 0
            || config.max_entities > u32::MAX as usize
            || config.max_components_per_entity == 0
            || config.max_component_bytes == 0
            || config.max_snapshot_bytes == 0
        {
            return Err(WorldError::InvalidConfig);
        }
        Ok(Self {
            id,
            config,
            revision: 0,
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            stable_keys: BTreeMap::new(),
            changes: Vec::new(),
            closed: false,
        })
    }

    pub const fn id(&self) -> WorldId {
        self.id
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn live_entities(&self) -> usize {
        self.live
    }

    pub fn owner_snapshot(&self) -> WorldOwnerSnapshot {
        let mut component_entries = 0;
        let mut component_bytes = 0;
        for state in self.slots.iter().filter_map(|slot| slot.state.as_ref()) {
            component_entries += state.components.len();
            component_bytes += state.components.values().map(Vec::len).sum::<usize>();
        }
        WorldOwnerSnapshot {
            closed: self.closed,
            live_entities: self.live,
            allocated_slots: self.slots.len(),
            free_slots: self.free.len(),
            stable_keys: self.stable_keys.len(),
            pending_changes: self.changes.len(),
            component_entries,
            component_bytes,
        }
    }

    pub fn shutdown(&mut self) -> WorldShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.slots.clear();
        self.free.clear();
        self.live = 0;
        self.stable_keys.clear();
        self.changes.clear();
        WorldShutdownReport {
            released_entities: snapshot.live_entities,
            released_slots: snapshot.allocated_slots,
            released_stable_keys: snapshot.stable_keys,
            released_changes: snapshot.pending_changes,
            released_component_entries: snapshot.component_entries,
            released_component_bytes: snapshot.component_bytes,
        }
    }

    pub fn entity_for_stable_key(&self, stable_key: u64) -> Option<WorldEntity> {
        self.stable_keys.get(&stable_key).copied()
    }

    pub fn stable_key(&self, entity: WorldEntity) -> Option<u64> {
        self.entity(entity).ok().map(|state| state.stable_key)
    }

    pub fn is_live(&self, entity: WorldEntity) -> bool {
        self.entity(entity).is_ok()
    }

    pub fn component(&self, entity: WorldEntity, component: u32) -> Option<&[u8]> {
        self.entity(entity)
            .ok()?
            .components
            .get(&component)
            .map(Vec::as_slice)
    }

    pub fn apply_stage(
        &mut self,
        mut commands: Vec<WorldCommand>,
    ) -> Result<WorldStageResult, WorldError> {
        self.apply_stage_reusing(&mut commands)
    }

    pub fn apply_stage_reusing(
        &mut self,
        commands: &mut Vec<WorldCommand>,
    ) -> Result<WorldStageResult, WorldError> {
        self.ensure_open()?;
        self.preflight(commands)?;
        let has_changes = commands.iter().any(|command| self.command_changes(command));
        if !has_changes {
            return Ok(WorldStageResult {
                revision: self.revision,
                spawned: Vec::new(),
                change_count: 0,
            });
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(WorldError::RevisionExhausted)?;
        let change_start = self.changes.len();
        let mut spawned = Vec::new();
        for command in commands.drain(..) {
            match command {
                WorldCommand::Spawn {
                    stable_key,
                    components,
                } => {
                    let entity = self.allocate_entity(stable_key, components);
                    self.changes.push(WorldChange {
                        revision,
                        entity,
                        kind: WorldChangeKind::Spawn { stable_key },
                    });
                    spawned.push((stable_key, entity));
                }
                WorldCommand::Despawn { entity } => {
                    let stable_key = self.entity(entity).unwrap().stable_key;
                    self.release_entity(entity);
                    self.stable_keys.remove(&stable_key);
                    self.changes.push(WorldChange {
                        revision,
                        entity,
                        kind: WorldChangeKind::Despawn,
                    });
                }
                WorldCommand::SetComponent {
                    entity,
                    component,
                    value,
                } => {
                    let state = self.entity_mut(entity).unwrap();
                    if state.components.get(&component) == Some(&value) {
                        continue;
                    }
                    state.components.insert(component, value);
                    self.changes.push(WorldChange {
                        revision,
                        entity,
                        kind: WorldChangeKind::SetComponent { component },
                    });
                }
                WorldCommand::RemoveComponent { entity, component } => {
                    self.entity_mut(entity)
                        .unwrap()
                        .components
                        .remove(&component);
                    self.changes.push(WorldChange {
                        revision,
                        entity,
                        kind: WorldChangeKind::RemoveComponent { component },
                    });
                }
            }
        }
        self.revision = revision;
        Ok(WorldStageResult {
            revision,
            spawned,
            change_count: self.changes.len() - change_start,
        })
    }

    pub fn drain_changes(&mut self) -> Vec<WorldChange> {
        std::mem::take(&mut self.changes)
    }

    pub fn snapshot(&self) -> Result<WorldSnapshot, WorldError> {
        self.ensure_open()?;
        let mut bytes = self
            .slots
            .len()
            .checked_mul(16)
            .and_then(|bytes| {
                self.free
                    .len()
                    .checked_mul(4)
                    .and_then(|free| bytes.checked_add(free))
            })
            .filter(|bytes| *bytes <= self.config.max_snapshot_bytes)
            .ok_or(WorldError::SnapshotCapacity)?;
        let slots = self
            .slots
            .iter()
            .map(|slot| {
                let (stable_key, components) = slot
                    .state
                    .as_ref()
                    .map(|state| (Some(state.stable_key), state.components.clone()))
                    .unwrap_or_else(|| (None, BTreeMap::new()));
                bytes =
                    snapshot_component_bytes(bytes, &components, self.config.max_snapshot_bytes)?;
                Ok(WorldSnapshotSlot {
                    generation: slot.generation,
                    stable_key,
                    components,
                })
            })
            .collect::<Result<Vec<_>, WorldError>>()?;
        Ok(WorldSnapshot {
            schema_version: 1,
            world: self.id,
            revision: self.revision,
            slots,
            free: self.free.clone(),
        })
    }

    pub fn restore(&mut self, snapshot: WorldSnapshot) -> Result<(), WorldError> {
        self.ensure_open()?;
        if self.revision != 0 || self.live != 0 || !self.slots.is_empty() {
            return Err(WorldError::RestoreRequiresEmpty);
        }
        if snapshot.schema_version != 1 {
            return Err(WorldError::SnapshotMalformed);
        }
        if snapshot.world != self.id {
            return Err(WorldError::WrongWorld);
        }
        if snapshot.slots.len() > self.config.max_entities {
            return Err(WorldError::EntityCapacity);
        }
        let mut free_set = BTreeSet::new();
        for index in &snapshot.free {
            if (*index as usize) >= snapshot.slots.len() || !free_set.insert(*index) {
                return Err(WorldError::SnapshotMalformed);
            }
        }
        let mut bytes = snapshot
            .slots
            .len()
            .checked_mul(16)
            .and_then(|bytes| {
                snapshot
                    .free
                    .len()
                    .checked_mul(4)
                    .and_then(|free| bytes.checked_add(free))
            })
            .filter(|bytes| *bytes <= self.config.max_snapshot_bytes)
            .ok_or(WorldError::SnapshotCapacity)?;
        let mut stable_keys = BTreeMap::new();
        let mut slots = Vec::with_capacity(snapshot.slots.len());
        let mut live = 0_usize;
        for (index, slot) in snapshot.slots.into_iter().enumerate() {
            if slot.generation == 0 {
                return Err(WorldError::SnapshotMalformed);
            }
            bytes =
                snapshot_component_bytes(bytes, &slot.components, self.config.max_snapshot_bytes)?;
            let state = if let Some(stable_key) = slot.stable_key {
                if stable_key == 0 || free_set.contains(&(index as u32)) {
                    return Err(WorldError::SnapshotMalformed);
                }
                self.validate_components(&slot.components)?;
                let entity = WorldEntity {
                    world: self.id,
                    entity: Handle {
                        index: index as u32,
                        generation: slot.generation,
                    },
                };
                if stable_keys.insert(stable_key, entity).is_some() {
                    return Err(WorldError::DuplicateStableKey);
                }
                live += 1;
                Some(EntityState {
                    stable_key,
                    components: slot.components,
                })
            } else {
                if !slot.components.is_empty() || !free_set.contains(&(index as u32)) {
                    return Err(WorldError::SnapshotMalformed);
                }
                None
            };
            slots.push(EntitySlot {
                generation: slot.generation,
                state,
            });
        }
        if free_set.len().checked_add(live) != Some(slots.len()) {
            return Err(WorldError::SnapshotMalformed);
        }
        self.revision = snapshot.revision;
        self.slots = slots;
        self.free = snapshot.free;
        self.live = live;
        self.stable_keys = stable_keys;
        self.changes.clear();
        Ok(())
    }

    fn preflight(&self, commands: &[WorldCommand]) -> Result<(), WorldError> {
        self.ensure_open()?;
        let mut target_entities = BTreeSet::new();
        let mut spawn_keys = BTreeSet::new();
        let mut final_count = self.live;
        for command in commands {
            match command {
                WorldCommand::Spawn {
                    stable_key,
                    components,
                } => {
                    if *stable_key == 0 || !spawn_keys.insert(*stable_key) {
                        return Err(WorldError::DuplicateStableKey);
                    }
                    if self.stable_keys.contains_key(stable_key) {
                        return Err(WorldError::StableKeyAlreadyLive);
                    }
                    self.validate_components(components)?;
                    final_count = final_count
                        .checked_add(1)
                        .ok_or(WorldError::EntityCapacity)?;
                }
                WorldCommand::Despawn { entity }
                | WorldCommand::SetComponent { entity, .. }
                | WorldCommand::RemoveComponent { entity, .. } => {
                    let state = self.entity(*entity)?;
                    if !target_entities.insert(*entity) {
                        return Err(WorldError::DuplicateEntityCommand);
                    }
                    match command {
                        WorldCommand::Despawn { .. } => {
                            if entity.entity.generation == u32::MAX {
                                return Err(WorldError::GenerationExhausted);
                            }
                            final_count -= 1;
                        }
                        WorldCommand::SetComponent {
                            component, value, ..
                        } => {
                            if *component == 0 {
                                return Err(WorldError::InvalidComponent);
                            }
                            if value.len() > self.config.max_component_bytes {
                                return Err(WorldError::ComponentTooLarge);
                            }
                            if !state.components.contains_key(component)
                                && state.components.len() == self.config.max_components_per_entity
                            {
                                return Err(WorldError::ComponentCapacity);
                            }
                        }
                        WorldCommand::RemoveComponent { component, .. } => {
                            if !state.components.contains_key(component) {
                                return Err(WorldError::ComponentNotFound);
                            }
                        }
                        WorldCommand::Spawn { .. } => unreachable!(),
                    }
                }
            }
        }
        if final_count > self.config.max_entities {
            return Err(WorldError::EntityCapacity);
        }
        Ok(())
    }

    fn validate_components(&self, components: &BTreeMap<u32, Vec<u8>>) -> Result<(), WorldError> {
        if components.len() > self.config.max_components_per_entity {
            return Err(WorldError::ComponentCapacity);
        }
        for (component, value) in components {
            if *component == 0 {
                return Err(WorldError::InvalidComponent);
            }
            if value.len() > self.config.max_component_bytes {
                return Err(WorldError::ComponentTooLarge);
            }
        }
        Ok(())
    }

    fn command_changes(&self, command: &WorldCommand) -> bool {
        match command {
            WorldCommand::SetComponent {
                entity,
                component,
                value,
            } => self.component(*entity, *component) != Some(value.as_slice()),
            _ => true,
        }
    }

    fn allocate_entity(
        &mut self,
        stable_key: u64,
        components: BTreeMap<u32, Vec<u8>>,
    ) -> WorldEntity {
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(EntitySlot {
                generation: 1,
                state: None,
            });
            index
        };
        let slot = &mut self.slots[index as usize];
        let entity = WorldEntity {
            world: self.id,
            entity: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.state = Some(EntityState {
            stable_key,
            components,
        });
        self.stable_keys.insert(stable_key, entity);
        self.live += 1;
        entity
    }

    fn release_entity(&mut self, entity: WorldEntity) {
        let slot = &mut self.slots[entity.entity.index as usize];
        slot.state = None;
        slot.generation = slot
            .generation
            .checked_add(1)
            .expect("preflight rejects generation exhaustion");
        self.free.push(entity.entity.index);
        self.live -= 1;
    }

    fn entity(&self, entity: WorldEntity) -> Result<&EntityState, WorldError> {
        if entity.world != self.id {
            return Err(WorldError::InvalidEntity);
        }
        let slot = self
            .slots
            .get(entity.entity.index as usize)
            .ok_or(WorldError::InvalidEntity)?;
        if slot.generation != entity.entity.generation {
            return Err(WorldError::InvalidEntity);
        }
        slot.state.as_ref().ok_or(WorldError::InvalidEntity)
    }

    fn entity_mut(&mut self, entity: WorldEntity) -> Result<&mut EntityState, WorldError> {
        self.ensure_open()?;
        if entity.world != self.id {
            return Err(WorldError::InvalidEntity);
        }
        let slot = self
            .slots
            .get_mut(entity.entity.index as usize)
            .ok_or(WorldError::InvalidEntity)?;
        if slot.generation != entity.entity.generation {
            return Err(WorldError::InvalidEntity);
        }
        slot.state.as_mut().ok_or(WorldError::InvalidEntity)
    }

    fn ensure_open(&self) -> Result<(), WorldError> {
        if self.closed {
            Err(WorldError::Closed)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputFrame {
    pub tick_id: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationTick {
    pub tick_id: u64,
    pub world_revision: u64,
    pub simulation_revision: u64,
    pub deterministic_hash: u64,
}

pub struct SimulationClock {
    tick_id: u64,
    deterministic_hash: u64,
    presentation_notifications: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationClockOwnerSnapshot {
    pub closed: bool,
    pub tick_id: u64,
    pub presentation_notifications: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationClockShutdownReport {
    pub final_tick_id: u64,
    pub released_presentation_notifications: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationClockSnapshot {
    pub schema_version: u32,
    pub tick_id: u64,
    pub deterministic_hash: u64,
}

impl Default for SimulationClock {
    fn default() -> Self {
        Self {
            tick_id: 0,
            deterministic_hash: 0xcbf29ce484222325,
            presentation_notifications: 0,
            closed: false,
        }
    }
}

impl SimulationClock {
    pub const fn snapshot(&self) -> SimulationClockSnapshot {
        SimulationClockSnapshot {
            schema_version: 1,
            tick_id: self.tick_id,
            deterministic_hash: self.deterministic_hash,
        }
    }

    pub fn restore(snapshot: SimulationClockSnapshot) -> Result<Self, WorldError> {
        if snapshot.schema_version != 1 {
            return Err(WorldError::SnapshotMalformed);
        }
        Ok(Self {
            tick_id: snapshot.tick_id,
            deterministic_hash: snapshot.deterministic_hash,
            presentation_notifications: 0,
            closed: false,
        })
    }

    pub const fn owner_snapshot(&self) -> SimulationClockOwnerSnapshot {
        SimulationClockOwnerSnapshot {
            closed: self.closed,
            tick_id: self.tick_id,
            presentation_notifications: self.presentation_notifications,
        }
    }

    pub fn shutdown(&mut self) -> SimulationClockShutdownReport {
        let report = SimulationClockShutdownReport {
            final_tick_id: self.tick_id,
            released_presentation_notifications: self.presentation_notifications,
        };
        self.closed = true;
        self.presentation_notifications = 0;
        report
    }

    pub fn advance(
        &mut self,
        input: &InputFrame,
        world_revision: u64,
    ) -> Result<SimulationTick, WorldError> {
        self.advance_with_state(input, world_revision, world_revision, 0)
    }

    pub fn advance_with_state(
        &mut self,
        input: &InputFrame,
        world_revision: u64,
        simulation_revision: u64,
        simulation_state_digest: u64,
    ) -> Result<SimulationTick, WorldError> {
        if self.closed {
            return Err(WorldError::Closed);
        }
        let next = self
            .tick_id
            .checked_add(1)
            .ok_or(WorldError::RevisionExhausted)?;
        if input.tick_id != next {
            return Err(WorldError::TickSequence);
        }
        self.tick_id = next;
        self.deterministic_hash = fnv_mix(self.deterministic_hash, &next.to_le_bytes());
        self.deterministic_hash = fnv_mix(self.deterministic_hash, &world_revision.to_le_bytes());
        self.deterministic_hash =
            fnv_mix(self.deterministic_hash, &simulation_revision.to_le_bytes());
        self.deterministic_hash = fnv_mix(
            self.deterministic_hash,
            &simulation_state_digest.to_le_bytes(),
        );
        self.deterministic_hash = fnv_mix(self.deterministic_hash, &input.bytes);
        Ok(SimulationTick {
            tick_id: next,
            world_revision,
            simulation_revision,
            deterministic_hash: self.deterministic_hash,
        })
    }

    pub fn notify_presentation(&mut self) {
        if self.closed {
            return;
        }
        self.presentation_notifications = self.presentation_notifications.saturating_add(1);
    }

    pub const fn deterministic_hash(&self) -> u64 {
        self.deterministic_hash
    }
}

fn fnv_mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn snapshot_component_bytes(
    mut total: usize,
    components: &BTreeMap<u32, Vec<u8>>,
    max_bytes: usize,
) -> Result<usize, WorldError> {
    for value in components.values() {
        total = total
            .checked_add(4)
            .and_then(|total| total.checked_add(value.len()))
            .filter(|total| *total <= max_bytes)
            .ok_or(WorldError::SnapshotCapacity)?;
    }
    Ok(total)
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

    fn world(index: u32) -> World {
        World::new(
            WorldId {
                engine: handle(100),
                handle: handle(index),
            },
            WorldConfig {
                max_entities: 4,
                max_components_per_entity: 4,
                max_component_bytes: 8,
                max_snapshot_bytes: 64,
            },
        )
        .unwrap()
    }

    fn components(value: u8) -> BTreeMap<u32, Vec<u8>> {
        BTreeMap::from([(1, vec![value])])
    }

    #[test]
    fn world_stage_is_atomic_and_journal_tracks_actual_changes() {
        let mut world = world(0);
        let spawned = world
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 7,
                components: components(1),
            }])
            .unwrap();
        let entity = spawned.spawned[0].1;
        assert_eq!(spawned.revision, 1);
        assert_eq!(spawned.change_count, 1);
        world.drain_changes();

        assert_eq!(
            world.apply_stage(vec![
                WorldCommand::SetComponent {
                    entity,
                    component: 1,
                    value: vec![2],
                },
                WorldCommand::Despawn { entity },
            ]),
            Err(WorldError::DuplicateEntityCommand)
        );
        assert_eq!(world.revision(), 1);
        assert_eq!(world.component(entity, 1), Some([1].as_slice()));
        assert!(world.drain_changes().is_empty());

        let no_change = world
            .apply_stage(vec![WorldCommand::SetComponent {
                entity,
                component: 1,
                value: vec![1],
            }])
            .unwrap();
        assert_eq!(no_change.change_count, 0);
        assert_eq!(no_change.revision, 1);
    }

    #[test]
    fn entity_generation_and_world_owner_reject_late_or_cross_world_handles() {
        let mut alpha = world(0);
        let mut beta = world(1);
        let mut foreign_engine = World::new(
            WorldId {
                engine: handle(101),
                handle: handle(0),
            },
            WorldConfig {
                max_entities: 4,
                max_components_per_entity: 4,
                max_component_bytes: 8,
                max_snapshot_bytes: 64,
            },
        )
        .unwrap();
        let entity = alpha
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 1,
                components: components(1),
            }])
            .unwrap()
            .spawned[0]
            .1;
        let beta_entity = beta
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 9,
                components: components(9),
            }])
            .unwrap()
            .spawned[0]
            .1;
        let foreign_entity = foreign_engine
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 1,
                components: components(7),
            }])
            .unwrap()
            .spawned[0]
            .1;
        assert_eq!(entity.entity, beta_entity.entity);
        assert_eq!(entity.entity, foreign_entity.entity);
        assert_eq!(entity.world.handle, foreign_entity.world.handle);
        assert_ne!(entity.world.engine, foreign_entity.world.engine);
        assert!(beta.component(entity, 1).is_none());
        assert!(foreign_engine.component(entity, 1).is_none());
        assert_eq!(
            beta.apply_stage(vec![WorldCommand::Despawn { entity }]),
            Err(WorldError::InvalidEntity)
        );
        alpha
            .apply_stage(vec![WorldCommand::Despawn { entity }])
            .unwrap();
        let current = alpha
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 2,
                components: components(2),
            }])
            .unwrap()
            .spawned[0]
            .1;
        assert_eq!(entity.entity.index, current.entity.index);
        assert_ne!(entity.entity.generation, current.entity.generation);
        assert!(alpha.component(entity, 1).is_none());
    }

    #[test]
    fn display_pulses_do_not_change_simulation_hash() {
        let mut alpha = SimulationClock::default();
        let mut beta = SimulationClock::default();
        alpha.notify_presentation();
        alpha.notify_presentation();
        for tick_id in 1..=3 {
            let input = InputFrame {
                tick_id,
                bytes: vec![tick_id as u8],
            };
            alpha.advance(&input, tick_id).unwrap();
            beta.advance(&input, tick_id).unwrap();
            beta.notify_presentation();
        }
        assert_eq!(alpha.deterministic_hash(), beta.deterministic_hash());
    }

    #[test]
    fn generation_exhaustion_rejects_despawn_without_mutation() {
        let mut world = world(0);
        let entity = world
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 1,
                components: components(1),
            }])
            .unwrap()
            .spawned[0]
            .1;
        world.slots[entity.entity.index as usize].generation = u32::MAX;
        let saturated = WorldEntity {
            entity: Handle {
                index: entity.entity.index,
                generation: u32::MAX,
            },
            ..entity
        };
        assert_eq!(
            world.apply_stage(vec![WorldCommand::Despawn { entity: saturated }]),
            Err(WorldError::GenerationExhausted)
        );
        assert_eq!(world.live_entities(), 1);
        assert_eq!(world.revision(), 1);
    }

    #[test]
    fn world_snapshot_roundtrip_preserves_owner_generation_and_allocation_order() {
        let mut source = world(0);
        let first = source
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 1,
                components: components(1),
            }])
            .unwrap()
            .spawned[0]
            .1;
        source
            .apply_stage(vec![WorldCommand::Despawn { entity: first }])
            .unwrap();
        let snapshot = source.snapshot().unwrap();
        let mut restored = world(0);
        restored.restore(snapshot).unwrap();
        assert_eq!(restored.revision(), source.revision());
        assert_eq!(restored.live_entities(), 0);
        let next = restored
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 2,
                components: components(2),
            }])
            .unwrap()
            .spawned[0]
            .1;
        assert_eq!(next.entity.index, first.entity.index);
        assert_eq!(next.entity.generation, first.entity.generation + 1);
        assert_eq!(restored.component(next, 1), Some([2].as_slice()));
    }

    #[test]
    fn malformed_or_cross_world_snapshot_leaves_empty_target_unchanged() {
        let mut source = world(0);
        source
            .apply_stage(vec![WorldCommand::Spawn {
                stable_key: 1,
                components: components(1),
            }])
            .unwrap();
        let mut foreign = source.snapshot().unwrap();
        foreign.world.handle = handle(9);
        let mut target = world(0);
        assert_eq!(target.restore(foreign), Err(WorldError::WrongWorld));
        assert_eq!(target.revision(), 0);
        assert_eq!(target.live_entities(), 0);

        let mut malformed = source.snapshot().unwrap();
        malformed.free.push(0);
        assert_eq!(
            target.restore(malformed),
            Err(WorldError::SnapshotMalformed)
        );
        assert_eq!(target.revision(), 0);
        assert_eq!(target.live_entities(), 0);
    }

    #[test]
    fn simulation_clock_snapshot_excludes_presentation_and_resumes_hash_chain() {
        let mut source = SimulationClock::default();
        for tick_id in 1..=3 {
            source
                .advance(
                    &InputFrame {
                        tick_id,
                        bytes: vec![tick_id as u8],
                    },
                    tick_id,
                )
                .unwrap();
            source.notify_presentation();
        }
        let mut restored = SimulationClock::restore(source.snapshot()).unwrap();
        source.notify_presentation();
        restored.notify_presentation();
        restored.notify_presentation();
        let input = InputFrame {
            tick_id: 4,
            bytes: vec![4],
        };
        assert_eq!(
            source.advance(&input, 4).unwrap().deterministic_hash,
            restored.advance(&input, 4).unwrap().deterministic_hash
        );
    }
}
