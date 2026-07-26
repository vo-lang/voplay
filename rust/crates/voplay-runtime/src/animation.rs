use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::Handle;

use crate::world::{WorldEntity, WorldId};

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnimationMachineId {
    pub world: WorldId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AnimationStateId(pub u32);

impl AnimationStateId {
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationStateDescriptor {
    pub id: AnimationStateId,
    pub root_motion_per_tick: [i64; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterCondition {
    Equal { parameter: u32, value: i64 },
    NotEqual { parameter: u32, value: i64 },
    AtLeast { parameter: u32, value: i64 },
    AtMost { parameter: u32, value: i64 },
}

impl ParameterCondition {
    fn parameter(self) -> u32 {
        match self {
            Self::Equal { parameter, .. }
            | Self::NotEqual { parameter, .. }
            | Self::AtLeast { parameter, .. }
            | Self::AtMost { parameter, .. } => parameter,
        }
    }

    fn matches(self, parameters: &BTreeMap<u32, i64>) -> bool {
        let current = parameters.get(&self.parameter()).copied().unwrap_or(0);
        match self {
            Self::Equal { value, .. } => current == value,
            Self::NotEqual { value, .. } => current != value,
            Self::AtLeast { value, .. } => current >= value,
            Self::AtMost { value, .. } => current <= value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationTransitionDescriptor {
    pub id: u32,
    pub from: AnimationStateId,
    pub to: AnimationStateId,
    pub minimum_state_ticks: u64,
    pub priority: i32,
    pub condition: Option<ParameterCondition>,
    pub gameplay_event: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationMachineDescriptor {
    pub entity: WorldEntity,
    pub initial_state: AnimationStateId,
    pub states: Vec<AnimationStateDescriptor>,
    pub transitions: Vec<AnimationTransitionDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationRuntimeConfig {
    pub max_machines: usize,
    pub max_states_per_machine: usize,
    pub max_transitions_per_machine: usize,
    pub max_parameters_per_machine: usize,
    pub max_commands_per_tick: usize,
    pub max_events_per_tick: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for AnimationRuntimeConfig {
    fn default() -> Self {
        Self {
            max_machines: 65_536,
            max_states_per_machine: 256,
            max_transitions_per_machine: 1_024,
            max_parameters_per_machine: 256,
            max_commands_per_tick: 65_536,
            max_events_per_tick: 65_536,
            max_snapshot_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationCommand {
    SetParameter {
        machine: AnimationMachineId,
        parameter: u32,
        value: i64,
    },
    RemoveParameter {
        machine: AnimationMachineId,
        parameter: u32,
    },
}

impl AnimationCommand {
    fn key(self) -> (AnimationMachineId, u32) {
        match self {
            Self::SetParameter {
                machine, parameter, ..
            }
            | Self::RemoveParameter { machine, parameter } => (machine, parameter),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationPose {
    pub machine: AnimationMachineId,
    pub entity: WorldEntity,
    pub state: AnimationStateId,
    pub state_ticks: u64,
    pub root_motion_delta: [i64; 3],
    pub accumulated_root_motion: [i64; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationGameplayEvent {
    pub tick: u64,
    pub machine: AnimationMachineId,
    pub entity: WorldEntity,
    pub transition: u32,
    pub event: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationFrame {
    pub tick: u64,
    pub poses: Vec<AnimationPose>,
    pub events: Vec<AnimationGameplayEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationMachineSnapshot {
    pub descriptor: AnimationMachineDescriptor,
    pub state: AnimationStateId,
    pub state_ticks: u64,
    pub parameters: BTreeMap<u32, i64>,
    pub accumulated_root_motion: [i64; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationSnapshotSlot {
    pub generation: u32,
    pub machine: Option<AnimationMachineSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationSnapshot {
    pub schema_version: u32,
    pub world: WorldId,
    pub tick: u64,
    pub slots: Vec<AnimationSnapshotSlot>,
    pub free: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnimationError {
    InvalidConfig,
    InvalidDescriptor,
    WrongWorld,
    MachineCapacity,
    StateCapacity,
    TransitionCapacity,
    ParameterCapacity,
    CommandCapacity,
    EventCapacity,
    DuplicateCommand,
    InvalidMachine,
    GenerationExhausted,
    TickSequence,
    ArithmeticOverflow,
    SnapshotCapacity,
    SnapshotMalformed,
    RestoreRequiresEmpty,
    Closed,
}

#[derive(Clone, Debug)]
struct Machine {
    descriptor: AnimationMachineDescriptor,
    state: AnimationStateId,
    state_ticks: u64,
    parameters: BTreeMap<u32, i64>,
    accumulated_root_motion: [i64; 3],
}

impl Machine {
    fn snapshot(&self) -> AnimationMachineSnapshot {
        AnimationMachineSnapshot {
            descriptor: self.descriptor.clone(),
            state: self.state,
            state_ticks: self.state_ticks,
            parameters: self.parameters.clone(),
            accumulated_root_motion: self.accumulated_root_motion,
        }
    }

    fn from_snapshot(snapshot: AnimationMachineSnapshot) -> Self {
        Self {
            descriptor: snapshot.descriptor,
            state: snapshot.state,
            state_ticks: snapshot.state_ticks,
            parameters: snapshot.parameters,
            accumulated_root_motion: snapshot.accumulated_root_motion,
        }
    }
}

#[derive(Clone, Debug)]
struct MachineSlot {
    generation: u32,
    machine: Option<Machine>,
}

pub struct AnimationRuntime {
    world: WorldId,
    config: AnimationRuntimeConfig,
    tick: u64,
    slots: Vec<MachineSlot>,
    free: Vec<u32>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationRuntimeOwnerSnapshot {
    pub world: WorldId,
    pub closed: bool,
    pub tick: u64,
    pub machines: usize,
    pub parameters: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationRuntimeShutdownReport {
    pub before: AnimationRuntimeOwnerSnapshot,
    pub after: AnimationRuntimeOwnerSnapshot,
}

impl AnimationRuntime {
    pub fn new(world: WorldId, config: AnimationRuntimeConfig) -> Result<Self, AnimationError> {
        if !world.is_valid()
            || config.max_machines == 0
            || config.max_machines > u32::MAX as usize
            || config.max_states_per_machine == 0
            || config.max_transitions_per_machine == 0
            || config.max_parameters_per_machine == 0
            || config.max_commands_per_tick == 0
            || config.max_events_per_tick == 0
            || config.max_snapshot_bytes == 0
        {
            return Err(AnimationError::InvalidConfig);
        }
        Ok(Self {
            world,
            config,
            tick: 0,
            slots: Vec::new(),
            free: Vec::new(),
            closed: false,
        })
    }

    pub const fn world(&self) -> WorldId {
        self.world
    }

    pub const fn tick_id(&self) -> u64 {
        self.tick
    }

    pub fn owner_snapshot(&self) -> AnimationRuntimeOwnerSnapshot {
        AnimationRuntimeOwnerSnapshot {
            world: self.world,
            closed: self.closed,
            tick: self.tick,
            machines: self
                .slots
                .iter()
                .filter(|slot| slot.machine.is_some())
                .count(),
            parameters: self
                .slots
                .iter()
                .filter_map(|slot| slot.machine.as_ref())
                .map(|machine| machine.parameters.len())
                .sum(),
        }
    }

    pub fn create_machine(
        &mut self,
        descriptor: AnimationMachineDescriptor,
    ) -> Result<AnimationMachineId, AnimationError> {
        self.ensure_open()?;
        validate_descriptor(self.world, &self.config, &descriptor)?;
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            if self.slots.len() >= self.config.max_machines {
                return Err(AnimationError::MachineCapacity);
            }
            let index =
                u32::try_from(self.slots.len()).map_err(|_| AnimationError::MachineCapacity)?;
            self.slots.push(MachineSlot {
                generation: 1,
                machine: None,
            });
            index
        };
        let slot = &mut self.slots[index as usize];
        let id = AnimationMachineId {
            world: self.world,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.machine = Some(Machine {
            state: descriptor.initial_state,
            descriptor,
            state_ticks: 0,
            parameters: BTreeMap::new(),
            accumulated_root_motion: [0; 3],
        });
        Ok(id)
    }

    pub fn destroy_machine(&mut self, id: AnimationMachineId) -> Result<(), AnimationError> {
        self.ensure_open()?;
        let index = self.machine_index(id)?;
        let next_generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(AnimationError::GenerationExhausted)?;
        self.slots[index].machine = None;
        self.slots[index].generation = next_generation;
        self.free.push(index as u32);
        Ok(())
    }

    pub fn machine_pose(&self, id: AnimationMachineId) -> Result<AnimationPose, AnimationError> {
        self.ensure_open()?;
        let index = self.machine_index(id)?;
        let machine = self.slots[index]
            .machine
            .as_ref()
            .ok_or(AnimationError::InvalidMachine)?;
        Ok(AnimationPose {
            machine: id,
            entity: machine.descriptor.entity,
            state: machine.state,
            state_ticks: machine.state_ticks,
            root_motion_delta: [0; 3],
            accumulated_root_motion: machine.accumulated_root_motion,
        })
    }

    pub fn step(
        &mut self,
        tick: u64,
        commands: &[AnimationCommand],
    ) -> Result<AnimationFrame, AnimationError> {
        self.ensure_open()?;
        if tick
            != self
                .tick
                .checked_add(1)
                .ok_or(AnimationError::TickSequence)?
        {
            return Err(AnimationError::TickSequence);
        }
        if commands.len() > self.config.max_commands_per_tick {
            return Err(AnimationError::CommandCapacity);
        }
        let mut command_keys = BTreeSet::new();
        let mut staged = self.slots.clone();
        for command in commands {
            let (id, parameter) = command.key();
            if parameter == 0 {
                return Err(AnimationError::InvalidDescriptor);
            }
            if !command_keys.insert((id, parameter)) {
                return Err(AnimationError::DuplicateCommand);
            }
            let index = machine_index_in(self.world, &staged, id)?;
            let machine = staged[index]
                .machine
                .as_mut()
                .ok_or(AnimationError::InvalidMachine)?;
            match *command {
                AnimationCommand::SetParameter { value, .. } => {
                    if !machine.parameters.contains_key(&parameter)
                        && machine.parameters.len() >= self.config.max_parameters_per_machine
                    {
                        return Err(AnimationError::ParameterCapacity);
                    }
                    machine.parameters.insert(parameter, value);
                }
                AnimationCommand::RemoveParameter { .. } => {
                    machine.parameters.remove(&parameter);
                }
            }
        }

        let mut poses = Vec::new();
        let mut events = Vec::new();
        for (index, slot) in staged.iter_mut().enumerate() {
            let generation = slot.generation;
            let Some(machine) = slot.machine.as_mut() else {
                continue;
            };
            let selected = machine
                .descriptor
                .transitions
                .iter()
                .filter(|transition| {
                    transition.from == machine.state
                        && machine.state_ticks >= transition.minimum_state_ticks
                        && transition
                            .condition
                            .is_none_or(|condition| condition.matches(&machine.parameters))
                })
                .min_by_key(|transition| (std::cmp::Reverse(transition.priority), transition.id))
                .copied();
            if let Some(transition) = selected {
                machine.state = transition.to;
                machine.state_ticks = 0;
                if let Some(event) = transition.gameplay_event {
                    if events.len() >= self.config.max_events_per_tick {
                        return Err(AnimationError::EventCapacity);
                    }
                    events.push(AnimationGameplayEvent {
                        tick,
                        machine: AnimationMachineId {
                            world: self.world,
                            handle: Handle {
                                index: index as u32,
                                generation,
                            },
                        },
                        entity: machine.descriptor.entity,
                        transition: transition.id,
                        event,
                    });
                }
            }
            let state = machine
                .descriptor
                .states
                .iter()
                .find(|state| state.id == machine.state)
                .ok_or(AnimationError::InvalidDescriptor)?;
            for axis in 0..3 {
                machine.accumulated_root_motion[axis] = machine.accumulated_root_motion[axis]
                    .checked_add(state.root_motion_per_tick[axis])
                    .ok_or(AnimationError::ArithmeticOverflow)?;
            }
            machine.state_ticks = machine
                .state_ticks
                .checked_add(1)
                .ok_or(AnimationError::ArithmeticOverflow)?;
            poses.push(AnimationPose {
                machine: AnimationMachineId {
                    world: self.world,
                    handle: Handle {
                        index: index as u32,
                        generation,
                    },
                },
                entity: machine.descriptor.entity,
                state: machine.state,
                state_ticks: machine.state_ticks,
                root_motion_delta: state.root_motion_per_tick,
                accumulated_root_motion: machine.accumulated_root_motion,
            });
        }
        self.slots = staged;
        self.tick = tick;
        Ok(AnimationFrame {
            tick,
            poses,
            events,
        })
    }

    pub fn snapshot(&self) -> Result<AnimationSnapshot, AnimationError> {
        self.ensure_open()?;
        let snapshot = AnimationSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            world: self.world,
            tick: self.tick,
            slots: self
                .slots
                .iter()
                .map(|slot| AnimationSnapshotSlot {
                    generation: slot.generation,
                    machine: slot.machine.as_ref().map(Machine::snapshot),
                })
                .collect(),
            free: self.free.clone(),
        };
        if snapshot_size(&snapshot)? > self.config.max_snapshot_bytes {
            return Err(AnimationError::SnapshotCapacity);
        }
        Ok(snapshot)
    }

    pub fn restore(&mut self, snapshot: AnimationSnapshot) -> Result<(), AnimationError> {
        self.ensure_open()?;
        if self.tick != 0 || !self.slots.is_empty() || !self.free.is_empty() {
            return Err(AnimationError::RestoreRequiresEmpty);
        }
        validate_snapshot(self.world, &self.config, &snapshot)?;
        self.tick = snapshot.tick;
        self.slots = snapshot
            .slots
            .into_iter()
            .map(|slot| MachineSlot {
                generation: slot.generation,
                machine: slot.machine.map(Machine::from_snapshot),
            })
            .collect();
        self.free = snapshot.free;
        Ok(())
    }

    pub fn shutdown(&mut self) -> AnimationRuntimeShutdownReport {
        let before = self.owner_snapshot();
        if !self.closed {
            self.slots.clear();
            self.free.clear();
            self.closed = true;
        }
        AnimationRuntimeShutdownReport {
            before,
            after: self.owner_snapshot(),
        }
    }

    fn machine_index(&self, id: AnimationMachineId) -> Result<usize, AnimationError> {
        machine_index_in(self.world, &self.slots, id)
    }

    fn ensure_open(&self) -> Result<(), AnimationError> {
        if self.closed {
            Err(AnimationError::Closed)
        } else {
            Ok(())
        }
    }
}

fn machine_index_in(
    world: WorldId,
    slots: &[MachineSlot],
    id: AnimationMachineId,
) -> Result<usize, AnimationError> {
    if id.world != world {
        return Err(AnimationError::WrongWorld);
    }
    let index = id.handle.index as usize;
    let slot = slots.get(index).ok_or(AnimationError::InvalidMachine)?;
    if id.handle.generation == 0
        || slot.generation != id.handle.generation
        || slot.machine.is_none()
    {
        return Err(AnimationError::InvalidMachine);
    }
    Ok(index)
}

fn validate_descriptor(
    world: WorldId,
    config: &AnimationRuntimeConfig,
    descriptor: &AnimationMachineDescriptor,
) -> Result<(), AnimationError> {
    if descriptor.entity.world != world {
        return Err(AnimationError::WrongWorld);
    }
    if !descriptor.initial_state.is_valid() || descriptor.states.is_empty() {
        return Err(AnimationError::InvalidDescriptor);
    }
    if descriptor.states.len() > config.max_states_per_machine {
        return Err(AnimationError::StateCapacity);
    }
    if descriptor.transitions.len() > config.max_transitions_per_machine {
        return Err(AnimationError::TransitionCapacity);
    }
    let states: BTreeSet<_> = descriptor.states.iter().map(|state| state.id).collect();
    if states.len() != descriptor.states.len() || !states.contains(&descriptor.initial_state) {
        return Err(AnimationError::InvalidDescriptor);
    }
    let mut transitions = BTreeSet::new();
    for transition in &descriptor.transitions {
        if transition.id == 0
            || !transitions.insert(transition.id)
            || !states.contains(&transition.from)
            || !states.contains(&transition.to)
            || transition.from == transition.to
            || transition
                .condition
                .is_some_and(|condition| condition.parameter() == 0)
            || transition.gameplay_event == Some(0)
        {
            return Err(AnimationError::InvalidDescriptor);
        }
    }
    Ok(())
}

fn snapshot_size(snapshot: &AnimationSnapshot) -> Result<usize, AnimationError> {
    let mut bytes = 32usize
        .checked_add(
            snapshot
                .slots
                .len()
                .checked_mul(8)
                .ok_or(AnimationError::SnapshotCapacity)?,
        )
        .and_then(|value| value.checked_add(snapshot.free.len().checked_mul(4)?))
        .ok_or(AnimationError::SnapshotCapacity)?;
    for slot in &snapshot.slots {
        let Some(machine) = &slot.machine else {
            continue;
        };
        bytes = bytes
            .checked_add(80)
            .and_then(|value| value.checked_add(machine.descriptor.states.len().checked_mul(32)?))
            .and_then(|value| {
                value.checked_add(machine.descriptor.transitions.len().checked_mul(64)?)
            })
            .and_then(|value| value.checked_add(machine.parameters.len().checked_mul(16)?))
            .ok_or(AnimationError::SnapshotCapacity)?;
    }
    Ok(bytes)
}

fn validate_snapshot(
    world: WorldId,
    config: &AnimationRuntimeConfig,
    snapshot: &AnimationSnapshot,
) -> Result<(), AnimationError> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION
        || snapshot.world != world
        || snapshot.slots.len() > config.max_machines
        || snapshot_size(snapshot)? > config.max_snapshot_bytes
    {
        return Err(AnimationError::SnapshotMalformed);
    }
    let free: BTreeSet<_> = snapshot.free.iter().copied().collect();
    if free.len() != snapshot.free.len() {
        return Err(AnimationError::SnapshotMalformed);
    }
    for (index, slot) in snapshot.slots.iter().enumerate() {
        if slot.generation == 0 || free.contains(&(index as u32)) != slot.machine.is_none() {
            return Err(AnimationError::SnapshotMalformed);
        }
        if let Some(machine) = &slot.machine {
            validate_descriptor(world, config, &machine.descriptor)
                .map_err(|_| AnimationError::SnapshotMalformed)?;
            if !machine
                .descriptor
                .states
                .iter()
                .any(|state| state.id == machine.state)
                || machine.parameters.len() > config.max_parameters_per_machine
                || machine.parameters.keys().any(|parameter| *parameter == 0)
            {
                return Err(AnimationError::SnapshotMalformed);
            }
        }
    }
    if free
        .iter()
        .any(|index| *index as usize >= snapshot.slots.len())
    {
        return Err(AnimationError::SnapshotMalformed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::{EngineId, EntityId};

    fn world_id(engine: u32) -> WorldId {
        WorldId {
            engine: EngineId {
                index: engine,
                generation: 1,
            },
            handle: Handle {
                index: 2,
                generation: 3,
            },
        }
    }

    fn entity(world: WorldId, index: u32) -> WorldEntity {
        WorldEntity {
            world,
            entity: EntityId {
                index,
                generation: 1,
            },
        }
    }

    fn descriptor(world: WorldId) -> AnimationMachineDescriptor {
        AnimationMachineDescriptor {
            entity: entity(world, 7),
            initial_state: AnimationStateId(1),
            states: vec![
                AnimationStateDescriptor {
                    id: AnimationStateId(2),
                    root_motion_per_tick: [3, 0, 0],
                },
                AnimationStateDescriptor {
                    id: AnimationStateId(1),
                    root_motion_per_tick: [0, 0, 0],
                },
                AnimationStateDescriptor {
                    id: AnimationStateId(3),
                    root_motion_per_tick: [0, 2, 0],
                },
            ],
            transitions: vec![
                AnimationTransitionDescriptor {
                    id: 20,
                    from: AnimationStateId(1),
                    to: AnimationStateId(3),
                    minimum_state_ticks: 1,
                    priority: 1,
                    condition: Some(ParameterCondition::AtLeast {
                        parameter: 1,
                        value: 1,
                    }),
                    gameplay_event: Some(300),
                },
                AnimationTransitionDescriptor {
                    id: 10,
                    from: AnimationStateId(1),
                    to: AnimationStateId(2),
                    minimum_state_ticks: 1,
                    priority: 2,
                    condition: Some(ParameterCondition::AtLeast {
                        parameter: 1,
                        value: 1,
                    }),
                    gameplay_event: Some(200),
                },
            ],
        }
    }

    #[test]
    fn stable_priority_transition_emits_event_and_applies_new_state_root_motion() {
        let world = world_id(1);
        let mut runtime = AnimationRuntime::new(world, AnimationRuntimeConfig::default()).unwrap();
        let machine = runtime.create_machine(descriptor(world)).unwrap();
        runtime.step(1, &[]).unwrap();
        let frame = runtime
            .step(
                2,
                &[AnimationCommand::SetParameter {
                    machine,
                    parameter: 1,
                    value: 1,
                }],
            )
            .unwrap();
        assert_eq!(frame.events.len(), 1);
        assert_eq!(frame.events[0].transition, 10);
        assert_eq!(frame.events[0].event, 200);
        assert_eq!(frame.poses[0].state, AnimationStateId(2));
        assert_eq!(frame.poses[0].root_motion_delta, [3, 0, 0]);
        assert_eq!(frame.poses[0].accumulated_root_motion, [3, 0, 0]);
    }

    #[test]
    fn command_failure_and_root_motion_overflow_leave_tick_and_state_unchanged() {
        let world = world_id(1);
        let mut config = AnimationRuntimeConfig::default();
        config.max_parameters_per_machine = 1;
        let mut runtime = AnimationRuntime::new(world, config).unwrap();
        let machine = runtime.create_machine(descriptor(world)).unwrap();
        runtime.step(1, &[]).unwrap();
        assert_eq!(
            runtime.step(
                2,
                &[
                    AnimationCommand::SetParameter {
                        machine,
                        parameter: 1,
                        value: 1,
                    },
                    AnimationCommand::SetParameter {
                        machine,
                        parameter: 2,
                        value: 1,
                    },
                ],
            ),
            Err(AnimationError::ParameterCapacity)
        );
        assert_eq!(runtime.tick_id(), 1);
        assert_eq!(
            runtime.machine_pose(machine).unwrap().state,
            AnimationStateId(1)
        );

        let mut snapshot = runtime.snapshot().unwrap();
        snapshot.slots[0]
            .machine
            .as_mut()
            .unwrap()
            .accumulated_root_motion = [i64::MAX, 0, 0];
        snapshot.slots[0].machine.as_mut().unwrap().state = AnimationStateId(2);
        let mut restored = AnimationRuntime::new(world, config).unwrap();
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored.step(2, &[]),
            Err(AnimationError::ArithmeticOverflow)
        );
        assert_eq!(restored.tick_id(), 1);
        assert_eq!(
            restored
                .machine_pose(machine)
                .unwrap()
                .accumulated_root_motion,
            [i64::MAX, 0, 0]
        );
    }

    #[test]
    fn generation_and_world_owner_reject_late_or_foreign_handles() {
        let world = world_id(1);
        let mut runtime = AnimationRuntime::new(world, AnimationRuntimeConfig::default()).unwrap();
        let first = runtime.create_machine(descriptor(world)).unwrap();
        runtime.destroy_machine(first).unwrap();
        let second = runtime.create_machine(descriptor(world)).unwrap();
        assert_eq!(first.handle.index, second.handle.index);
        assert_ne!(first.handle.generation, second.handle.generation);
        assert_eq!(
            runtime.machine_pose(first),
            Err(AnimationError::InvalidMachine)
        );
        let foreign = AnimationMachineId {
            world: world_id(2),
            handle: second.handle,
        };
        assert_eq!(
            runtime.machine_pose(foreign),
            Err(AnimationError::WrongWorld)
        );
    }

    #[test]
    fn snapshot_restore_replays_identical_frames_and_allocation_order() {
        let world = world_id(1);
        let config = AnimationRuntimeConfig::default();
        let mut original = AnimationRuntime::new(world, config).unwrap();
        let first = original.create_machine(descriptor(world)).unwrap();
        let removed = original.create_machine(descriptor(world)).unwrap();
        original.destroy_machine(removed).unwrap();
        original.step(1, &[]).unwrap();
        let snapshot = original.snapshot().unwrap();
        let mut restored = AnimationRuntime::new(world, config).unwrap();
        restored.restore(snapshot).unwrap();
        let command = [AnimationCommand::SetParameter {
            machine: first,
            parameter: 1,
            value: 1,
        }];
        assert_eq!(original.step(2, &command), restored.step(2, &command));
        let original_next = original.create_machine(descriptor(world)).unwrap();
        let restored_next = restored.create_machine(descriptor(world)).unwrap();
        assert_eq!(original_next, restored_next);
    }

    #[test]
    fn malformed_snapshot_does_not_change_empty_restore_target() {
        let world = world_id(1);
        let config = AnimationRuntimeConfig::default();
        let mut source = AnimationRuntime::new(world, config).unwrap();
        source.create_machine(descriptor(world)).unwrap();
        let mut snapshot = source.snapshot().unwrap();
        snapshot.free.push(0);
        let mut target = AnimationRuntime::new(world, config).unwrap();
        assert_eq!(
            target.restore(snapshot),
            Err(AnimationError::SnapshotMalformed)
        );
        assert_eq!(target.tick_id(), 0);
        let id = target.create_machine(descriptor(world)).unwrap();
        assert_eq!(id.handle.index, 0);
        assert_eq!(id.handle.generation, 1);
    }
}
