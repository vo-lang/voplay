use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::Handle;

use crate::{
    schedule::{Schedule, Stage},
    world::{WorldEntity, WorldId},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicsSpace {
    TwoD,
    ThreeD,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhysicsBodyId {
    pub world: WorldId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhysicsColliderId {
    pub world: WorldId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhysicsJointId {
    pub world: WorldId,
    pub handle: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyKind {
    Static,
    Kinematic,
    Dynamic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBodyDescriptor {
    pub entity: WorldEntity,
    pub kind: BodyKind,
    pub pose: [i64; 3],
    pub velocity: [i64; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsColliderDescriptor {
    pub body: PhysicsBodyId,
    pub half_extent: [u32; 3],
    pub layer: u32,
    pub mask: u32,
    pub sensor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JointKind {
    Fixed {
        offset: [i64; 3],
    },
    DistanceLimit {
        min_distance_squared: u64,
        max_distance_squared: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsJointDescriptor {
    pub first: PhysicsBodyId,
    pub second: PhysicsBodyId,
    pub kind: JointKind,
    pub collide_connected: bool,
    pub break_threshold_squared: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicsCommand {
    SetWorldPose {
        body: PhysicsBodyId,
        pose: [i64; 3],
    },
    SetKinematicTarget {
        body: PhysicsBodyId,
        pose: [i64; 3],
    },
    Teleport {
        body: PhysicsBodyId,
        pose: [i64; 3],
    },
    AddImpulse {
        body: PhysicsBodyId,
        impulse: [i64; 3],
    },
    SetVelocity {
        body: PhysicsBodyId,
        velocity: [i64; 3],
    },
}

impl PhysicsCommand {
    const fn body(self) -> PhysicsBodyId {
        match self {
            Self::SetWorldPose { body, .. }
            | Self::SetKinematicTarget { body, .. }
            | Self::Teleport { body, .. }
            | Self::AddImpulse { body, .. }
            | Self::SetVelocity { body, .. } => body,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContactPair {
    pub tick: u64,
    pub first: PhysicsColliderId,
    pub second: PhysicsColliderId,
    pub sensor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JointEventKind {
    ConstraintApplied,
    Broken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JointEvent {
    pub tick: u64,
    pub joint: PhysicsJointId,
    pub kind: JointEventKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicsCastQuery {
    Ray {
        origin: [i64; 3],
        delta: [i64; 3],
        layer_mask: u32,
    },
    Shape {
        center: [i64; 3],
        half_extent: [u32; 3],
        delta: [i64; 3],
        layer_mask: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsCastHit {
    pub collider: PhysicsColliderId,
    pub toi_numerator: u64,
    pub toi_denominator: u64,
    pub sensor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBackendHandle {
    pub world: WorldId,
    pub generation: Handle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsPostStep {
    pub tick: u64,
    pub poses: Vec<(WorldEntity, [i64; 3])>,
    pub contacts: Vec<ContactPair>,
    pub joint_events: Vec<JointEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsConfig {
    pub max_bodies: usize,
    pub max_colliders: usize,
    pub max_joints: usize,
    pub max_commands_per_tick: usize,
    pub max_contacts_per_tick: usize,
    pub max_joint_events_per_tick: usize,
    pub max_query_batch: usize,
    pub max_query_hits: usize,
    pub max_snapshot_bytes: usize,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            max_bodies: 100_000,
            max_colliders: 200_000,
            max_joints: 100_000,
            max_commands_per_tick: 100_000,
            max_contacts_per_tick: 200_000,
            max_joint_events_per_tick: 100_000,
            max_query_batch: 4096,
            max_query_hits: 65_536,
            max_snapshot_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicsError {
    InvalidConfig,
    WrongWorld,
    InvalidBody,
    InvalidCollider,
    DuplicateEntity,
    BodyCapacity,
    ColliderCapacity,
    BodyHasColliders,
    JointCapacity,
    InvalidJoint,
    BodyHasJoints,
    InvalidShape,
    InvalidFilter,
    TickSequence,
    CommandCapacity,
    DuplicateBodyCommand,
    OwnerPolicy,
    ArithmeticOverflow,
    ContactCapacity,
    JointEventCapacity,
    QueryCapacity,
    SnapshotCapacity,
    SnapshotMalformed,
    RestoreRequiresEmpty,
    GenerationExhausted,
    WrongActor,
    StaleBackend,
    BackendCapacity,
    InvalidStageBinding,
    StageSequence,
    Closed,
}

pub struct PhysicsBackendAdapter {
    world: WorldId,
    actor: Handle,
    generation: Handle,
    max_batch_commands: usize,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBackendOwnerSnapshot {
    pub handle: PhysicsBackendHandle,
    pub actor: Handle,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsBackendShutdownReport {
    pub before: PhysicsBackendOwnerSnapshot,
    pub after: PhysicsBackendOwnerSnapshot,
}

impl PhysicsBackendAdapter {
    pub fn new(
        world: WorldId,
        actor: Handle,
        generation: Handle,
        max_batch_commands: usize,
    ) -> Result<Self, PhysicsError> {
        if !world.is_valid()
            || !actor.is_valid()
            || !generation.is_valid()
            || max_batch_commands == 0
        {
            return Err(PhysicsError::InvalidConfig);
        }
        Ok(Self {
            world,
            actor,
            generation,
            max_batch_commands,
            closed: false,
        })
    }

    pub const fn handle(&self) -> PhysicsBackendHandle {
        PhysicsBackendHandle {
            world: self.world,
            generation: self.generation,
        }
    }

    pub const fn owner_snapshot(&self) -> PhysicsBackendOwnerSnapshot {
        PhysicsBackendOwnerSnapshot {
            handle: self.handle(),
            actor: self.actor,
            closed: self.closed,
        }
    }

    pub fn execute_tick(
        &self,
        handle: PhysicsBackendHandle,
        actor: Handle,
        runtime: &mut PhysicsRuntime,
        tick: u64,
        commands: Vec<PhysicsCommand>,
    ) -> Result<PhysicsPostStep, PhysicsError> {
        self.validate_call(handle, actor, runtime, commands.len())?;
        runtime.apply_prephysics(tick, commands)?;
        runtime.step(tick)?;
        Ok(runtime.post_step())
    }

    pub fn restart(&mut self) -> Result<PhysicsBackendHandle, PhysicsError> {
        if self.closed {
            return Err(PhysicsError::Closed);
        }
        let generation = self
            .generation
            .generation
            .checked_add(1)
            .ok_or(PhysicsError::GenerationExhausted)?;
        self.generation.generation = generation;
        Ok(self.handle())
    }

    pub fn shutdown(&mut self) -> Result<PhysicsBackendShutdownReport, PhysicsError> {
        let before = self.owner_snapshot();
        if self.closed {
            return Ok(PhysicsBackendShutdownReport {
                before,
                after: before,
            });
        }
        let generation = self
            .generation
            .generation
            .checked_add(1)
            .ok_or(PhysicsError::GenerationExhausted)?;
        self.generation.generation = generation;
        self.closed = true;
        Ok(PhysicsBackendShutdownReport {
            before,
            after: self.owner_snapshot(),
        })
    }

    fn validate_call(
        &self,
        handle: PhysicsBackendHandle,
        actor: Handle,
        runtime: &PhysicsRuntime,
        command_count: usize,
    ) -> Result<(), PhysicsError> {
        if self.closed {
            return Err(PhysicsError::Closed);
        }
        if actor != self.actor {
            return Err(PhysicsError::WrongActor);
        }
        if handle != self.handle() {
            return Err(PhysicsError::StaleBackend);
        }
        if runtime.world != self.world {
            return Err(PhysicsError::WrongWorld);
        }
        if command_count > self.max_batch_commands {
            return Err(PhysicsError::BackendCapacity);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsStageBinding {
    schedule_hash: u64,
    prephysics_system: String,
    physics_system: String,
    postphysics_system: String,
}

impl PhysicsStageBinding {
    pub fn bind(
        schedule: &Schedule,
        prephysics_system: &str,
        physics_system: &str,
        postphysics_system: &str,
    ) -> Result<Self, PhysicsError> {
        for (name, stage) in [
            (prephysics_system, Stage::PrePhysics),
            (physics_system, Stage::Physics),
            (postphysics_system, Stage::PostPhysics),
        ] {
            if schedule
                .systems()
                .iter()
                .filter(|system| system.name == name && system.stage == stage)
                .count()
                != 1
            {
                return Err(PhysicsError::InvalidStageBinding);
            }
        }
        Ok(Self {
            schedule_hash: schedule.hash(),
            prephysics_system: prephysics_system.to_owned(),
            physics_system: physics_system.to_owned(),
            postphysics_system: postphysics_system.to_owned(),
        })
    }

    pub const fn schedule_hash(&self) -> u64 {
        self.schedule_hash
    }

    pub fn system_for(&self, stage: Stage) -> Option<&str> {
        match stage {
            Stage::PrePhysics => Some(&self.prephysics_system),
            Stage::Physics => Some(&self.physics_system),
            Stage::PostPhysics => Some(&self.postphysics_system),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhysicsStageState {
    Idle,
    PrePhysics { tick: u64 },
    Physics { tick: u64 },
}

pub struct PhysicsStageAdapter {
    backend: PhysicsBackendAdapter,
    binding: PhysicsStageBinding,
    state: PhysicsStageState,
}

impl PhysicsStageAdapter {
    pub fn new(backend: PhysicsBackendAdapter, binding: PhysicsStageBinding) -> Self {
        Self {
            backend,
            binding,
            state: PhysicsStageState::Idle,
        }
    }

    pub const fn backend_handle(&self) -> PhysicsBackendHandle {
        self.backend.handle()
    }

    pub const fn schedule_hash(&self) -> u64 {
        self.binding.schedule_hash
    }

    pub fn run_prephysics(
        &mut self,
        handle: PhysicsBackendHandle,
        actor: Handle,
        runtime: &mut PhysicsRuntime,
        tick: u64,
        commands: Vec<PhysicsCommand>,
    ) -> Result<(), PhysicsError> {
        if self.state != PhysicsStageState::Idle {
            return Err(PhysicsError::StageSequence);
        }
        self.backend
            .validate_call(handle, actor, runtime, commands.len())?;
        runtime.apply_prephysics(tick, commands)?;
        self.state = PhysicsStageState::PrePhysics { tick };
        Ok(())
    }

    pub fn run_physics(
        &mut self,
        handle: PhysicsBackendHandle,
        actor: Handle,
        runtime: &mut PhysicsRuntime,
        tick: u64,
    ) -> Result<(), PhysicsError> {
        if self.state != (PhysicsStageState::PrePhysics { tick }) {
            return Err(PhysicsError::StageSequence);
        }
        self.backend.validate_call(handle, actor, runtime, 0)?;
        runtime.step(tick)?;
        self.state = PhysicsStageState::Physics { tick };
        Ok(())
    }

    pub fn run_postphysics(
        &mut self,
        handle: PhysicsBackendHandle,
        actor: Handle,
        runtime: &PhysicsRuntime,
        tick: u64,
    ) -> Result<PhysicsPostStep, PhysicsError> {
        if self.state != (PhysicsStageState::Physics { tick }) || runtime.tick() != tick {
            return Err(PhysicsError::StageSequence);
        }
        self.backend.validate_call(handle, actor, runtime, 0)?;
        let post = runtime.post_step();
        self.state = PhysicsStageState::Idle;
        Ok(post)
    }

    pub fn restart_backend(&mut self) -> Result<PhysicsBackendHandle, PhysicsError> {
        if self.state != PhysicsStageState::Idle {
            return Err(PhysicsError::StageSequence);
        }
        self.backend.restart()
    }

    pub fn shutdown(&mut self) -> Result<PhysicsBackendShutdownReport, PhysicsError> {
        self.state = PhysicsStageState::Idle;
        self.backend.shutdown()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BodyState {
    entity: WorldEntity,
    kind: BodyKind,
    pose: [i64; 3],
    velocity: [i64; 3],
    kinematic_target: Option<[i64; 3]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BodySlot {
    generation: u32,
    state: Option<BodyState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ColliderState {
    body: PhysicsBodyId,
    half_extent: [u32; 3],
    layer: u32,
    mask: u32,
    sensor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ColliderSlot {
    generation: u32,
    state: Option<ColliderState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct JointState {
    descriptor: PhysicsJointDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct JointSlot {
    generation: u32,
    state: Option<JointState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsBodySnapshot {
    pub generation: u32,
    pub descriptor: Option<PhysicsBodyDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsColliderSnapshot {
    pub generation: u32,
    pub descriptor: Option<PhysicsColliderDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsJointSnapshot {
    pub generation: u32,
    pub descriptor: Option<PhysicsJointDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicsSnapshot {
    pub schema_version: u32,
    pub world: WorldId,
    pub space: PhysicsSpace,
    pub tick: u64,
    pub bodies: Vec<PhysicsBodySnapshot>,
    pub body_free: Vec<u32>,
    pub colliders: Vec<PhysicsColliderSnapshot>,
    pub collider_free: Vec<u32>,
    pub joints: Vec<PhysicsJointSnapshot>,
    pub joint_free: Vec<u32>,
}

pub struct PhysicsRuntime {
    world: WorldId,
    space: PhysicsSpace,
    config: PhysicsConfig,
    tick: u64,
    prepared_tick: Option<u64>,
    bodies: Vec<BodySlot>,
    body_free: Vec<u32>,
    entities: BTreeMap<WorldEntity, PhysicsBodyId>,
    colliders: Vec<ColliderSlot>,
    collider_free: Vec<u32>,
    joints: Vec<JointSlot>,
    joint_free: Vec<u32>,
    contacts: Vec<ContactPair>,
    joint_events: Vec<JointEvent>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsRuntimeOwnerSnapshot {
    pub world: WorldId,
    pub closed: bool,
    pub tick: u64,
    pub prepared_tick: bool,
    pub bodies: usize,
    pub colliders: usize,
    pub joints: usize,
    pub contacts: usize,
    pub joint_events: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicsRuntimeShutdownReport {
    pub before: PhysicsRuntimeOwnerSnapshot,
    pub after: PhysicsRuntimeOwnerSnapshot,
}

impl PhysicsRuntime {
    pub fn new(
        world: WorldId,
        space: PhysicsSpace,
        config: PhysicsConfig,
    ) -> Result<Self, PhysicsError> {
        if !world.is_valid()
            || config.max_bodies == 0
            || config.max_bodies > u32::MAX as usize
            || config.max_colliders == 0
            || config.max_colliders > u32::MAX as usize
            || config.max_joints == 0
            || config.max_joints > u32::MAX as usize
            || config.max_commands_per_tick == 0
            || config.max_contacts_per_tick == 0
            || config.max_joint_events_per_tick == 0
            || config.max_query_batch == 0
            || config.max_query_hits == 0
            || config.max_snapshot_bytes == 0
        {
            return Err(PhysicsError::InvalidConfig);
        }
        Ok(Self {
            world,
            space,
            config,
            tick: 0,
            prepared_tick: None,
            bodies: Vec::new(),
            body_free: Vec::new(),
            entities: BTreeMap::new(),
            colliders: Vec::new(),
            collider_free: Vec::new(),
            joints: Vec::new(),
            joint_free: Vec::new(),
            contacts: Vec::new(),
            joint_events: Vec::new(),
            closed: false,
        })
    }

    pub const fn tick(&self) -> u64 {
        self.tick
    }

    pub fn owner_snapshot(&self) -> PhysicsRuntimeOwnerSnapshot {
        PhysicsRuntimeOwnerSnapshot {
            world: self.world,
            closed: self.closed,
            tick: self.tick,
            prepared_tick: self.prepared_tick.is_some(),
            bodies: self
                .bodies
                .iter()
                .filter(|slot| slot.state.is_some())
                .count(),
            colliders: self
                .colliders
                .iter()
                .filter(|slot| slot.state.is_some())
                .count(),
            joints: self
                .joints
                .iter()
                .filter(|slot| slot.state.is_some())
                .count(),
            contacts: self.contacts.len(),
            joint_events: self.joint_events.len(),
        }
    }

    pub fn create_body(
        &mut self,
        descriptor: PhysicsBodyDescriptor,
    ) -> Result<PhysicsBodyId, PhysicsError> {
        self.ensure_open()?;
        if descriptor.entity.world != self.world {
            return Err(PhysicsError::WrongWorld);
        }
        if self.entities.contains_key(&descriptor.entity) {
            return Err(PhysicsError::DuplicateEntity);
        }
        if self.entities.len() == self.config.max_bodies {
            return Err(PhysicsError::BodyCapacity);
        }
        let index = if let Some(index) = self.body_free.pop() {
            index
        } else {
            let index = self.bodies.len() as u32;
            self.bodies.push(BodySlot {
                generation: 1,
                state: None,
            });
            index
        };
        let slot = &mut self.bodies[index as usize];
        let body = PhysicsBodyId {
            world: self.world,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.state = Some(BodyState {
            entity: descriptor.entity,
            kind: descriptor.kind,
            pose: descriptor.pose,
            velocity: descriptor.velocity,
            kinematic_target: None,
        });
        self.entities.insert(descriptor.entity, body);
        Ok(body)
    }

    pub fn destroy_body(&mut self, body: PhysicsBodyId) -> Result<(), PhysicsError> {
        self.ensure_open()?;
        let state = *self.body(body)?;
        if self
            .colliders
            .iter()
            .any(|slot| slot.state.is_some_and(|collider| collider.body == body))
        {
            return Err(PhysicsError::BodyHasColliders);
        }
        if self.joints.iter().any(|slot| {
            slot.state.is_some_and(|joint| {
                joint.descriptor.first == body || joint.descriptor.second == body
            })
        }) {
            return Err(PhysicsError::BodyHasJoints);
        }
        let generation = self.bodies[body.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(PhysicsError::GenerationExhausted)?;
        let slot = &mut self.bodies[body.handle.index as usize];
        slot.state = None;
        slot.generation = generation;
        self.body_free.push(body.handle.index);
        self.entities.remove(&state.entity);
        Ok(())
    }

    pub fn create_collider(
        &mut self,
        descriptor: PhysicsColliderDescriptor,
    ) -> Result<PhysicsColliderId, PhysicsError> {
        self.ensure_open()?;
        self.body(descriptor.body)?;
        validate_collider(self.space, descriptor)?;
        let live = self
            .colliders
            .iter()
            .filter(|slot| slot.state.is_some())
            .count();
        if live == self.config.max_colliders {
            return Err(PhysicsError::ColliderCapacity);
        }
        let index = if let Some(index) = self.collider_free.pop() {
            index
        } else {
            let index = self.colliders.len() as u32;
            self.colliders.push(ColliderSlot {
                generation: 1,
                state: None,
            });
            index
        };
        let slot = &mut self.colliders[index as usize];
        let collider = PhysicsColliderId {
            world: self.world,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.state = Some(ColliderState {
            body: descriptor.body,
            half_extent: descriptor.half_extent,
            layer: descriptor.layer,
            mask: descriptor.mask,
            sensor: descriptor.sensor,
        });
        Ok(collider)
    }

    pub fn destroy_collider(&mut self, collider: PhysicsColliderId) -> Result<(), PhysicsError> {
        self.ensure_open()?;
        self.collider(collider)?;
        let generation = self.colliders[collider.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(PhysicsError::GenerationExhausted)?;
        let slot = &mut self.colliders[collider.handle.index as usize];
        slot.state = None;
        slot.generation = generation;
        self.collider_free.push(collider.handle.index);
        Ok(())
    }

    pub fn collider_body(
        &self,
        collider: PhysicsColliderId,
    ) -> Result<PhysicsBodyId, PhysicsError> {
        self.ensure_open()?;
        Ok(self.collider(collider)?.body)
    }

    pub fn create_joint(
        &mut self,
        descriptor: PhysicsJointDescriptor,
    ) -> Result<PhysicsJointId, PhysicsError> {
        self.ensure_open()?;
        self.body(descriptor.first)?;
        self.body(descriptor.second)?;
        validate_joint(descriptor)?;
        let live = self
            .joints
            .iter()
            .filter(|slot| slot.state.is_some())
            .count();
        if live == self.config.max_joints {
            return Err(PhysicsError::JointCapacity);
        }
        let index = if let Some(index) = self.joint_free.pop() {
            index
        } else {
            let index = self.joints.len() as u32;
            self.joints.push(JointSlot {
                generation: 1,
                state: None,
            });
            index
        };
        let slot = &mut self.joints[index as usize];
        let joint = PhysicsJointId {
            world: self.world,
            handle: Handle {
                index,
                generation: slot.generation,
            },
        };
        slot.state = Some(JointState { descriptor });
        Ok(joint)
    }

    pub fn destroy_joint(&mut self, joint: PhysicsJointId) -> Result<(), PhysicsError> {
        self.ensure_open()?;
        self.joint(joint)?;
        let generation = self.joints[joint.handle.index as usize]
            .generation
            .checked_add(1)
            .ok_or(PhysicsError::GenerationExhausted)?;
        let slot = &mut self.joints[joint.handle.index as usize];
        slot.state = None;
        slot.generation = generation;
        self.joint_free.push(joint.handle.index);
        Ok(())
    }

    pub fn joint_descriptor(
        &self,
        joint: PhysicsJointId,
    ) -> Result<PhysicsJointDescriptor, PhysicsError> {
        self.ensure_open()?;
        Ok(self.joint(joint)?.descriptor)
    }

    pub fn apply_prephysics(
        &mut self,
        tick: u64,
        commands: Vec<PhysicsCommand>,
    ) -> Result<(), PhysicsError> {
        self.ensure_open()?;
        if tick != self.tick.checked_add(1).ok_or(PhysicsError::TickSequence)?
            || self.prepared_tick.is_some()
        {
            return Err(PhysicsError::TickSequence);
        }
        if commands.len() > self.config.max_commands_per_tick {
            return Err(PhysicsError::CommandCapacity);
        }
        let mut touched = BTreeSet::new();
        let mut staged = self.bodies.clone();
        for command in commands {
            let body = command.body();
            if !touched.insert(body) {
                return Err(PhysicsError::DuplicateBodyCommand);
            }
            let state = body_mut_from(self.world, &mut staged, body)?;
            apply_command(state, command)?;
        }
        self.bodies = staged;
        self.prepared_tick = Some(tick);
        Ok(())
    }

    pub fn step(&mut self, tick: u64) -> Result<&[ContactPair], PhysicsError> {
        self.ensure_open()?;
        if self.prepared_tick != Some(tick) {
            return Err(PhysicsError::TickSequence);
        }
        let mut staged = self.bodies.clone();
        for slot in &mut staged {
            let Some(body) = slot.state.as_mut() else {
                continue;
            };
            match body.kind {
                BodyKind::Static => {}
                BodyKind::Kinematic => {
                    if let Some(target) = body.kinematic_target.take() {
                        body.pose = target;
                    }
                }
                BodyKind::Dynamic => {
                    for axis in 0..3 {
                        body.pose[axis] = body.pose[axis]
                            .checked_add(body.velocity[axis])
                            .ok_or(PhysicsError::ArithmeticOverflow)?;
                    }
                }
            }
        }
        let mut staged_joints = self.joints.clone();
        let mut staged_joint_free = self.joint_free.clone();
        let joint_events = solve_joints(
            self.world,
            tick,
            &mut staged,
            &mut staged_joints,
            &mut staged_joint_free,
            self.config.max_joint_events_per_tick,
        )?;
        let contacts = build_contacts(
            self.world,
            tick,
            &staged,
            &self.colliders,
            &staged_joints,
            self.config.max_contacts_per_tick,
        )?;
        self.bodies = staged;
        self.joints = staged_joints;
        self.joint_free = staged_joint_free;
        self.contacts = contacts;
        self.joint_events = joint_events;
        self.tick = tick;
        self.prepared_tick = None;
        Ok(&self.contacts)
    }

    pub fn pose(&self, body: PhysicsBodyId) -> Result<[i64; 3], PhysicsError> {
        self.ensure_open()?;
        Ok(self.body(body)?.pose)
    }

    pub fn contacts(&self, tick: u64) -> Result<&[ContactPair], PhysicsError> {
        self.ensure_open()?;
        (tick == self.tick)
            .then_some(self.contacts.as_slice())
            .ok_or(PhysicsError::TickSequence)
    }

    pub fn joint_events(&self, tick: u64) -> Result<&[JointEvent], PhysicsError> {
        self.ensure_open()?;
        (tick == self.tick)
            .then_some(self.joint_events.as_slice())
            .ok_or(PhysicsError::TickSequence)
    }

    pub fn post_step(&self) -> PhysicsPostStep {
        PhysicsPostStep {
            tick: self.tick,
            poses: self
                .bodies
                .iter()
                .filter_map(|slot| slot.state.map(|body| (body.entity, body.pose)))
                .collect(),
            contacts: self.contacts.clone(),
            joint_events: self.joint_events.clone(),
        }
    }

    pub fn query_points(
        &self,
        tick: u64,
        points: &[[i64; 3]],
        layer_mask: u32,
    ) -> Result<Vec<Vec<PhysicsColliderId>>, PhysicsError> {
        self.ensure_open()?;
        if tick != self.tick {
            return Err(PhysicsError::TickSequence);
        }
        if points.len() > self.config.max_query_batch {
            return Err(PhysicsError::QueryCapacity);
        }
        if layer_mask == 0 {
            return Err(PhysicsError::InvalidFilter);
        }
        let mut result = Vec::with_capacity(points.len());
        let mut total_hits = 0_usize;
        for point in points {
            let mut hits = Vec::new();
            for (index, slot) in self.colliders.iter().enumerate() {
                let Some(collider) = slot.state else {
                    continue;
                };
                if collider.layer & layer_mask == 0 {
                    continue;
                }
                let pose = body_from(self.world, &self.bodies, collider.body)?.pose;
                if point_in_bounds(*point, pose, collider.half_extent)? {
                    total_hits = total_hits
                        .checked_add(1)
                        .filter(|hits| *hits <= self.config.max_query_hits)
                        .ok_or(PhysicsError::QueryCapacity)?;
                    hits.push(PhysicsColliderId {
                        world: self.world,
                        handle: Handle {
                            index: index as u32,
                            generation: slot.generation,
                        },
                    });
                }
            }
            result.push(hits);
        }
        Ok(result)
    }

    pub fn cast_batch(
        &self,
        tick: u64,
        queries: &[PhysicsCastQuery],
    ) -> Result<Vec<Vec<PhysicsCastHit>>, PhysicsError> {
        self.ensure_open()?;
        if tick != self.tick {
            return Err(PhysicsError::TickSequence);
        }
        if queries.len() > self.config.max_query_batch {
            return Err(PhysicsError::QueryCapacity);
        }
        let mut total_hits = 0_usize;
        let mut batches = Vec::with_capacity(queries.len());
        for query in queries {
            let (origin, delta, cast_extent, layer_mask) = match *query {
                PhysicsCastQuery::Ray {
                    origin,
                    delta,
                    layer_mask,
                } => (origin, delta, [0; 3], layer_mask),
                PhysicsCastQuery::Shape {
                    center,
                    half_extent,
                    delta,
                    layer_mask,
                } => {
                    validate_query_shape(self.space, half_extent)?;
                    (center, delta, half_extent, layer_mask)
                }
            };
            if layer_mask == 0
                || delta == [0; 3]
                || (self.space == PhysicsSpace::TwoD && delta[2] != 0)
            {
                return Err(if layer_mask == 0 {
                    PhysicsError::InvalidFilter
                } else {
                    PhysicsError::InvalidShape
                });
            }
            let mut hits = Vec::new();
            for (index, slot) in self.colliders.iter().enumerate() {
                let Some(collider) = slot.state else {
                    continue;
                };
                if collider.layer & layer_mask == 0 {
                    continue;
                }
                let expanded = [
                    collider.half_extent[0]
                        .checked_add(cast_extent[0])
                        .ok_or(PhysicsError::ArithmeticOverflow)?,
                    collider.half_extent[1]
                        .checked_add(cast_extent[1])
                        .ok_or(PhysicsError::ArithmeticOverflow)?,
                    collider.half_extent[2]
                        .checked_add(cast_extent[2])
                        .ok_or(PhysicsError::ArithmeticOverflow)?,
                ];
                let pose = body_from(self.world, &self.bodies, collider.body)?.pose;
                let Some((toi_numerator, toi_denominator)) =
                    segment_bounds_toi(origin, delta, pose, expanded)?
                else {
                    continue;
                };
                total_hits = total_hits
                    .checked_add(1)
                    .filter(|hits| *hits <= self.config.max_query_hits)
                    .ok_or(PhysicsError::QueryCapacity)?;
                hits.push(PhysicsCastHit {
                    collider: PhysicsColliderId {
                        world: self.world,
                        handle: Handle {
                            index: index as u32,
                            generation: slot.generation,
                        },
                    },
                    toi_numerator,
                    toi_denominator,
                    sensor: collider.sensor,
                });
            }
            hits.sort_by(|first, second| {
                (u128::from(first.toi_numerator) * u128::from(second.toi_denominator))
                    .cmp(&(u128::from(second.toi_numerator) * u128::from(first.toi_denominator)))
                    .then_with(|| first.collider.cmp(&second.collider))
            });
            batches.push(hits);
        }
        Ok(batches)
    }

    pub fn snapshot(&self) -> Result<PhysicsSnapshot, PhysicsError> {
        self.ensure_open()?;
        let snapshot = PhysicsSnapshot {
            schema_version: 1,
            world: self.world,
            space: self.space,
            tick: self.tick,
            bodies: self
                .bodies
                .iter()
                .map(|slot| PhysicsBodySnapshot {
                    generation: slot.generation,
                    descriptor: slot.state.map(|body| PhysicsBodyDescriptor {
                        entity: body.entity,
                        kind: body.kind,
                        pose: body.pose,
                        velocity: body.velocity,
                    }),
                })
                .collect(),
            body_free: self.body_free.clone(),
            colliders: self
                .colliders
                .iter()
                .map(|slot| PhysicsColliderSnapshot {
                    generation: slot.generation,
                    descriptor: slot.state.map(|collider| PhysicsColliderDescriptor {
                        body: collider.body,
                        half_extent: collider.half_extent,
                        layer: collider.layer,
                        mask: collider.mask,
                        sensor: collider.sensor,
                    }),
                })
                .collect(),
            collider_free: self.collider_free.clone(),
            joints: self
                .joints
                .iter()
                .map(|slot| PhysicsJointSnapshot {
                    generation: slot.generation,
                    descriptor: slot.state.map(|joint| joint.descriptor),
                })
                .collect(),
            joint_free: self.joint_free.clone(),
        };
        validate_snapshot(&snapshot, &self.config, self.world, self.space)?;
        Ok(snapshot)
    }

    pub fn restore(&mut self, snapshot: PhysicsSnapshot) -> Result<(), PhysicsError> {
        self.ensure_open()?;
        if self.bodies.iter().any(|slot| slot.state.is_some())
            || self.colliders.iter().any(|slot| slot.state.is_some())
            || self.joints.iter().any(|slot| slot.state.is_some())
            || self.tick != 0
        {
            return Err(PhysicsError::RestoreRequiresEmpty);
        }
        validate_snapshot(&snapshot, &self.config, self.world, self.space)?;
        let mut bodies = Vec::with_capacity(snapshot.bodies.len());
        let mut entities = BTreeMap::new();
        for (index, item) in snapshot.bodies.iter().enumerate() {
            let state = item.descriptor.map(|descriptor| BodyState {
                entity: descriptor.entity,
                kind: descriptor.kind,
                pose: descriptor.pose,
                velocity: descriptor.velocity,
                kinematic_target: None,
            });
            if let Some(state) = state {
                let id = PhysicsBodyId {
                    world: self.world,
                    handle: Handle {
                        index: index as u32,
                        generation: item.generation,
                    },
                };
                if entities.insert(state.entity, id).is_some() {
                    return Err(PhysicsError::SnapshotMalformed);
                }
            }
            bodies.push(BodySlot {
                generation: item.generation,
                state,
            });
        }
        self.bodies = bodies;
        self.body_free = snapshot.body_free;
        self.entities = entities;
        self.colliders = snapshot
            .colliders
            .into_iter()
            .map(|item| ColliderSlot {
                generation: item.generation,
                state: item.descriptor.map(|descriptor| ColliderState {
                    body: descriptor.body,
                    half_extent: descriptor.half_extent,
                    layer: descriptor.layer,
                    mask: descriptor.mask,
                    sensor: descriptor.sensor,
                }),
            })
            .collect();
        self.collider_free = snapshot.collider_free;
        self.joints = snapshot
            .joints
            .into_iter()
            .map(|item| JointSlot {
                generation: item.generation,
                state: item.descriptor.map(|descriptor| JointState { descriptor }),
            })
            .collect();
        self.joint_free = snapshot.joint_free;
        self.tick = snapshot.tick;
        self.prepared_tick = None;
        self.contacts.clear();
        self.joint_events.clear();
        Ok(())
    }

    pub fn shutdown(&mut self) -> PhysicsRuntimeShutdownReport {
        let before = self.owner_snapshot();
        if !self.closed {
            self.prepared_tick = None;
            self.bodies.clear();
            self.body_free.clear();
            self.entities.clear();
            self.colliders.clear();
            self.collider_free.clear();
            self.joints.clear();
            self.joint_free.clear();
            self.contacts.clear();
            self.joint_events.clear();
            self.closed = true;
        }
        PhysicsRuntimeShutdownReport {
            before,
            after: self.owner_snapshot(),
        }
    }

    fn ensure_open(&self) -> Result<(), PhysicsError> {
        if self.closed {
            Err(PhysicsError::Closed)
        } else {
            Ok(())
        }
    }

    fn body(&self, body: PhysicsBodyId) -> Result<&BodyState, PhysicsError> {
        body_from(self.world, &self.bodies, body)
    }

    fn collider(&self, collider: PhysicsColliderId) -> Result<&ColliderState, PhysicsError> {
        if collider.world != self.world {
            return Err(PhysicsError::WrongWorld);
        }
        let slot = self
            .colliders
            .get(collider.handle.index as usize)
            .ok_or(PhysicsError::InvalidCollider)?;
        if slot.generation != collider.handle.generation {
            return Err(PhysicsError::InvalidCollider);
        }
        slot.state.as_ref().ok_or(PhysicsError::InvalidCollider)
    }

    fn joint(&self, joint: PhysicsJointId) -> Result<&JointState, PhysicsError> {
        if joint.world != self.world {
            return Err(PhysicsError::WrongWorld);
        }
        let slot = self
            .joints
            .get(joint.handle.index as usize)
            .ok_or(PhysicsError::InvalidJoint)?;
        if slot.generation != joint.handle.generation {
            return Err(PhysicsError::InvalidJoint);
        }
        slot.state.as_ref().ok_or(PhysicsError::InvalidJoint)
    }
}

fn body_from(
    world: WorldId,
    bodies: &[BodySlot],
    body: PhysicsBodyId,
) -> Result<&BodyState, PhysicsError> {
    if body.world != world {
        return Err(PhysicsError::WrongWorld);
    }
    let slot = bodies
        .get(body.handle.index as usize)
        .ok_or(PhysicsError::InvalidBody)?;
    if slot.generation != body.handle.generation {
        return Err(PhysicsError::InvalidBody);
    }
    slot.state.as_ref().ok_or(PhysicsError::InvalidBody)
}

fn body_mut_from(
    world: WorldId,
    bodies: &mut [BodySlot],
    body: PhysicsBodyId,
) -> Result<&mut BodyState, PhysicsError> {
    if body.world != world {
        return Err(PhysicsError::WrongWorld);
    }
    let slot = bodies
        .get_mut(body.handle.index as usize)
        .ok_or(PhysicsError::InvalidBody)?;
    if slot.generation != body.handle.generation {
        return Err(PhysicsError::InvalidBody);
    }
    slot.state.as_mut().ok_or(PhysicsError::InvalidBody)
}

fn apply_command(state: &mut BodyState, command: PhysicsCommand) -> Result<(), PhysicsError> {
    match (state.kind, command) {
        (BodyKind::Static, PhysicsCommand::SetWorldPose { pose, .. }) => state.pose = pose,
        (BodyKind::Kinematic, PhysicsCommand::SetKinematicTarget { pose, .. }) => {
            state.kinematic_target = Some(pose);
        }
        (BodyKind::Kinematic | BodyKind::Dynamic, PhysicsCommand::Teleport { pose, .. }) => {
            state.pose = pose;
            state.kinematic_target = None;
        }
        (BodyKind::Dynamic, PhysicsCommand::AddImpulse { impulse, .. }) => {
            for axis in 0..3 {
                state.velocity[axis] = state.velocity[axis]
                    .checked_add(impulse[axis])
                    .ok_or(PhysicsError::ArithmeticOverflow)?;
            }
        }
        (BodyKind::Dynamic, PhysicsCommand::SetVelocity { velocity, .. }) => {
            state.velocity = velocity;
        }
        _ => return Err(PhysicsError::OwnerPolicy),
    }
    Ok(())
}

fn solve_joints(
    world: WorldId,
    tick: u64,
    bodies: &mut [BodySlot],
    joints: &mut [JointSlot],
    joint_free: &mut Vec<u32>,
    max_events: usize,
) -> Result<Vec<JointEvent>, PhysicsError> {
    let mut events = Vec::new();
    for (index, slot) in joints.iter_mut().enumerate() {
        let Some(joint) = slot.state else {
            continue;
        };
        let first = *body_from(world, bodies, joint.descriptor.first)?;
        let second = *body_from(world, bodies, joint.descriptor.second)?;
        let Some((second_delta, error_squared)) =
            joint_correction(joint.descriptor.kind, first.pose, second.pose)?
        else {
            continue;
        };
        let joint_id = PhysicsJointId {
            world,
            handle: Handle {
                index: index as u32,
                generation: slot.generation,
            },
        };
        if joint
            .descriptor
            .break_threshold_squared
            .is_some_and(|threshold| error_squared > u128::from(threshold))
        {
            let generation = slot
                .generation
                .checked_add(1)
                .ok_or(PhysicsError::GenerationExhausted)?;
            push_joint_event(
                &mut events,
                max_events,
                JointEvent {
                    tick,
                    joint: joint_id,
                    kind: JointEventKind::Broken,
                },
            )?;
            slot.state = None;
            slot.generation = generation;
            joint_free.push(index as u32);
            continue;
        }
        let target = match (first.kind, second.kind) {
            (_, BodyKind::Dynamic) => Some((joint.descriptor.second, second_delta)),
            (BodyKind::Dynamic, _) => Some((joint.descriptor.first, negate_delta(second_delta)?)),
            _ => None,
        };
        if let Some((body, delta)) = target {
            let state = body_mut_from(world, bodies, body)?;
            for axis in 0..3 {
                state.pose[axis] = state.pose[axis]
                    .checked_add(delta[axis])
                    .ok_or(PhysicsError::ArithmeticOverflow)?;
            }
            push_joint_event(
                &mut events,
                max_events,
                JointEvent {
                    tick,
                    joint: joint_id,
                    kind: JointEventKind::ConstraintApplied,
                },
            )?;
        }
    }
    Ok(events)
}

fn push_joint_event(
    events: &mut Vec<JointEvent>,
    max_events: usize,
    event: JointEvent,
) -> Result<(), PhysicsError> {
    if events.len() == max_events {
        return Err(PhysicsError::JointEventCapacity);
    }
    events.push(event);
    Ok(())
}

fn joint_correction(
    kind: JointKind,
    first: [i64; 3],
    second: [i64; 3],
) -> Result<Option<([i64; 3], u128)>, PhysicsError> {
    match kind {
        JointKind::Fixed { offset } => {
            let mut correction = [0_i64; 3];
            for axis in 0..3 {
                let desired = first[axis]
                    .checked_add(offset[axis])
                    .ok_or(PhysicsError::ArithmeticOverflow)?;
                correction[axis] = desired
                    .checked_sub(second[axis])
                    .ok_or(PhysicsError::ArithmeticOverflow)?;
            }
            let error = vector_squared(correction)?;
            Ok((error != 0).then_some((correction, error)))
        }
        JointKind::DistanceLimit {
            min_distance_squared,
            max_distance_squared,
        } => {
            let mut separation = [0_i64; 3];
            for axis in 0..3 {
                separation[axis] = second[axis]
                    .checked_sub(first[axis])
                    .ok_or(PhysicsError::ArithmeticOverflow)?;
            }
            let distance = vector_squared(separation)?;
            if distance >= u128::from(min_distance_squared)
                && distance <= u128::from(max_distance_squared)
            {
                return Ok(None);
            }
            let axis = (0..3)
                .max_by_key(|axis| separation[*axis].unsigned_abs())
                .unwrap_or(0);
            let sign = if separation[axis] < 0 { -1 } else { 1 };
            let mut correction = [0_i64; 3];
            let error = if distance > u128::from(max_distance_squared) {
                correction[axis] = -sign;
                distance - u128::from(max_distance_squared)
            } else {
                correction[axis] = sign;
                u128::from(min_distance_squared) - distance
            };
            Ok(Some((correction, error)))
        }
    }
}

fn vector_squared(vector: [i64; 3]) -> Result<u128, PhysicsError> {
    vector.iter().try_fold(0_u128, |total, value| {
        let magnitude = u128::from(value.unsigned_abs());
        magnitude
            .checked_mul(magnitude)
            .and_then(|square| total.checked_add(square))
            .ok_or(PhysicsError::ArithmeticOverflow)
    })
}

fn negate_delta(delta: [i64; 3]) -> Result<[i64; 3], PhysicsError> {
    Ok([
        delta[0]
            .checked_neg()
            .ok_or(PhysicsError::ArithmeticOverflow)?,
        delta[1]
            .checked_neg()
            .ok_or(PhysicsError::ArithmeticOverflow)?,
        delta[2]
            .checked_neg()
            .ok_or(PhysicsError::ArithmeticOverflow)?,
    ])
}

fn validate_collider(
    space: PhysicsSpace,
    descriptor: PhysicsColliderDescriptor,
) -> Result<(), PhysicsError> {
    if descriptor.layer == 0 || descriptor.mask == 0 {
        return Err(PhysicsError::InvalidFilter);
    }
    if descriptor.half_extent[0] == 0
        || descriptor.half_extent[1] == 0
        || (space == PhysicsSpace::ThreeD && descriptor.half_extent[2] == 0)
        || (space == PhysicsSpace::TwoD && descriptor.half_extent[2] != 0)
    {
        return Err(PhysicsError::InvalidShape);
    }
    Ok(())
}

fn validate_joint(descriptor: PhysicsJointDescriptor) -> Result<(), PhysicsError> {
    if descriptor.first == descriptor.second || descriptor.first.world != descriptor.second.world {
        return Err(PhysicsError::InvalidJoint);
    }
    if let JointKind::DistanceLimit {
        min_distance_squared,
        max_distance_squared,
    } = descriptor.kind
    {
        if min_distance_squared > max_distance_squared || max_distance_squared == 0 {
            return Err(PhysicsError::InvalidJoint);
        }
    }
    if descriptor.break_threshold_squared == Some(0) {
        return Err(PhysicsError::InvalidJoint);
    }
    Ok(())
}

fn build_contacts(
    world: WorldId,
    tick: u64,
    bodies: &[BodySlot],
    colliders: &[ColliderSlot],
    joints: &[JointSlot],
    max_contacts: usize,
) -> Result<Vec<ContactPair>, PhysicsError> {
    let live = colliders
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            slot.state.map(|state| {
                (
                    PhysicsColliderId {
                        world,
                        handle: Handle {
                            index: index as u32,
                            generation: slot.generation,
                        },
                    },
                    state,
                )
            })
        })
        .collect::<Vec<_>>();
    let mut contacts = Vec::new();
    for first_index in 0..live.len() {
        for second_index in first_index + 1..live.len() {
            let (first_id, first) = live[first_index];
            let (second_id, second) = live[second_index];
            if first.body == second.body
                || first.layer & second.mask == 0
                || second.layer & first.mask == 0
                || joint_disables_collision(joints, first.body, second.body)
            {
                continue;
            }
            let first_pose = body_from(world, bodies, first.body)?.pose;
            let second_pose = body_from(world, bodies, second.body)?.pose;
            if bounds_overlap(
                first_pose,
                first.half_extent,
                second_pose,
                second.half_extent,
            )? {
                if contacts.len() == max_contacts {
                    return Err(PhysicsError::ContactCapacity);
                }
                contacts.push(ContactPair {
                    tick,
                    first: first_id,
                    second: second_id,
                    sensor: first.sensor || second.sensor,
                });
            }
        }
    }
    Ok(contacts)
}

fn joint_disables_collision(
    joints: &[JointSlot],
    first: PhysicsBodyId,
    second: PhysicsBodyId,
) -> bool {
    joints.iter().any(|slot| {
        slot.state.is_some_and(|joint| {
            !joint.descriptor.collide_connected
                && ((joint.descriptor.first == first && joint.descriptor.second == second)
                    || (joint.descriptor.first == second && joint.descriptor.second == first))
        })
    })
}

fn point_in_bounds(
    point: [i64; 3],
    pose: [i64; 3],
    half_extent: [u32; 3],
) -> Result<bool, PhysicsError> {
    for axis in 0..3 {
        let extent = i64::from(half_extent[axis]);
        let min = pose[axis]
            .checked_sub(extent)
            .ok_or(PhysicsError::ArithmeticOverflow)?;
        let max = pose[axis]
            .checked_add(extent)
            .ok_or(PhysicsError::ArithmeticOverflow)?;
        if point[axis] < min || point[axis] > max {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_query_shape(space: PhysicsSpace, half_extent: [u32; 3]) -> Result<(), PhysicsError> {
    if half_extent[0] == 0
        || half_extent[1] == 0
        || (space == PhysicsSpace::ThreeD && half_extent[2] == 0)
        || (space == PhysicsSpace::TwoD && half_extent[2] != 0)
    {
        return Err(PhysicsError::InvalidShape);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Fraction {
    numerator: i128,
    denominator: i128,
}

impl Fraction {
    fn new(numerator: i128, denominator: i128) -> Self {
        if denominator < 0 {
            Self {
                numerator: -numerator,
                denominator: -denominator,
            }
        } else {
            Self {
                numerator,
                denominator,
            }
        }
    }

    fn compare(self, other: Self) -> Result<std::cmp::Ordering, PhysicsError> {
        Ok(self
            .numerator
            .checked_mul(other.denominator)
            .ok_or(PhysicsError::ArithmeticOverflow)?
            .cmp(
                &other
                    .numerator
                    .checked_mul(self.denominator)
                    .ok_or(PhysicsError::ArithmeticOverflow)?,
            ))
    }
}

fn segment_bounds_toi(
    origin: [i64; 3],
    delta: [i64; 3],
    pose: [i64; 3],
    half_extent: [u32; 3],
) -> Result<Option<(u64, u64)>, PhysicsError> {
    let mut entry = Fraction::new(0, 1);
    let mut exit = Fraction::new(1, 1);
    for axis in 0..3 {
        let extent = i128::from(half_extent[axis]);
        let min = i128::from(pose[axis]) - extent;
        let max = i128::from(pose[axis]) + extent;
        let origin = i128::from(origin[axis]);
        let delta = i128::from(delta[axis]);
        if delta == 0 {
            if origin < min || origin > max {
                return Ok(None);
            }
            continue;
        }
        let mut first = Fraction::new(min - origin, delta);
        let mut second = Fraction::new(max - origin, delta);
        if first.compare(second)? == std::cmp::Ordering::Greater {
            std::mem::swap(&mut first, &mut second);
        }
        if first.compare(entry)? == std::cmp::Ordering::Greater {
            entry = first;
        }
        if second.compare(exit)? == std::cmp::Ordering::Less {
            exit = second;
        }
        if entry.compare(exit)? == std::cmp::Ordering::Greater {
            return Ok(None);
        }
    }
    if entry.compare(Fraction::new(0, 1))? == std::cmp::Ordering::Less
        || entry.compare(Fraction::new(1, 1))? == std::cmp::Ordering::Greater
    {
        return Ok(None);
    }
    let numerator = u64::try_from(entry.numerator).map_err(|_| PhysicsError::ArithmeticOverflow)?;
    let denominator =
        u64::try_from(entry.denominator).map_err(|_| PhysicsError::ArithmeticOverflow)?;
    Ok(Some((numerator, denominator)))
}

fn bounds_overlap(
    first_pose: [i64; 3],
    first_extent: [u32; 3],
    second_pose: [i64; 3],
    second_extent: [u32; 3],
) -> Result<bool, PhysicsError> {
    for axis in 0..3 {
        let distance = first_pose[axis]
            .checked_sub(second_pose[axis])
            .and_then(i64::checked_abs)
            .ok_or(PhysicsError::ArithmeticOverflow)?;
        let extent = i64::from(first_extent[axis])
            .checked_add(i64::from(second_extent[axis]))
            .ok_or(PhysicsError::ArithmeticOverflow)?;
        if distance > extent {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_snapshot(
    snapshot: &PhysicsSnapshot,
    config: &PhysicsConfig,
    world: WorldId,
    space: PhysicsSpace,
) -> Result<(), PhysicsError> {
    if snapshot.schema_version != 1 || snapshot.world != world || snapshot.space != space {
        return Err(PhysicsError::SnapshotMalformed);
    }
    if snapshot.bodies.len() > config.max_bodies
        || snapshot.colliders.len() > config.max_colliders
        || snapshot.joints.len() > config.max_joints
    {
        return Err(PhysicsError::SnapshotCapacity);
    }
    let bytes = snapshot
        .bodies
        .len()
        .checked_mul(80)
        .and_then(|bytes| {
            snapshot
                .colliders
                .len()
                .checked_mul(64)
                .and_then(|v| bytes.checked_add(v))
        })
        .and_then(|bytes| {
            snapshot
                .body_free
                .len()
                .checked_mul(4)
                .and_then(|v| bytes.checked_add(v))
        })
        .and_then(|bytes| {
            snapshot
                .collider_free
                .len()
                .checked_mul(4)
                .and_then(|v| bytes.checked_add(v))
        })
        .and_then(|bytes| {
            snapshot
                .joints
                .len()
                .checked_mul(64)
                .and_then(|v| bytes.checked_add(v))
        })
        .and_then(|bytes| {
            snapshot
                .joint_free
                .len()
                .checked_mul(4)
                .and_then(|v| bytes.checked_add(v))
        })
        .filter(|bytes| *bytes <= config.max_snapshot_bytes)
        .ok_or(PhysicsError::SnapshotCapacity)?;
    let _ = bytes;
    validate_free_list(&snapshot.bodies, &snapshot.body_free)?;
    validate_free_list(&snapshot.colliders, &snapshot.collider_free)?;
    validate_free_list(&snapshot.joints, &snapshot.joint_free)?;
    for body in &snapshot.bodies {
        if body.generation == 0
            || body
                .descriptor
                .is_some_and(|descriptor| descriptor.entity.world != world)
        {
            return Err(PhysicsError::SnapshotMalformed);
        }
    }
    for collider in &snapshot.colliders {
        if collider.generation == 0 {
            return Err(PhysicsError::SnapshotMalformed);
        }
        if let Some(descriptor) = collider.descriptor {
            validate_collider(space, descriptor)?;
            let body = snapshot
                .bodies
                .get(descriptor.body.handle.index as usize)
                .ok_or(PhysicsError::SnapshotMalformed)?;
            if descriptor.body.world != world
                || body.generation != descriptor.body.handle.generation
                || body.descriptor.is_none()
            {
                return Err(PhysicsError::SnapshotMalformed);
            }
        }
    }
    for joint in &snapshot.joints {
        if joint.generation == 0 {
            return Err(PhysicsError::SnapshotMalformed);
        }
        if let Some(descriptor) = joint.descriptor {
            validate_joint(descriptor)?;
            for body_id in [descriptor.first, descriptor.second] {
                let body = snapshot
                    .bodies
                    .get(body_id.handle.index as usize)
                    .ok_or(PhysicsError::SnapshotMalformed)?;
                if body_id.world != world
                    || body.generation != body_id.handle.generation
                    || body.descriptor.is_none()
                {
                    return Err(PhysicsError::SnapshotMalformed);
                }
            }
        }
    }
    Ok(())
}

trait SnapshotSlot {
    fn generation(&self) -> u32;
    fn is_free(&self) -> bool;
}

impl SnapshotSlot for PhysicsBodySnapshot {
    fn generation(&self) -> u32 {
        self.generation
    }

    fn is_free(&self) -> bool {
        self.descriptor.is_none()
    }
}

impl SnapshotSlot for PhysicsColliderSnapshot {
    fn generation(&self) -> u32 {
        self.generation
    }

    fn is_free(&self) -> bool {
        self.descriptor.is_none()
    }
}

impl SnapshotSlot for PhysicsJointSnapshot {
    fn generation(&self) -> u32 {
        self.generation
    }

    fn is_free(&self) -> bool {
        self.descriptor.is_none()
    }
}

fn validate_free_list<T: SnapshotSlot>(slots: &[T], free: &[u32]) -> Result<(), PhysicsError> {
    let mut seen = BTreeSet::new();
    for index in free {
        let slot = slots
            .get(*index as usize)
            .ok_or(PhysicsError::SnapshotMalformed)?;
        if slot.generation() == 0 || !slot.is_free() || !seen.insert(*index) {
            return Err(PhysicsError::SnapshotMalformed);
        }
    }
    if slots
        .iter()
        .enumerate()
        .any(|(index, slot)| slot.is_free() != seen.contains(&(index as u32)))
    {
        return Err(PhysicsError::SnapshotMalformed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{AccessSet, SystemSpec};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn world(engine: u32) -> WorldId {
        WorldId {
            engine: handle(engine),
            handle: handle(10),
        }
    }

    fn entity(world: WorldId, index: u32) -> WorldEntity {
        WorldEntity {
            world,
            entity: handle(index),
        }
    }

    fn runtime(world: WorldId) -> PhysicsRuntime {
        PhysicsRuntime::new(
            world,
            PhysicsSpace::TwoD,
            PhysicsConfig {
                max_bodies: 8,
                max_colliders: 8,
                max_joints: 8,
                max_commands_per_tick: 8,
                max_contacts_per_tick: 8,
                max_joint_events_per_tick: 8,
                max_query_batch: 8,
                max_query_hits: 32,
                max_snapshot_bytes: 4096,
            },
        )
        .unwrap()
    }

    fn body_descriptor(world: WorldId, index: u32, kind: BodyKind) -> PhysicsBodyDescriptor {
        PhysicsBodyDescriptor {
            entity: entity(world, index),
            kind,
            pose: [index as i64 * 2, 0, 0],
            velocity: [0, 0, 0],
        }
    }

    fn collider(body: PhysicsBodyId, sensor: bool) -> PhysicsColliderDescriptor {
        PhysicsColliderDescriptor {
            body,
            half_extent: [2, 2, 0],
            layer: 1,
            mask: 1,
            sensor,
        }
    }

    fn fixed_joint(
        first: PhysicsBodyId,
        second: PhysicsBodyId,
        collide_connected: bool,
    ) -> PhysicsJointDescriptor {
        PhysicsJointDescriptor {
            first,
            second,
            kind: JointKind::Fixed { offset: [2, 0, 0] },
            collide_connected,
            break_threshold_squared: None,
        }
    }

    fn physics_schedule() -> Schedule {
        Schedule::configure(vec![
            SystemSpec {
                name: "physics.prepare".to_owned(),
                stage: Stage::PrePhysics,
                deterministic: true,
                access: AccessSet::default(),
                before: Vec::new(),
                after: Vec::new(),
            },
            SystemSpec {
                name: "physics.step".to_owned(),
                stage: Stage::Physics,
                deterministic: true,
                access: AccessSet::default(),
                before: Vec::new(),
                after: Vec::new(),
            },
            SystemSpec {
                name: "physics.writeback".to_owned(),
                stage: Stage::PostPhysics,
                deterministic: true,
                access: AccessSet::default(),
                before: Vec::new(),
                after: Vec::new(),
            },
        ])
        .unwrap()
    }

    #[test]
    fn owner_policy_and_duplicate_commands_fail_before_partial_prephysics_commit() {
        let world = world(1);
        let mut physics = runtime(world);
        let static_body = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let dynamic_body = physics
            .create_body(body_descriptor(world, 2, BodyKind::Dynamic))
            .unwrap();
        assert_eq!(
            physics.apply_prephysics(
                1,
                vec![
                    PhysicsCommand::SetWorldPose {
                        body: static_body,
                        pose: [9, 0, 0],
                    },
                    PhysicsCommand::SetWorldPose {
                        body: dynamic_body,
                        pose: [9, 0, 0],
                    },
                ],
            ),
            Err(PhysicsError::OwnerPolicy)
        );
        assert_eq!(physics.pose(static_body).unwrap(), [2, 0, 0]);
        assert_eq!(physics.pose(dynamic_body).unwrap(), [4, 0, 0]);
        assert_eq!(physics.tick(), 0);
    }

    #[test]
    fn fixed_step_contacts_and_queries_are_bound_to_exact_tick_snapshot() {
        let world = world(1);
        let mut physics = runtime(world);
        let first = physics
            .create_body(body_descriptor(world, 1, BodyKind::Dynamic))
            .unwrap();
        let second = physics
            .create_body(body_descriptor(world, 2, BodyKind::Static))
            .unwrap();
        let first_collider = physics.create_collider(collider(first, false)).unwrap();
        physics.create_collider(collider(second, true)).unwrap();
        physics
            .apply_prephysics(
                1,
                vec![PhysicsCommand::SetVelocity {
                    body: first,
                    velocity: [1, 0, 0],
                }],
            )
            .unwrap();
        let contacts = physics.step(1).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].first, first_collider);
        assert!(contacts[0].sensor);
        assert_eq!(physics.pose(first).unwrap(), [3, 0, 0]);
        assert_eq!(
            physics.query_points(1, &[[3, 0, 0]], 1).unwrap()[0].len(),
            2
        );
        assert_eq!(
            physics.query_points(0, &[[3, 0, 0]], 1),
            Err(PhysicsError::TickSequence)
        );
    }

    #[test]
    fn ray_and_shape_cast_batch_use_exact_toi_stable_order_and_hard_hit_budget() {
        let world = world(1);
        let mut physics = runtime(world);
        let first = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let second = physics
            .create_body(body_descriptor(world, 2, BodyKind::Static))
            .unwrap();
        let first_collider = physics.create_collider(collider(first, false)).unwrap();
        let second_collider = physics.create_collider(collider(second, true)).unwrap();
        physics.apply_prephysics(1, Vec::new()).unwrap();
        physics.step(1).unwrap();

        let queries = [
            PhysicsCastQuery::Ray {
                origin: [-10, 0, 0],
                delta: [20, 0, 0],
                layer_mask: 1,
            },
            PhysicsCastQuery::Shape {
                center: [-10, 0, 0],
                half_extent: [1, 1, 0],
                delta: [20, 0, 0],
                layer_mask: 1,
            },
        ];
        let hits = physics.cast_batch(1, &queries).unwrap();
        assert_eq!(
            hits[0].iter().map(|hit| hit.collider).collect::<Vec<_>>(),
            vec![first_collider, second_collider]
        );
        assert_eq!(
            (hits[0][0].toi_numerator, hits[0][0].toi_denominator),
            (10, 20)
        );
        assert_eq!(
            (hits[1][0].toi_numerator, hits[1][0].toi_denominator),
            (9, 20)
        );
        assert!(hits[0][1].sensor);
        assert_eq!(
            physics.cast_batch(0, &queries),
            Err(PhysicsError::TickSequence)
        );
        assert_eq!(
            physics.cast_batch(
                1,
                &[PhysicsCastQuery::Ray {
                    origin: [0, 0, 0],
                    delta: [1, 0, 1],
                    layer_mask: 1,
                }]
            ),
            Err(PhysicsError::InvalidShape)
        );

        physics.config.max_query_hits = 1;
        assert_eq!(
            physics.cast_batch(1, &queries[..1]),
            Err(PhysicsError::QueryCapacity)
        );
    }

    #[test]
    fn backend_batch_binds_actor_world_and_generation_before_tick_mutation() {
        let owner = world(1);
        let actor = handle(90);
        let mut adapter = PhysicsBackendAdapter::new(owner, actor, handle(20), 1).unwrap();
        let old_backend = adapter.handle();
        let mut physics = runtime(owner);
        let body = physics
            .create_body(body_descriptor(owner, 1, BodyKind::Dynamic))
            .unwrap();
        let command = PhysicsCommand::SetVelocity {
            body,
            velocity: [3, 0, 0],
        };
        assert_eq!(
            adapter.execute_tick(old_backend, handle(91), &mut physics, 1, vec![command]),
            Err(PhysicsError::WrongActor)
        );
        assert_eq!(physics.tick(), 0);
        assert_eq!(
            adapter.execute_tick(old_backend, actor, &mut physics, 1, vec![command, command],),
            Err(PhysicsError::BackendCapacity)
        );
        assert_eq!(physics.tick(), 0);

        let post = adapter
            .execute_tick(old_backend, actor, &mut physics, 1, vec![command])
            .unwrap();
        assert_eq!(post.tick, 1);
        assert_eq!(post.poses, vec![(entity(owner, 1), [5, 0, 0])]);
        let current_backend = adapter.restart().unwrap();
        assert_eq!(
            adapter.execute_tick(old_backend, actor, &mut physics, 2, Vec::new()),
            Err(PhysicsError::StaleBackend)
        );
        assert_eq!(physics.tick(), 1);
        adapter
            .execute_tick(current_backend, actor, &mut physics, 2, Vec::new())
            .unwrap();
        assert_eq!(physics.tick(), 2);

        let mut foreign = runtime(world(2));
        assert_eq!(
            adapter.execute_tick(current_backend, actor, &mut foreign, 1, Vec::new()),
            Err(PhysicsError::WrongWorld)
        );
        assert_eq!(foreign.tick(), 0);
    }

    #[test]
    fn schedule_binding_and_stage_adapter_enforce_order_retry_and_restart_boundary() {
        let owner = world(1);
        let actor = handle(90);
        let schedule = physics_schedule();
        assert_eq!(
            PhysicsStageBinding::bind(
                &schedule,
                "physics.step",
                "physics.prepare",
                "physics.writeback"
            ),
            Err(PhysicsError::InvalidStageBinding)
        );
        let binding = PhysicsStageBinding::bind(
            &schedule,
            "physics.prepare",
            "physics.step",
            "physics.writeback",
        )
        .unwrap();
        let backend = PhysicsBackendAdapter::new(owner, actor, handle(20), 8).unwrap();
        let mut adapter = PhysicsStageAdapter::new(backend, binding);
        assert_eq!(adapter.schedule_hash(), schedule.hash());
        let old_handle = adapter.backend_handle();
        let mut physics = runtime(owner);
        for index in 1..=3 {
            let body = physics
                .create_body(body_descriptor(owner, index, BodyKind::Static))
                .unwrap();
            physics.create_collider(collider(body, false)).unwrap();
        }
        assert_eq!(
            adapter.run_physics(old_handle, actor, &mut physics, 1),
            Err(PhysicsError::StageSequence)
        );
        assert_eq!(
            adapter.run_prephysics(old_handle, handle(91), &mut physics, 1, Vec::new()),
            Err(PhysicsError::WrongActor)
        );
        adapter
            .run_prephysics(old_handle, actor, &mut physics, 1, Vec::new())
            .unwrap();
        assert_eq!(adapter.restart_backend(), Err(PhysicsError::StageSequence));
        assert_eq!(
            adapter.run_prephysics(old_handle, actor, &mut physics, 1, Vec::new()),
            Err(PhysicsError::StageSequence)
        );
        adapter
            .run_physics(old_handle, actor, &mut physics, 1)
            .unwrap();
        assert_eq!(
            adapter.run_postphysics(old_handle, handle(91), &physics, 1),
            Err(PhysicsError::WrongActor)
        );
        let post = adapter
            .run_postphysics(old_handle, actor, &physics, 1)
            .unwrap();
        assert_eq!(post.tick, 1);

        physics.config.max_contacts_per_tick = 1;
        adapter
            .run_prephysics(old_handle, actor, &mut physics, 2, Vec::new())
            .unwrap();
        assert_eq!(
            adapter.run_physics(old_handle, actor, &mut physics, 2),
            Err(PhysicsError::ContactCapacity)
        );
        assert_eq!(physics.tick(), 1);
        physics.config.max_contacts_per_tick = 8;
        adapter
            .run_physics(old_handle, actor, &mut physics, 2)
            .unwrap();
        adapter
            .run_postphysics(old_handle, actor, &physics, 2)
            .unwrap();

        let current_handle = adapter.restart_backend().unwrap();
        assert_ne!(current_handle, old_handle);
        assert_eq!(
            adapter.run_prephysics(old_handle, actor, &mut physics, 3, Vec::new()),
            Err(PhysicsError::StaleBackend)
        );
        adapter
            .run_prephysics(current_handle, actor, &mut physics, 3, Vec::new())
            .unwrap();
        adapter
            .run_physics(current_handle, actor, &mut physics, 3)
            .unwrap();
        adapter
            .run_postphysics(current_handle, actor, &physics, 3)
            .unwrap();
    }

    #[test]
    fn contact_capacity_or_arithmetic_failure_preserves_last_good_tick() {
        let world = world(1);
        let mut physics = runtime(world);
        physics.config.max_contacts_per_tick = 1;
        for index in 1..=3 {
            let body = physics
                .create_body(body_descriptor(world, index, BodyKind::Static))
                .unwrap();
            physics.create_collider(collider(body, false)).unwrap();
        }
        physics.apply_prephysics(1, Vec::new()).unwrap();
        assert_eq!(physics.step(1), Err(PhysicsError::ContactCapacity));
        assert_eq!(physics.tick(), 0);
        assert_eq!(physics.prepared_tick, Some(1));
    }

    #[test]
    fn snapshot_restore_preserves_generation_owner_and_next_fixed_step() {
        let world = world(1);
        let mut source = runtime(world);
        let old = source
            .create_body(body_descriptor(world, 1, BodyKind::Dynamic))
            .unwrap();
        source.destroy_body(old).unwrap();
        let current = source
            .create_body(body_descriptor(world, 2, BodyKind::Dynamic))
            .unwrap();
        source.create_collider(collider(current, false)).unwrap();
        source.apply_prephysics(1, Vec::new()).unwrap();
        source.step(1).unwrap();
        let snapshot = source.snapshot().unwrap();

        let mut restored = runtime(world);
        restored.restore(snapshot).unwrap();
        assert_eq!(restored.pose(old), Err(PhysicsError::InvalidBody));
        assert_eq!(restored.pose(current), source.pose(current));
        restored.apply_prephysics(2, Vec::new()).unwrap();
        source.apply_prephysics(2, Vec::new()).unwrap();
        restored.step(2).unwrap();
        source.step(2).unwrap();
        assert_eq!(restored.pose(current), source.pose(current));
    }

    #[test]
    fn collider_generation_reuse_rejects_late_handles_and_guards_body_lifetime() {
        let world = world(1);
        let mut physics = runtime(world);
        let body = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let old = physics.create_collider(collider(body, false)).unwrap();
        assert_eq!(
            physics.destroy_body(body),
            Err(PhysicsError::BodyHasColliders)
        );
        physics.destroy_collider(old).unwrap();
        assert_eq!(
            physics.collider_body(old),
            Err(PhysicsError::InvalidCollider)
        );
        let current = physics.create_collider(collider(body, false)).unwrap();
        assert_eq!(current.handle.index, old.handle.index);
        assert_ne!(current.handle.generation, old.handle.generation);
        physics.destroy_collider(current).unwrap();
        physics.destroy_body(body).unwrap();
    }

    #[test]
    fn joint_generation_filters_contacts_and_guards_both_body_lifetimes() {
        let world = world(1);
        let mut physics = runtime(world);
        let first = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let second = physics
            .create_body(body_descriptor(world, 2, BodyKind::Static))
            .unwrap();
        let first_collider = physics.create_collider(collider(first, false)).unwrap();
        let second_collider = physics.create_collider(collider(second, false)).unwrap();
        let old = physics
            .create_joint(fixed_joint(first, second, false))
            .unwrap();
        assert_eq!(
            physics.create_joint(fixed_joint(first, first, false)),
            Err(PhysicsError::InvalidJoint)
        );
        physics.apply_prephysics(1, Vec::new()).unwrap();
        assert!(physics.step(1).unwrap().is_empty());
        physics.destroy_collider(first_collider).unwrap();
        physics.destroy_collider(second_collider).unwrap();
        assert_eq!(
            physics.destroy_body(first),
            Err(PhysicsError::BodyHasJoints)
        );
        physics.destroy_joint(old).unwrap();
        assert_eq!(
            physics.joint_descriptor(old),
            Err(PhysicsError::InvalidJoint)
        );
        let current = physics
            .create_joint(fixed_joint(first, second, true))
            .unwrap();
        assert_eq!(current.handle.index, old.handle.index);
        assert_ne!(current.handle.generation, old.handle.generation);
        let snapshot = physics.snapshot().unwrap();
        let mut restored = runtime(world);
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored.joint_descriptor(current),
            physics.joint_descriptor(current)
        );
    }

    #[test]
    fn fixed_and_distance_constraints_update_dynamic_owner_and_emit_tick_events() {
        let world = world(1);
        let mut physics = runtime(world);
        let anchor = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let fixed_body = physics
            .create_body(body_descriptor(world, 2, BodyKind::Dynamic))
            .unwrap();
        let distance_body = physics
            .create_body(body_descriptor(world, 5, BodyKind::Dynamic))
            .unwrap();
        let fixed = physics
            .create_joint(PhysicsJointDescriptor {
                first: anchor,
                second: fixed_body,
                kind: JointKind::Fixed { offset: [10, 0, 0] },
                collide_connected: false,
                break_threshold_squared: None,
            })
            .unwrap();
        let distance = physics
            .create_joint(PhysicsJointDescriptor {
                first: anchor,
                second: distance_body,
                kind: JointKind::DistanceLimit {
                    min_distance_squared: 1,
                    max_distance_squared: 16,
                },
                collide_connected: false,
                break_threshold_squared: None,
            })
            .unwrap();
        physics.apply_prephysics(1, Vec::new()).unwrap();
        physics.step(1).unwrap();
        assert_eq!(physics.pose(fixed_body).unwrap(), [12, 0, 0]);
        assert_eq!(physics.pose(distance_body).unwrap(), [9, 0, 0]);
        assert_eq!(
            physics.joint_events(1).unwrap(),
            &[
                JointEvent {
                    tick: 1,
                    joint: fixed,
                    kind: JointEventKind::ConstraintApplied,
                },
                JointEvent {
                    tick: 1,
                    joint: distance,
                    kind: JointEventKind::ConstraintApplied,
                },
            ]
        );
    }

    #[test]
    fn joint_break_and_event_capacity_are_atomic_and_generation_safe() {
        let world = world(1);
        let mut physics = runtime(world);
        let anchor = physics
            .create_body(body_descriptor(world, 1, BodyKind::Static))
            .unwrap();
        let first = physics
            .create_body(body_descriptor(world, 2, BodyKind::Dynamic))
            .unwrap();
        let second = physics
            .create_body(body_descriptor(world, 3, BodyKind::Dynamic))
            .unwrap();
        let breaking = physics
            .create_joint(PhysicsJointDescriptor {
                first: anchor,
                second: first,
                kind: JointKind::Fixed { offset: [20, 0, 0] },
                collide_connected: false,
                break_threshold_squared: Some(4),
            })
            .unwrap();
        physics.apply_prephysics(1, Vec::new()).unwrap();
        physics.step(1).unwrap();
        assert_eq!(physics.pose(first).unwrap(), [4, 0, 0]);
        assert_eq!(
            physics.joint_events(1).unwrap(),
            &[JointEvent {
                tick: 1,
                joint: breaking,
                kind: JointEventKind::Broken,
            }]
        );
        assert_eq!(
            physics.joint_descriptor(breaking),
            Err(PhysicsError::InvalidJoint)
        );

        let first_joint = physics
            .create_joint(PhysicsJointDescriptor {
                first: anchor,
                second: first,
                kind: JointKind::Fixed { offset: [10, 0, 0] },
                collide_connected: false,
                break_threshold_squared: None,
            })
            .unwrap();
        physics
            .create_joint(PhysicsJointDescriptor {
                first: anchor,
                second,
                kind: JointKind::Fixed { offset: [10, 0, 0] },
                collide_connected: false,
                break_threshold_squared: None,
            })
            .unwrap();
        assert_eq!(first_joint.handle.index, breaking.handle.index);
        assert_ne!(first_joint.handle.generation, breaking.handle.generation);
        physics.config.max_joint_events_per_tick = 1;
        let first_pose = physics.pose(first).unwrap();
        let second_pose = physics.pose(second).unwrap();
        physics.apply_prephysics(2, Vec::new()).unwrap();
        assert_eq!(physics.step(2), Err(PhysicsError::JointEventCapacity));
        assert_eq!(physics.tick(), 1);
        assert_eq!(physics.pose(first).unwrap(), first_pose);
        assert_eq!(physics.pose(second).unwrap(), second_pose);
        assert_eq!(physics.prepared_tick, Some(2));
    }

    #[test]
    fn cross_world_and_malformed_snapshot_are_atomic() {
        let owner = world(1);
        let mut source = runtime(owner);
        let body = source
            .create_body(body_descriptor(owner, 1, BodyKind::Static))
            .unwrap();
        let mut malformed = source.snapshot().unwrap();
        malformed.colliders.push(PhysicsColliderSnapshot {
            generation: 1,
            descriptor: Some(collider(body, false)),
        });
        malformed.collider_free.push(0);
        let mut target = runtime(owner);
        assert_eq!(
            target.restore(malformed),
            Err(PhysicsError::SnapshotMalformed)
        );
        assert_eq!(target.tick(), 0);
        assert_eq!(
            target.create_body(body_descriptor(world(2), 1, BodyKind::Static)),
            Err(PhysicsError::WrongWorld)
        );
    }
}
