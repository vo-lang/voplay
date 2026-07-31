use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::{EngineId, Handle};

use crate::{
    asset::{AssetRef, AssetScopeId},
    audio::AudioEvent,
    audio_control::{AudioControlBuilder, AudioControlError},
    control::{
        ControlDomain, ControlError, ControlRefAllocatorLease, ControlSnapshotState,
        ControlTxnBuilder, RealizationOutcome,
    },
    haptics::{HapticsCommand, RumbleRequest},
    logic_io::{LogicIoCommand, LogicIoConfig, LogicIoError, LogicIoOutbox, LogicIoSnapshot},
    outbox::{OutboxError, RenderOutbox, RenderOutboxConfig, RenderTransient},
    plugin::{PluginDescriptor, PluginRegistry, PluginRegistryConfig, PluginRegistryError},
    presentation_state::{
        PresentationState, PresentationStateConfig, PresentationStateError, PresentationStateOp,
        PresentationStateSnapshot,
    },
    render_control::{RenderControlBuilder, RenderControlError},
    schedule::{Schedule, ScheduleError, Stage, SystemSpec},
    simulation_state::{
        SimulationEvent, SimulationResourceOp, SimulationState, SimulationStateConfig,
        SimulationStateError, SimulationStateSnapshot,
    },
    supervisor::{
        AudioRecoveryPhase, EndpointHandle, GroupState, InstanceGroupConfig,
        InstanceGroupSupervisor, RenderRecoveryPhase, Role, SupervisorError,
    },
    surface::PresentationPulse,
    world::{
        InputFrame, SimulationClock, SimulationClockSnapshot, SimulationTick, World, WorldCommand,
        WorldConfig, WorldError, WorldId, WorldSnapshot,
    },
    RenderEvent, RenderOp,
};

pub const CONTROL_COMMITTED_EVENT_KIND: u32 = 0x5650_0104;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameEntryDescriptor {
    pub entry_factory_id: u64,
    pub code_artifact_digest: [u8; 32],
    pub entry_schema_fingerprint: [u8; 32],
    pub init_config_fingerprint: [u8; 32],
    pub required_provider_template: u32,
    pub roles: BTreeSet<Role>,
}

impl GameEntryDescriptor {
    pub fn validate(&self) -> Result<(), GameEngineError> {
        if self.entry_factory_id == 0
            || self.code_artifact_digest.iter().all(|byte| *byte == 0)
            || self.entry_schema_fingerprint.iter().all(|byte| *byte == 0)
            || self.init_config_fingerprint.iter().all(|byte| *byte == 0)
            || self.required_provider_template == 0
            || self.roles.is_empty()
            || !self.roles.contains(&Role::Logic)
        {
            return Err(GameEngineError::InvalidDescriptor);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedInitData {
    pub config_fingerprint: [u8; 32],
    pub bytes: Vec<u8>,
}

impl OwnedInitData {
    fn validate(
        &self,
        descriptor: &GameEntryDescriptor,
        max_bytes: usize,
    ) -> Result<(), GameEngineError> {
        if self.config_fingerprint != descriptor.init_config_fingerprint {
            return Err(GameEngineError::InitConfigMismatch);
        }
        if self.bytes.len() > max_bytes {
            return Err(GameEngineError::InitDataCapacity);
        }
        Ok(())
    }
}

pub trait TargetIslandFactory {
    type Game: Game;

    fn descriptor(&self) -> &GameEntryDescriptor;
    fn construct_logic(&mut self, init: OwnedInitData) -> Result<Self::Game, GameEngineError>;
}

pub struct GameConfigure {
    systems: Vec<SystemSpec>,
    plugins: PluginRegistry,
}

impl GameConfigure {
    pub fn add_system(&mut self, system: SystemSpec) {
        self.systems.push(system);
    }

    pub fn register_plugin(&mut self, plugin: PluginDescriptor) -> Result<(), GameEngineError> {
        self.plugins.register(plugin)?;
        Ok(())
    }
}

pub struct GameContext<'a> {
    engine: EngineId,
    stage: Stage,
    world: &'a World,
    world_commands: &'a mut Vec<WorldCommand>,
    world_command_capacity: usize,
    render_ops: &'a mut Vec<RenderOp>,
    render_op_capacity: usize,
    transients: &'a mut Vec<RenderTransient>,
    transient_capacity: usize,
    presentation_pulse: Option<PresentationPulse>,
    presentation_state: &'a PresentationState,
    presentation_ops: &'a mut Vec<PresentationStateOp>,
    presentation_op_capacity: usize,
    simulation_state: &'a SimulationState,
    resource_ops: &'a mut Vec<SimulationResourceOp>,
    resource_op_capacity: usize,
    emitted_events: &'a mut Vec<SimulationEvent>,
    event_capacity: usize,
    logic_io_commands: &'a mut Vec<LogicIoCommand>,
    logic_io_capacity: usize,
    control_lease: ControlRefAllocatorLease,
    render_control_revision: u64,
    audio_control_revision: u64,
    control_transactions: &'a mut Vec<ControlTxnBuilder>,
}

impl GameContext<'_> {
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    pub const fn world(&self) -> &World {
        self.world
    }

    pub const fn presentation_pulse(&self) -> Option<PresentationPulse> {
        self.presentation_pulse
    }

    pub fn presentation_state(&self) -> Result<&PresentationState, GameEngineError> {
        if !matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::PresentationStateAccessFromTick);
        }
        Ok(self.presentation_state)
    }

    pub fn emit_presentation_state(
        &mut self,
        op: PresentationStateOp,
    ) -> Result<(), GameEngineError> {
        if self.stage != Stage::Frame {
            return Err(GameEngineError::OutputStageViolation);
        }
        if self.presentation_ops.len() == self.presentation_op_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.presentation_ops.push(op);
        Ok(())
    }

    pub fn simulation_resource(&self, resource: u32) -> Option<&[u8]> {
        self.simulation_state.resource(resource)
    }

    pub fn simulation_events(&self, kind: u32) -> impl Iterator<Item = &SimulationEvent> {
        self.simulation_state.events(kind)
    }

    pub fn queue_resource_op(&mut self, op: SimulationResourceOp) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::WorldMutationFromPresentationStage);
        }
        if self.resource_ops.len() == self.resource_op_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.resource_ops.push(op);
        Ok(())
    }

    pub fn emit_simulation_event(&mut self, event: SimulationEvent) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::WorldMutationFromPresentationStage);
        }
        if self.emitted_events.len() == self.event_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.emitted_events.push(event);
        Ok(())
    }

    pub fn queue_world_command(&mut self, command: WorldCommand) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::WorldMutationFromPresentationStage);
        }
        if self.world_commands.len() == self.world_command_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.world_commands.push(command);
        Ok(())
    }

    pub fn emit_render_op(&mut self, op: RenderOp) -> Result<(), GameEngineError> {
        if self.stage != Stage::Extract {
            return Err(GameEngineError::OutputStageViolation);
        }
        if self.render_ops.len() == self.render_op_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.render_ops.push(op);
        Ok(())
    }

    pub fn emit_transient(&mut self, transient: RenderTransient) -> Result<(), GameEngineError> {
        if self.stage != Stage::Frame {
            return Err(GameEngineError::OutputStageViolation);
        }
        if self.transients.len() == self.transient_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.transients.push(transient);
        Ok(())
    }

    pub fn request_asset(
        &mut self,
        sequence: u64,
        scope: AssetScopeId,
        asset_ref: AssetRef,
        deadline_millis: u64,
    ) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::AssetRequest {
            sequence,
            scope,
            asset_ref,
            deadline_millis,
        })
    }

    pub fn submit_asset_control(
        &mut self,
        sequence: u64,
        payload: Vec<u8>,
    ) -> Result<(), GameEngineError> {
        if !matches!(self.stage, Stage::Startup | Stage::PostTick) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::AssetControl { sequence, payload })
    }

    pub fn begin_render_control(
        &mut self,
        transaction_id: u64,
    ) -> Result<RenderControlBuilder, GameEngineError> {
        if !matches!(self.stage, Stage::Startup | Stage::PostTick) {
            return Err(GameEngineError::OutputStageViolation);
        }
        Ok(RenderControlBuilder::new(
            self.engine,
            self.control_lease.begin(
                ControlDomain::Render,
                transaction_id,
                self.render_control_revision,
            ),
        )?)
    }

    pub fn begin_audio_control(
        &mut self,
        transaction_id: u64,
    ) -> Result<AudioControlBuilder, GameEngineError> {
        if !matches!(self.stage, Stage::Startup | Stage::PostTick) {
            return Err(GameEngineError::OutputStageViolation);
        }
        Ok(AudioControlBuilder::new(
            self.engine,
            self.control_lease.begin(
                ControlDomain::Audio,
                transaction_id,
                self.audio_control_revision,
            ),
        )?)
    }

    pub fn submit_control_transaction(
        &mut self,
        transaction: ControlTxnBuilder,
    ) -> Result<(), GameEngineError> {
        if !matches!(self.stage, Stage::Startup | Stage::PostTick)
            || self
                .control_transactions
                .iter()
                .any(|existing| existing.domain() == transaction.domain())
        {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.control_transactions.push(transaction);
        Ok(())
    }

    pub fn emit_render_event(&mut self, event: RenderEvent) -> Result<(), GameEngineError> {
        if !matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::RenderEvent(event))
    }

    pub fn emit_audio_event(&mut self, event: AudioEvent) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::AudioEvent(event))
    }

    pub fn request_rumble(&mut self, request: RumbleRequest) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::Haptics(HapticsCommand::Start(request)))
    }

    pub fn cancel_rumble(
        &mut self,
        request_id: u64,
        device: Handle,
    ) -> Result<(), GameEngineError> {
        if matches!(self.stage, Stage::Extract | Stage::Frame) {
            return Err(GameEngineError::OutputStageViolation);
        }
        self.queue_logic_io(LogicIoCommand::Haptics(HapticsCommand::Cancel {
            request_id,
            device,
        }))
    }

    fn queue_logic_io(&mut self, command: LogicIoCommand) -> Result<(), GameEngineError> {
        if self.logic_io_commands.len() == self.logic_io_capacity {
            return Err(GameEngineError::StageOutputCapacity);
        }
        self.logic_io_commands.push(command);
        Ok(())
    }
}

pub trait Game {
    fn configure(&self, configure: &mut GameConfigure) -> Result<(), GameEngineError>;

    fn execute(
        &mut self,
        system: &SystemSpec,
        context: &mut GameContext<'_>,
        input: Option<&InputFrame>,
    ) -> Result<(), GameEngineError>;
}

#[derive(Clone, Debug)]
pub struct GameEngineConfig {
    pub world: WorldConfig,
    pub group: InstanceGroupConfig,
    pub actors: BTreeMap<Role, Handle>,
    pub render_outbox: RenderOutboxConfig,
    pub presentation_state: PresentationStateConfig,
    pub simulation_state: SimulationStateConfig,
    pub logic_io: LogicIoConfig,
    pub max_init_bytes: usize,
    pub max_stage_world_commands: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameEngineState {
    Constructed,
    Running,
    Paused,
    Recovering,
    Closed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameEngineRecoveryCapsule {
    pub schema_version: u32,
    pub engine: EngineId,
    pub entry: GameEntryDescriptor,
    pub schedule_hash: u64,
    pub world_fingerprint: [u8; 32],
    pub clock_fingerprint: [u8; 32],
    pub presentation_fingerprint: [u8; 32],
    pub render_control: crate::control::ControlStateSnapshot,
    pub audio_control: crate::control::ControlStateSnapshot,
    pub observed_render_control_revision: u64,
    pub observed_audio_control_revision: u64,
    pub presentation_state: PresentationStateSnapshot,
    pub world: WorldSnapshot,
    pub clock: SimulationClockSnapshot,
    pub simulation_state: SimulationStateSnapshot,
    pub simulation_revision: u64,
    pub pending_endpoint_events: Vec<SimulationEvent>,
    pub logic_io: LogicIoSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GameEngineError {
    InvalidDescriptor,
    DescriptorFactoryMismatch,
    InvalidConfig,
    InvalidState,
    FactoryFailure,
    GameFailure,
    InitConfigMismatch,
    InitDataCapacity,
    InvalidPresentationPulse,
    WorldMutationFromPresentationStage,
    PresentationStateAccessFromTick,
    OutputStageViolation,
    StageOutputCapacity,
    RecoveryIdentityMismatch,
    Control(ControlError),
    Outbox(OutboxError),
    PresentationState(PresentationStateError),
    SimulationState(SimulationStateError),
    SimulationRevisionExhausted,
    LogicIo(LogicIoError),
    RenderControl(RenderControlError),
    AudioControl(AudioControlError),
    Plugin(PluginRegistryError),
    Schedule(ScheduleError),
    Supervisor(SupervisorError),
    World(WorldError),
}

impl From<ScheduleError> for GameEngineError {
    fn from(error: ScheduleError) -> Self {
        Self::Schedule(error)
    }
}

impl From<SupervisorError> for GameEngineError {
    fn from(error: SupervisorError) -> Self {
        Self::Supervisor(error)
    }
}

impl From<WorldError> for GameEngineError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

impl From<OutboxError> for GameEngineError {
    fn from(error: OutboxError) -> Self {
        Self::Outbox(error)
    }
}

impl From<PresentationStateError> for GameEngineError {
    fn from(error: PresentationStateError) -> Self {
        Self::PresentationState(error)
    }
}

impl From<SimulationStateError> for GameEngineError {
    fn from(error: SimulationStateError) -> Self {
        Self::SimulationState(error)
    }
}

impl From<ControlError> for GameEngineError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

impl From<LogicIoError> for GameEngineError {
    fn from(error: LogicIoError) -> Self {
        Self::LogicIo(error)
    }
}

impl From<RenderControlError> for GameEngineError {
    fn from(error: RenderControlError) -> Self {
        Self::RenderControl(error)
    }
}

impl From<AudioControlError> for GameEngineError {
    fn from(error: AudioControlError) -> Self {
        Self::AudioControl(error)
    }
}

impl From<PluginRegistryError> for GameEngineError {
    fn from(error: PluginRegistryError) -> Self {
        Self::Plugin(error)
    }
}

pub struct GameEngine<G: Game> {
    id: EngineId,
    entry: GameEntryDescriptor,
    state: GameEngineState,
    game: G,
    schedule: Schedule,
    plugins: PluginRegistry,
    supervisor: InstanceGroupSupervisor,
    actors: BTreeMap<Role, Handle>,
    endpoints: BTreeMap<Role, EndpointHandle>,
    world: World,
    clock: SimulationClock,
    render_outbox: RenderOutbox,
    presentation_state: PresentationState,
    presentation_state_config: PresentationStateConfig,
    simulation_state: SimulationState,
    simulation_state_config: SimulationStateConfig,
    simulation_revision: u64,
    logic_io: LogicIoOutbox,
    control_lease: Option<ControlRefAllocatorLease>,
    control_lease_capacity: u32,
    max_extract_ops: usize,
    max_frame_transients: usize,
    max_presentation_ops: usize,
    max_stage_world_commands: usize,
    max_resource_ops: usize,
    max_stage_events: usize,
    max_stage_event_bytes: usize,
    max_total_event_bytes: usize,
    max_logic_io_commands: usize,
    pending_endpoint_events: Vec<SimulationEvent>,
    pending_endpoint_event_bytes: usize,
    stage_systems: BTreeMap<Stage, Vec<SystemSpec>>,
    stage_buffers: GameStageBuffers,
}

struct GameStageBuffers {
    render_ops: Vec<RenderOp>,
    transients: Vec<RenderTransient>,
    presentation_ops: Vec<PresentationStateOp>,
    world_commands: Vec<WorldCommand>,
    resource_ops: Vec<SimulationResourceOp>,
    emitted_events: Vec<SimulationEvent>,
    logic_io_commands: Vec<LogicIoCommand>,
    control_transactions: Vec<ControlTxnBuilder>,
    peak_used_slots: usize,
}

impl GameStageBuffers {
    fn new(config: &GameEngineConfig) -> Self {
        Self {
            render_ops: Vec::with_capacity(config.render_outbox.max_pending_ops),
            transients: Vec::with_capacity(config.render_outbox.max_domains),
            presentation_ops: Vec::with_capacity(config.presentation_state.max_entries_per_domain),
            world_commands: Vec::with_capacity(config.max_stage_world_commands),
            resource_ops: Vec::with_capacity(config.simulation_state.max_resources),
            emitted_events: Vec::with_capacity(config.simulation_state.max_stage_events),
            logic_io_commands: Vec::with_capacity(config.logic_io.max_commands),
            control_transactions: Vec::with_capacity(config.group.control.max_ops_per_transaction),
            peak_used_slots: 0,
        }
    }

    fn used_slots(&self) -> usize {
        self.render_ops.len()
            + self.transients.len()
            + self.presentation_ops.len()
            + self.world_commands.len()
            + self.resource_ops.len()
            + self.emitted_events.len()
            + self.logic_io_commands.len()
            + self.control_transactions.len()
    }

    fn reserved_slots(&self) -> usize {
        self.render_ops.capacity()
            + self.transients.capacity()
            + self.presentation_ops.capacity()
            + self.world_commands.capacity()
            + self.resource_ops.capacity()
            + self.emitted_events.capacity()
            + self.logic_io_commands.capacity()
            + self.control_transactions.capacity()
    }

    fn observe_peak(&mut self) {
        self.peak_used_slots = self.peak_used_slots.max(self.used_slots());
    }

    fn clear(&mut self) {
        self.render_ops.clear();
        self.transients.clear();
        self.presentation_ops.clear();
        self.world_commands.clear();
        self.resource_ops.clear();
        self.emitted_events.clear();
        self.logic_io_commands.clear();
        self.control_transactions.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameEngineOwnerSnapshot {
    pub engine: EngineId,
    pub state: GameEngineState,
    pub live_roles: usize,
    pub live_entities: usize,
    pub simulation_revision: u64,
    pub pending_endpoint_events: usize,
    pub pending_endpoint_event_bytes: usize,
    pub pending_logic_io_commands: usize,
    pub stage_buffer_reserved_slots: usize,
    pub stage_buffer_peak_used_slots: usize,
    pub world: crate::world::WorldOwnerSnapshot,
    pub clock: crate::world::SimulationClockOwnerSnapshot,
    pub render_outbox: crate::outbox::RenderOutboxOwnerSnapshot,
    pub presentation_state: crate::presentation_state::PresentationStateOwnerSnapshot,
    pub simulation_state: crate::simulation_state::SimulationStateOwnerSnapshot,
    pub logic_io: crate::logic_io::LogicIoOwnerSnapshot,
}

impl<G: Game> GameEngine<G> {
    pub fn install<F>(
        id: EngineId,
        descriptor: GameEntryDescriptor,
        factory: &mut F,
        init: OwnedInitData,
        config: GameEngineConfig,
    ) -> Result<Self, GameEngineError>
    where
        F: TargetIslandFactory<Game = G>,
    {
        if !id.is_valid() || config.max_init_bytes == 0 || config.max_stage_world_commands == 0 {
            return Err(GameEngineError::InvalidConfig);
        }
        descriptor.validate()?;
        if factory.descriptor() != &descriptor {
            return Err(GameEngineError::DescriptorFactoryMismatch);
        }
        init.validate(&descriptor, config.max_init_bytes)?;
        if descriptor.roles != config.group.roles
            || config.actors.keys().copied().collect::<BTreeSet<_>>() != config.group.roles
            || config.actors.values().any(|actor| !actor.is_valid())
        {
            return Err(GameEngineError::InvalidConfig);
        }
        let control_lease_capacity = u32::try_from(config.group.control.max_ops_per_transaction)
            .map_err(|_| GameEngineError::InvalidConfig)?;
        let game = factory.construct_logic(init)?;
        let mut configure = GameConfigure {
            systems: Vec::new(),
            plugins: PluginRegistry::new(id, PluginRegistryConfig::default(), BTreeSet::new())?,
        };
        game.configure(&mut configure)?;
        configure.plugins.freeze()?;
        let schedule = Schedule::configure(configure.systems)?;
        let mut stage_systems = BTreeMap::<Stage, Vec<SystemSpec>>::new();
        for system in schedule.systems() {
            stage_systems
                .entry(system.stage)
                .or_default()
                .push(system.clone());
        }
        let stage_buffers = GameStageBuffers::new(&config);
        let supervisor = InstanceGroupSupervisor::new(id, config.group)?;
        let render_outbox = RenderOutbox::new(id, config.render_outbox)?;
        let presentation_state = PresentationState::new(id, config.presentation_state)?;
        let world_id = WorldId {
            engine: id,
            handle: Handle {
                index: 0,
                generation: 1,
            },
        };
        let world = World::new(world_id, config.world)?;
        let simulation_state = SimulationState::new(world_id, config.simulation_state)?;
        let logic_io = LogicIoOutbox::new(id, config.logic_io)?;
        Ok(Self {
            id,
            entry: descriptor,
            state: GameEngineState::Constructed,
            game,
            schedule,
            plugins: configure.plugins,
            supervisor,
            actors: config.actors,
            endpoints: BTreeMap::new(),
            world,
            clock: SimulationClock::default(),
            render_outbox,
            presentation_state,
            presentation_state_config: config.presentation_state,
            simulation_state,
            simulation_state_config: config.simulation_state,
            simulation_revision: 0,
            logic_io,
            control_lease: None,
            control_lease_capacity,
            max_extract_ops: config.render_outbox.max_pending_ops,
            max_frame_transients: config.render_outbox.max_domains,
            max_presentation_ops: config.presentation_state.max_entries_per_domain,
            max_stage_world_commands: config.max_stage_world_commands,
            max_resource_ops: config.simulation_state.max_resources,
            max_stage_events: config.simulation_state.max_stage_events,
            max_stage_event_bytes: config.simulation_state.max_event_bytes,
            max_total_event_bytes: config.simulation_state.max_total_event_bytes,
            max_logic_io_commands: config.logic_io.max_commands,
            pending_endpoint_events: Vec::with_capacity(config.simulation_state.max_stage_events),
            pending_endpoint_event_bytes: 0,
            stage_systems,
            stage_buffers,
        })
    }

    pub const fn plugins(&self) -> &PluginRegistry {
        &self.plugins
    }

    pub fn start(&mut self) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Constructed {
            return Err(GameEngineError::InvalidState);
        }
        let roles = self.actors.keys().copied().collect::<Vec<_>>();
        for role in roles {
            let actor = self.actors[&role];
            let endpoint = self.supervisor.start_role(role, actor)?;
            self.endpoints.insert(role, endpoint);
        }
        let logic_endpoint = self.endpoints[&Role::Logic];
        self.control_lease = Some(self.supervisor.control_store_mut().issue_lease(
            self.actors[&Role::Logic],
            logic_endpoint.generation,
            self.control_lease_capacity,
        )?);
        if let Err(error) = self.execute_stage(Stage::Startup, None, None) {
            self.state = GameEngineState::Failed;
            return Err(error);
        }
        for (role, endpoint) in &self.endpoints {
            self.supervisor.mark_ready(*endpoint, self.actors[role])?;
        }
        if self.supervisor.state() != GroupState::Running {
            self.state = GameEngineState::Failed;
            return Err(GameEngineError::InvalidState);
        }
        self.state = GameEngineState::Running;
        Ok(())
    }

    pub fn manual_step(&mut self, input: &InputFrame) -> Result<SimulationTick, GameEngineError> {
        if self.state != GameEngineState::Running {
            return Err(GameEngineError::InvalidState);
        }
        self.publish_endpoint_events()?;
        for stage in [
            Stage::PreTick,
            Stage::Input,
            Stage::Gameplay,
            Stage::PrePhysics,
            Stage::Physics,
            Stage::PostPhysics,
            Stage::PostTick,
        ] {
            if let Err(error) = self.execute_stage(stage, None, Some(input)) {
                self.state = GameEngineState::Failed;
                return Err(error);
            }
        }
        Ok(self.clock.advance_with_state(
            input,
            self.world.revision(),
            self.simulation_revision,
            self.simulation_state.deterministic_digest(),
        )?)
    }

    pub fn pause(&mut self) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Running {
            return Err(GameEngineError::InvalidState);
        }
        self.supervisor.suspend()?;
        self.state = GameEngineState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Paused {
            return Err(GameEngineError::InvalidState);
        }
        self.supervisor.resume()?;
        self.state = GameEngineState::Running;
        Ok(())
    }

    pub fn report_endpoint_fault(
        &mut self,
        role: Role,
    ) -> Result<Option<EndpointHandle>, GameEngineError> {
        self.report_endpoint_fault_with_deadline(role, 0, u64::MAX)
    }

    pub fn report_endpoint_fault_with_deadline(
        &mut self,
        role: Role,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<Option<EndpointHandle>, GameEngineError> {
        if !matches!(
            self.state,
            GameEngineState::Running | GameEngineState::Paused
        ) {
            return Err(GameEngineError::InvalidState);
        }
        let endpoint = self
            .endpoints
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let replacement = self.supervisor.report_fault_with_deadline(
            endpoint,
            actor,
            now_micros,
            deadline_micros,
        )?;
        if let Some(replacement) = replacement {
            self.endpoints.insert(role, replacement);
            if role == Role::Logic {
                self.control_lease = None;
            }
            self.state = GameEngineState::Recovering;
        } else {
            self.state = GameEngineState::Failed;
        }
        Ok(replacement)
    }

    pub fn next_endpoint_recovery_deadline_micros(&self) -> Option<u64> {
        self.supervisor.next_recovery_deadline_micros()
    }

    pub fn service_endpoint_recovery_deadline(
        &mut self,
        now_micros: u64,
    ) -> Option<EndpointHandle> {
        let expired = self.supervisor.service_recovery_deadline(now_micros)?;
        self.state = GameEngineState::Failed;
        Some(expired.endpoint)
    }

    pub fn bind_endpoint_replacement(
        &mut self,
        role: Role,
        actor: Handle,
    ) -> Result<EndpointHandle, GameEngineError> {
        if self.state != GameEngineState::Recovering || role == Role::Logic {
            return Err(GameEngineError::InvalidState);
        }
        let endpoint = self
            .endpoints
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        self.supervisor.bind_replacement(endpoint, actor)?;
        self.actors.insert(role, actor);
        Ok(endpoint)
    }

    pub fn render_recovery_phase(&self) -> Option<RenderRecoveryPhase> {
        let endpoint = self.endpoints.get(&Role::Render).copied()?;
        self.supervisor.render_recovery_phase(endpoint)
    }

    pub fn render_recovery_control_snapshot(
        &self,
    ) -> Result<crate::control::ControlStateSnapshot, GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self.supervisor.render_control_snapshot(endpoint, actor)?)
    }

    pub fn confirm_render_recovery_control(
        &mut self,
        revision: u64,
    ) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self
            .supervisor
            .confirm_render_control_snapshot(endpoint, actor, revision)?)
    }

    pub fn confirm_render_recovery_state(
        &mut self,
        render_revision: u64,
        required_control_revision: u64,
    ) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self.supervisor.confirm_render_state_snapshot(
            endpoint,
            actor,
            render_revision,
            required_control_revision,
        )?)
    }

    pub fn confirm_render_recovery_residency(
        &mut self,
        render_revision: u64,
    ) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Render)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self
            .supervisor
            .confirm_asset_residency(endpoint, actor, render_revision)?)
    }

    pub fn audio_recovery_phase(&self) -> Option<AudioRecoveryPhase> {
        let endpoint = self.endpoints.get(&Role::Audio).copied()?;
        self.supervisor.audio_recovery_phase(endpoint)
    }

    pub fn audio_recovery_control_snapshot(
        &self,
    ) -> Result<crate::control::ControlStateSnapshot, GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self.supervisor.audio_control_snapshot(endpoint, actor)?)
    }

    pub fn confirm_audio_recovery_control(&mut self, revision: u64) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self
            .supervisor
            .confirm_audio_control_snapshot(endpoint, actor, revision)?)
    }

    pub fn confirm_audio_recovery_streams(
        &mut self,
        required_control_revision: u64,
    ) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self.supervisor.confirm_audio_asset_streams(
            endpoint,
            actor,
            required_control_revision,
        )?)
    }

    pub fn confirm_audio_recovery_device(&mut self) -> Result<(), GameEngineError> {
        let endpoint = self
            .endpoints
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&Role::Audio)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        Ok(self
            .supervisor
            .confirm_audio_device_activation(endpoint, actor)?)
    }

    pub fn mark_endpoint_replacement_ready(&mut self, role: Role) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Recovering || role == Role::Logic {
            return Err(GameEngineError::InvalidState);
        }
        let endpoint = self
            .endpoints
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let actor = self
            .actors
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        self.supervisor.mark_ready(endpoint, actor)?;
        self.state = match self.supervisor.state() {
            GroupState::Running => GameEngineState::Running,
            GroupState::Suspended => GameEngineState::Paused,
            GroupState::Recovering => GameEngineState::Recovering,
            GroupState::Failed => GameEngineState::Failed,
            _ => return Err(GameEngineError::InvalidState),
        };
        Ok(())
    }

    pub fn presentation_pulse(
        &mut self,
        pulse: PresentationPulse,
        input: Option<&InputFrame>,
    ) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Running {
            return Err(GameEngineError::InvalidState);
        }
        if pulse.surface.engine != self.id || pulse.pulse_id == 0 {
            return Err(GameEngineError::InvalidPresentationPulse);
        }
        self.execute_stage(Stage::Extract, Some(pulse), input)?;
        self.execute_stage(Stage::Frame, Some(pulse), input)?;
        self.clock.notify_presentation();
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), GameEngineError> {
        if self.state == GameEngineState::Closed {
            return Ok(());
        }
        if self.state != GameEngineState::Running
            && self.state != GameEngineState::Paused
            && self.state != GameEngineState::Recovering
            && self.state != GameEngineState::Failed
        {
            return Err(GameEngineError::InvalidState);
        }
        let shutdown_result = self.execute_stage(Stage::Shutdown, None, None);
        self.close_owned_state()?;
        shutdown_result
    }

    pub fn abandon(&mut self) -> Result<(), GameEngineError> {
        if self.state == GameEngineState::Closed {
            return Ok(());
        }
        self.close_owned_state()
    }

    fn close_owned_state(&mut self) -> Result<(), GameEngineError> {
        self.supervisor.begin_close()?;
        for role in Role::shutdown_order() {
            let Some(endpoint) = self.endpoints.get(&role).copied() else {
                continue;
            };
            self.supervisor.mark_closed(endpoint, self.actors[&role])?;
        }
        if self.supervisor.state() != GroupState::Closed {
            self.state = GameEngineState::Failed;
            return Err(GameEngineError::InvalidState);
        }
        self.supervisor.release_closed_owned_state()?;
        self.endpoints.clear();
        self.actors.clear();
        self.control_lease = None;
        self.world.shutdown();
        self.clock.shutdown();
        self.render_outbox.shutdown();
        self.presentation_state.shutdown();
        self.simulation_state.shutdown();
        self.logic_io.shutdown();
        self.plugins.shutdown();
        self.pending_endpoint_events = Vec::new();
        self.pending_endpoint_event_bytes = 0;
        self.state = GameEngineState::Closed;
        Ok(())
    }

    pub const fn id(&self) -> EngineId {
        self.id
    }

    pub const fn state(&self) -> GameEngineState {
        self.state
    }

    pub fn owner_snapshot(&self) -> GameEngineOwnerSnapshot {
        GameEngineOwnerSnapshot {
            engine: self.id,
            state: self.state,
            live_roles: self.endpoints.len(),
            live_entities: self.world.live_entities(),
            simulation_revision: self.simulation_revision,
            pending_endpoint_events: self.pending_endpoint_events.len(),
            pending_endpoint_event_bytes: self.pending_endpoint_event_bytes,
            pending_logic_io_commands: self.logic_io.commands().len(),
            stage_buffer_reserved_slots: self.stage_buffers.reserved_slots(),
            stage_buffer_peak_used_slots: self.stage_buffers.peak_used_slots,
            world: self.world.owner_snapshot(),
            clock: self.clock.owner_snapshot(),
            render_outbox: self.render_outbox.owner_snapshot(),
            presentation_state: self.presentation_state.owner_snapshot(),
            simulation_state: self.simulation_state.owner_snapshot(),
            logic_io: self.logic_io.owner_snapshot(),
        }
    }

    pub const fn deterministic_hash(&self) -> u64 {
        self.clock.deterministic_hash()
    }

    pub fn recovery_capsule(&self) -> Result<GameEngineRecoveryCapsule, GameEngineError> {
        if matches!(
            self.state,
            GameEngineState::Closed | GameEngineState::Failed
        ) {
            return Err(GameEngineError::InvalidState);
        }
        Ok(GameEngineRecoveryCapsule {
            schema_version: 4,
            engine: self.id,
            entry: self.entry.clone(),
            schedule_hash: self.schedule.hash(),
            world_fingerprint: self.entry.entry_schema_fingerprint,
            clock_fingerprint: game_recovery_fingerprint(b"voplay.clock.v1", &[]),
            presentation_fingerprint: presentation_recovery_fingerprint(
                self.presentation_state_config,
            ),
            render_control: self
                .supervisor
                .control_store()
                .snapshot(ControlDomain::Render)?,
            audio_control: self
                .supervisor
                .control_store()
                .snapshot(ControlDomain::Audio)?,
            observed_render_control_revision: self.supervisor.control_store().render_revision(),
            observed_audio_control_revision: self.supervisor.control_store().audio_revision(),
            presentation_state: self.presentation_state.snapshot(),
            world: self.world.snapshot()?,
            clock: self.clock.snapshot(),
            simulation_state: self.simulation_state.snapshot(),
            simulation_revision: self.simulation_revision,
            pending_endpoint_events: self.pending_endpoint_events.clone(),
            logic_io: self.logic_io.snapshot(),
        })
    }

    pub fn restore_recovery_capsule(
        &mut self,
        capsule: GameEngineRecoveryCapsule,
    ) -> Result<(), GameEngineError> {
        if self.state != GameEngineState::Constructed {
            return Err(GameEngineError::InvalidState);
        }
        if capsule.schema_version != 4
            || capsule.engine != self.id
            || capsule.entry != self.entry
            || capsule.schedule_hash != self.schedule.hash()
            || capsule.world_fingerprint != self.entry.entry_schema_fingerprint
            || capsule.clock_fingerprint != game_recovery_fingerprint(b"voplay.clock.v1", &[])
            || capsule.presentation_fingerprint
                != presentation_recovery_fingerprint(self.presentation_state_config)
            || capsule.render_control.engine != self.id
            || capsule.audio_control.engine != self.id
            || capsule.render_control.domain != ControlDomain::Render
            || capsule.audio_control.domain != ControlDomain::Audio
            || capsule.observed_render_control_revision != capsule.render_control.revision
            || capsule.observed_audio_control_revision != capsule.audio_control.revision
            || capsule.render_control.revision == 0
                && (capsule.render_control.last_transaction_id != 0
                    || !capsule.render_control.entries.is_empty())
            || capsule.audio_control.revision == 0
                && (capsule.audio_control.last_transaction_id != 0
                    || !capsule.audio_control.entries.is_empty())
            || capsule.render_control.revision != 0
                && capsule.render_control.last_transaction_id == 0
            || capsule.audio_control.revision != 0 && capsule.audio_control.last_transaction_id == 0
            || capsule.presentation_state.engine != self.id
            || capsule.simulation_revision < capsule.world.revision
            || capsule.simulation_revision < capsule.simulation_state.revision
        {
            return Err(GameEngineError::RecoveryIdentityMismatch);
        }
        let mut restored_control = self.supervisor.control_store().clone();
        if capsule.render_control.revision != 0 {
            restored_control
                .adopt_snapshot(&capsule.render_control)
                .map_err(|_| GameEngineError::RecoveryIdentityMismatch)?;
        }
        if capsule.audio_control.revision != 0 {
            restored_control
                .adopt_snapshot(&capsule.audio_control)
                .map_err(|_| GameEngineError::RecoveryIdentityMismatch)?;
        }
        let clock = SimulationClock::restore(capsule.clock)?;
        let logic_io_commit = self
            .logic_io
            .prepare_restore(capsule.logic_io)
            .map_err(|_| GameEngineError::RecoveryIdentityMismatch)?;
        let pending_endpoint_event_bytes =
            capsule
                .pending_endpoint_events
                .iter()
                .try_fold(0_usize, |total, event| {
                    if event.kind == 0 || event.bytes.len() > self.max_stage_event_bytes {
                        return None;
                    }
                    total.checked_add(event.bytes.len())
                });
        if capsule.pending_endpoint_events.len() > self.max_stage_events
            || pending_endpoint_event_bytes.is_none_or(|bytes| bytes > self.max_total_event_bytes)
        {
            return Err(GameEngineError::RecoveryIdentityMismatch);
        }
        let simulation_state = SimulationState::restore(
            capsule.simulation_state,
            self.world.id(),
            self.simulation_state_config,
        )?;
        let mut presentation_state =
            PresentationState::new(self.id, self.presentation_state_config)?;
        presentation_state
            .restore(capsule.presentation_state)
            .map_err(|_| GameEngineError::RecoveryIdentityMismatch)?;
        self.world.restore(capsule.world)?;
        self.clock = clock;
        self.simulation_state = simulation_state;
        self.presentation_state = presentation_state;
        self.simulation_revision = capsule.simulation_revision;
        self.pending_endpoint_event_bytes = pending_endpoint_event_bytes.unwrap();
        self.pending_endpoint_events = capsule.pending_endpoint_events;
        self.logic_io.commit(logic_io_commit)?;
        *self.supervisor.control_store_mut() = restored_control;
        self.control_lease = None;
        Ok(())
    }

    pub const fn world(&self) -> &World {
        &self.world
    }

    #[cfg(feature = "inspection")]
    pub(crate) fn world_mut_for_inspection(&mut self) -> &mut World {
        &mut self.world
    }

    pub const fn schedule_hash(&self) -> u64 {
        self.schedule.hash()
    }

    pub const fn simulation_revision(&self) -> u64 {
        self.simulation_revision
    }

    pub fn render_control_revision(&self) -> u64 {
        self.supervisor.control_store().render_revision()
    }

    pub fn audio_control_revision(&self) -> u64 {
        self.supervisor.control_store().audio_revision()
    }

    pub const fn render_outbox(&self) -> &RenderOutbox {
        &self.render_outbox
    }

    pub const fn presentation_state(&self) -> &PresentationState {
        &self.presentation_state
    }

    pub const fn simulation_state(&self) -> &SimulationState {
        &self.simulation_state
    }

    pub(crate) fn render_outbox_mut(&mut self) -> &mut RenderOutbox {
        &mut self.render_outbox
    }

    pub fn logic_io_commands(&self) -> &std::collections::VecDeque<LogicIoCommand> {
        self.logic_io.commands()
    }

    pub fn consume_logic_io(&mut self, count: usize) -> Result<(), GameEngineError> {
        Ok(self.logic_io.consume(count)?)
    }

    pub fn deliver_endpoint_event(
        &mut self,
        event: SimulationEvent,
    ) -> Result<(), GameEngineError> {
        if event.kind == 0
            || event.bytes.len() > self.max_stage_event_bytes
            || self.pending_endpoint_events.len() == self.max_stage_events
        {
            return Err(GameEngineError::StageOutputCapacity);
        }
        let bytes = self
            .pending_endpoint_event_bytes
            .checked_add(event.bytes.len())
            .filter(|bytes| *bytes <= self.max_total_event_bytes)
            .ok_or(GameEngineError::StageOutputCapacity)?;
        self.pending_endpoint_events.push(event);
        self.pending_endpoint_event_bytes = bytes;
        Ok(())
    }

    pub fn acknowledge_control(
        &mut self,
        domain: ControlDomain,
        revision: u64,
    ) -> Result<(), GameEngineError> {
        let role = match domain {
            ControlDomain::Render => Role::Render,
            ControlDomain::Audio => Role::Audio,
        };
        let endpoint = self
            .endpoints
            .get(&role)
            .copied()
            .ok_or(GameEngineError::InvalidState)?;
        let snapshot = self.supervisor.control_store().snapshot(domain)?;
        if revision == 0 || snapshot.revision != revision {
            return Err(GameEngineError::InvalidState);
        }
        for entry in snapshot.entries {
            if matches!(entry.state, ControlSnapshotState::Desired) {
                self.supervisor.control_store_mut().record_realization(
                    entry.stable,
                    endpoint.generation,
                    RealizationOutcome::Live,
                )?;
            }
        }
        Ok(())
    }

    fn publish_endpoint_events(&mut self) -> Result<(), GameEngineError> {
        if self.pending_endpoint_events.is_empty() {
            return Ok(());
        }
        let next_revision = self
            .simulation_revision
            .checked_add(1)
            .ok_or(GameEngineError::SimulationRevisionExhausted)?;
        self.simulation_state
            .append_events_reusing(&mut self.pending_endpoint_events)?;
        self.simulation_revision = next_revision;
        self.pending_endpoint_event_bytes = 0;
        Ok(())
    }

    fn execute_stage(
        &mut self,
        stage: Stage,
        pulse: Option<PresentationPulse>,
        input: Option<&InputFrame>,
    ) -> Result<(), GameEngineError> {
        let systems = self
            .stage_systems
            .get(&stage)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        self.stage_buffers.clear();
        let buffers = &mut self.stage_buffers;
        let control_lease = self.control_lease.ok_or(GameEngineError::InvalidState)?;
        let render_control_revision = self.supervisor.control_store().render_revision();
        let audio_control_revision = self.supervisor.control_store().audio_revision();
        for system in systems {
            let mut context = GameContext {
                engine: self.id,
                stage,
                world: &self.world,
                world_commands: &mut buffers.world_commands,
                world_command_capacity: self.max_stage_world_commands,
                render_ops: &mut buffers.render_ops,
                render_op_capacity: self.max_extract_ops,
                transients: &mut buffers.transients,
                transient_capacity: self.max_frame_transients,
                presentation_pulse: pulse,
                presentation_state: &self.presentation_state,
                presentation_ops: &mut buffers.presentation_ops,
                presentation_op_capacity: self.max_presentation_ops,
                simulation_state: &self.simulation_state,
                resource_ops: &mut buffers.resource_ops,
                resource_op_capacity: self.max_resource_ops,
                emitted_events: &mut buffers.emitted_events,
                event_capacity: self.max_stage_events,
                logic_io_commands: &mut buffers.logic_io_commands,
                logic_io_capacity: self.max_logic_io_commands,
                control_lease,
                render_control_revision,
                audio_control_revision,
                control_transactions: &mut buffers.control_transactions,
            };
            self.game.execute(system, &mut context, input)?;
        }
        buffers.observe_peak();
        let mut staged_control = self.supervisor.control_store().clone();
        let logic_generation = self.endpoints[&Role::Logic].generation;
        for transaction in buffers.control_transactions.drain(..) {
            let domain = transaction.domain();
            let committed = staged_control
                .commit(transaction)?
                .publish_at_safe_point(logic_generation)
                .map_err(|(error, _)| error)?;
            buffers
                .emitted_events
                .push(control_committed_event(domain, &committed));
            let snapshot = staged_control.snapshot(domain)?;
            buffers.logic_io_commands.push(match domain {
                ControlDomain::Render => LogicIoCommand::RenderControl(snapshot),
                ControlDomain::Audio => LogicIoCommand::AudioControl(snapshot),
            });
        }
        let logic_io_commit = self
            .logic_io
            .prepare_reusing(&mut buffers.logic_io_commands)?;
        match stage {
            Stage::Extract => self.render_outbox.enqueue_reusing(
                self.simulation_revision,
                self.supervisor.control_store().render_revision(),
                &mut buffers.render_ops,
            )?,
            Stage::Frame => {
                let pulse = pulse.ok_or(GameEngineError::InvalidPresentationPulse)?;
                let presentation_commit = self.presentation_state.prepare_frame_reusing(
                    pulse.surface.domain,
                    pulse.pulse_id,
                    &mut buffers.presentation_ops,
                )?;
                self.render_outbox
                    .stage_transients_reusing(&mut buffers.transients)?;
                self.presentation_state.commit(presentation_commit)?;
            }
            Stage::Startup
            | Stage::PreTick
            | Stage::Input
            | Stage::Gameplay
            | Stage::PrePhysics
            | Stage::Physics
            | Stage::PostPhysics
            | Stage::PostTick
            | Stage::Shutdown => {
                let simulation_commit = self.simulation_state.prepare_stage_reusing(
                    &mut buffers.resource_ops,
                    &mut buffers.emitted_events,
                )?;
                let advances_simulation = !buffers.world_commands.is_empty()
                    || simulation_commit.revision() != self.simulation_state.revision();
                let next_simulation_revision = if advances_simulation {
                    self.simulation_revision
                        .checked_add(1)
                        .ok_or(GameEngineError::SimulationRevisionExhausted)?
                } else {
                    self.simulation_revision
                };
                self.world
                    .apply_stage_reusing(&mut buffers.world_commands)?;
                self.simulation_state
                    .commit_reusing(simulation_commit, &mut buffers.emitted_events)?;
                self.simulation_revision = next_simulation_revision;
            }
        }
        *self.supervisor.control_store_mut() = staged_control;
        self.logic_io.commit(logic_io_commit)?;
        Ok(())
    }
}

fn control_committed_event(
    domain: ControlDomain,
    committed: &crate::control::ControlCommitted,
) -> SimulationEvent {
    let mut bytes = Vec::with_capacity(21 + committed.promotions.len() * 13);
    bytes.push(match domain {
        ControlDomain::Render => 1,
        ControlDomain::Audio => 2,
    });
    bytes.extend_from_slice(&committed.transaction_id.to_le_bytes());
    bytes.extend_from_slice(&committed.control_revision.to_le_bytes());
    bytes.extend_from_slice(&(committed.promotions.len() as u32).to_le_bytes());
    for (token, stable) in &committed.promotions {
        bytes.extend_from_slice(&token.to_le_bytes());
        bytes.push(match stable.kind {
            crate::control::ControlKind::RenderView => 1,
            crate::control::ControlKind::RenderTarget => 2,
            crate::control::ControlKind::AudioBus => 3,
            crate::control::ControlKind::PersistentAudioSource => 4,
            crate::control::ControlKind::RenderGraph => 5,
        });
        bytes.extend_from_slice(&stable.handle.index.to_le_bytes());
        bytes.extend_from_slice(&stable.handle.generation.to_le_bytes());
    }
    SimulationEvent {
        kind: CONTROL_COMMITTED_EVENT_KIND,
        bytes,
    }
}

fn presentation_recovery_fingerprint(config: PresentationStateConfig) -> [u8; 32] {
    game_recovery_fingerprint(
        b"voplay.presentation-state.v1",
        &[
            config.max_domains as u64,
            config.max_entries_per_domain as u64,
            config.max_value_bytes as u64,
            config.max_total_bytes as u64,
        ],
    )
}

fn game_recovery_fingerprint(label: &[u8], values: &[u64]) -> [u8; 32] {
    let mut output = [0_u8; 32];
    for lane in 0..4 {
        let mut hash =
            0xcbf2_9ce4_8422_2325_u64 ^ (lane as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for byte in label
            .iter()
            .copied()
            .chain(values.iter().flat_map(|value| value.to_le_bytes()))
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        output[lane * 8..lane * 8 + 8].copy_from_slice(&hash.to_le_bytes());
    }
    output
}

pub fn run_headless<F>(
    id: EngineId,
    factory: &mut F,
    init: OwnedInitData,
    config: GameEngineConfig,
    inputs: impl IntoIterator<Item = InputFrame>,
) -> Result<u64, GameEngineError>
where
    F: TargetIslandFactory,
{
    let descriptor = factory.descriptor().clone();
    let mut engine = GameEngine::install(id, descriptor, factory, init, config)?;
    engine.start()?;
    for input in inputs {
        engine.manual_step(&input)?;
    }
    let hash = engine.deterministic_hash();
    engine.shutdown()?;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;
    use crate::{
        control::EngineControlConfig,
        outbox::PresentationDomainId,
        schedule::{AccessSet, Stage},
        surface::{GameSurfaceId, SurfaceMetrics},
        world::WorldCommand,
    };
    use vo_app_protocol::SurfaceHandle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    struct CounterGame;

    impl Game for CounterGame {
        fn configure(&self, configure: &mut GameConfigure) -> Result<(), GameEngineError> {
            for (name, stage) in [
                ("start", Stage::Startup),
                ("tick", Stage::Gameplay),
                ("frame", Stage::Frame),
                ("shutdown", Stage::Shutdown),
            ] {
                configure.add_system(SystemSpec {
                    name: name.to_owned(),
                    stage,
                    deterministic: stage == Stage::Gameplay,
                    access: AccessSet::default(),
                    before: Vec::new(),
                    after: Vec::new(),
                });
            }
            Ok(())
        }

        fn execute(
            &mut self,
            system: &SystemSpec,
            context: &mut GameContext<'_>,
            input: Option<&InputFrame>,
        ) -> Result<(), GameEngineError> {
            if system.name == "tick" {
                let input = input.ok_or(GameEngineError::GameFailure)?;
                if context.world().live_entities() == 0 {
                    context.queue_world_command(WorldCommand::Spawn {
                        stable_key: 1,
                        components: BTreeMap::from([(1, input.bytes.clone())]),
                    })?;
                } else {
                    let entity = context.world().entity_for_stable_key(1).unwrap();
                    context.queue_world_command(WorldCommand::SetComponent {
                        entity,
                        component: 1,
                        value: input.bytes.clone(),
                    })?;
                }
            }
            Ok(())
        }
    }

    struct Factory {
        descriptor: GameEntryDescriptor,
        constructions: usize,
    }

    struct ShutdownTrackingGame {
        calls: Rc<Cell<u32>>,
    }

    impl Game for ShutdownTrackingGame {
        fn configure(&self, configure: &mut GameConfigure) -> Result<(), GameEngineError> {
            configure.add_system(SystemSpec {
                name: "tracked-shutdown".to_owned(),
                stage: Stage::Shutdown,
                deterministic: false,
                access: AccessSet::default(),
                before: Vec::new(),
                after: Vec::new(),
            });
            Ok(())
        }

        fn execute(
            &mut self,
            system: &SystemSpec,
            _context: &mut GameContext<'_>,
            _input: Option<&InputFrame>,
        ) -> Result<(), GameEngineError> {
            if system.name == "tracked-shutdown" {
                self.calls.set(self.calls.get() + 1);
            }
            Ok(())
        }
    }

    struct ShutdownTrackingFactory {
        descriptor: GameEntryDescriptor,
        calls: Rc<Cell<u32>>,
    }

    impl TargetIslandFactory for ShutdownTrackingFactory {
        type Game = ShutdownTrackingGame;

        fn descriptor(&self) -> &GameEntryDescriptor {
            &self.descriptor
        }

        fn construct_logic(&mut self, _init: OwnedInitData) -> Result<Self::Game, GameEngineError> {
            Ok(ShutdownTrackingGame {
                calls: Rc::clone(&self.calls),
            })
        }
    }

    impl TargetIslandFactory for Factory {
        type Game = CounterGame;

        fn descriptor(&self) -> &GameEntryDescriptor {
            &self.descriptor
        }

        fn construct_logic(&mut self, init: OwnedInitData) -> Result<Self::Game, GameEngineError> {
            if init.bytes != b"counter" {
                return Err(GameEngineError::FactoryFailure);
            }
            self.constructions += 1;
            Ok(CounterGame)
        }
    }

    fn config() -> GameEngineConfig {
        let roles = BTreeSet::from([Role::Logic]);
        GameEngineConfig {
            world: WorldConfig {
                max_entities: 4,
                max_components_per_entity: 4,
                max_component_bytes: 8,
                max_snapshot_bytes: 64,
            },
            group: InstanceGroupConfig {
                roles: roles.clone(),
                allow_logic_recovery: false,
                control: EngineControlConfig::default(),
            },
            actors: BTreeMap::from([(Role::Logic, handle(10))]),
            render_outbox: RenderOutboxConfig::default(),
            presentation_state: PresentationStateConfig::default(),
            simulation_state: SimulationStateConfig::default(),
            logic_io: LogicIoConfig::default(),
            max_init_bytes: 64,
            max_stage_world_commands: 64,
        }
    }

    fn factory() -> Factory {
        factory_with_roles(BTreeSet::from([Role::Logic]))
    }

    fn factory_with_roles(roles: BTreeSet<Role>) -> Factory {
        Factory {
            descriptor: GameEntryDescriptor {
                entry_factory_id: 7,
                code_artifact_digest: [1; 32],
                entry_schema_fingerprint: [2; 32],
                init_config_fingerprint: [3; 32],
                required_provider_template: 11,
                roles,
            },
            constructions: 0,
        }
    }

    fn full_owner_graph_config() -> GameEngineConfig {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let mut config = config();
        config.group.roles = roles;
        config.actors = BTreeMap::from([
            (Role::Logic, handle(10)),
            (Role::Asset, handle(20)),
            (Role::Render, handle(30)),
            (Role::Audio, handle(40)),
        ]);
        config
    }

    fn init() -> OwnedInitData {
        OwnedInitData {
            config_fingerprint: [3; 32],
            bytes: b"counter".to_vec(),
        }
    }

    fn inputs() -> Vec<InputFrame> {
        (1..=10)
            .map(|tick_id| InputFrame {
                tick_id,
                bytes: vec![tick_id as u8],
            })
            .collect()
    }

    #[test]
    fn game_stage_buffers_are_reserved_once_and_reused_across_ticks() {
        let id = EngineId {
            index: 51,
            generation: 1,
        };
        let mut factory = factory();
        let mut engine = GameEngine::install(
            id,
            factory.descriptor.clone(),
            &mut factory,
            init(),
            config(),
        )
        .expect("install");
        let capacities = (
            engine.stage_buffers.render_ops.capacity(),
            engine.stage_buffers.transients.capacity(),
            engine.stage_buffers.presentation_ops.capacity(),
            engine.stage_buffers.world_commands.capacity(),
            engine.stage_buffers.resource_ops.capacity(),
            engine.stage_buffers.emitted_events.capacity(),
            engine.stage_buffers.logic_io_commands.capacity(),
            engine.stage_buffers.control_transactions.capacity(),
        );
        assert_eq!(
            engine.stage_systems.values().map(Vec::len).sum::<usize>(),
            engine.schedule.systems().len()
        );
        assert_eq!(
            engine.owner_snapshot().stage_buffer_reserved_slots,
            capacities.0
                + capacities.1
                + capacities.2
                + capacities.3
                + capacities.4
                + capacities.5
                + capacities.6
                + capacities.7
        );

        engine.start().expect("start");
        for input in inputs() {
            engine.manual_step(&input).expect("tick");
        }

        assert_eq!(
            (
                engine.stage_buffers.render_ops.capacity(),
                engine.stage_buffers.transients.capacity(),
                engine.stage_buffers.presentation_ops.capacity(),
                engine.stage_buffers.world_commands.capacity(),
                engine.stage_buffers.resource_ops.capacity(),
                engine.stage_buffers.emitted_events.capacity(),
                engine.stage_buffers.logic_io_commands.capacity(),
                engine.stage_buffers.control_transactions.capacity(),
            ),
            capacities
        );
        assert!(engine.owner_snapshot().stage_buffer_peak_used_slots > 0);
    }

    fn pulse(pulse_id: u64) -> PresentationPulse {
        PresentationPulse {
            surface: GameSurfaceId {
                engine: handle(1),
                surface: SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                domain: PresentationDomainId {
                    engine: handle(1),
                    handle: handle(1),
                },
            },
            pulse_id,
            deadline_micros: pulse_id * 1_000,
            metrics: SurfaceMetrics {
                width: 640,
                height: 480,
                scale_numerator: 1,
                scale_denominator: 1,
            },
        }
    }

    #[test]
    fn headless_and_embedded_manual_step_share_lifecycle_and_hash() {
        let mut headless_factory = factory();
        let headless_hash =
            run_headless(handle(1), &mut headless_factory, init(), config(), inputs()).unwrap();
        assert_eq!(headless_factory.constructions, 1);

        let mut embedded_factory = factory();
        let mut embedded = GameEngine::install(
            handle(1),
            embedded_factory.descriptor().clone(),
            &mut embedded_factory,
            init(),
            config(),
        )
        .unwrap();
        assert_eq!(embedded.state(), GameEngineState::Constructed);
        embedded.start().unwrap();
        for (index, input) in inputs().into_iter().enumerate() {
            embedded.manual_step(&input).unwrap();
            embedded
                .presentation_pulse(pulse(index as u64 + 1), Some(&input))
                .unwrap();
        }
        assert_eq!(embedded.deterministic_hash(), headless_hash);
        embedded.pause().unwrap();
        assert_eq!(embedded.state(), GameEngineState::Paused);
        assert_eq!(
            embedded.manual_step(&inputs()[0]),
            Err(GameEngineError::InvalidState)
        );
        embedded.resume().unwrap();
        embedded.shutdown().unwrap();
        assert_eq!(embedded.state(), GameEngineState::Closed);
        assert_eq!(embedded_factory.constructions, 1);
    }

    #[test]
    fn descriptor_role_and_actor_set_are_exact_before_game_construction() {
        let mut factory = factory();
        let mut invalid = config();
        invalid.actors.insert(Role::Render, handle(20));
        assert!(matches!(
            GameEngine::install(
                handle(1),
                factory.descriptor().clone(),
                &mut factory,
                init(),
                invalid,
            ),
            Err(GameEngineError::InvalidConfig)
        ));
        assert_eq!(factory.constructions, 0);
    }

    #[test]
    fn owned_init_is_validated_before_factory_construction() {
        let mut factory = factory();
        let mut mismatched = init();
        mismatched.config_fingerprint = [9; 32];
        assert!(matches!(
            GameEngine::install(
                handle(1),
                factory.descriptor().clone(),
                &mut factory,
                mismatched,
                config(),
            ),
            Err(GameEngineError::InitConfigMismatch)
        ));
        assert_eq!(factory.constructions, 0);

        let mut oversized = init();
        oversized.bytes.resize(65, 0);
        assert!(matches!(
            GameEngine::install(
                handle(1),
                factory.descriptor().clone(),
                &mut factory,
                oversized,
                config(),
            ),
            Err(GameEngineError::InitDataCapacity)
        ));
        assert_eq!(factory.constructions, 0);
    }

    #[test]
    fn generated_descriptor_must_match_target_island_factory() {
        let mut factory = factory();
        let mut forged = factory.descriptor().clone();
        forged.entry_factory_id += 1;
        assert!(matches!(
            GameEngine::install(handle(1), forged, &mut factory, init(), config()),
            Err(GameEngineError::DescriptorFactoryMismatch)
        ));
        assert_eq!(factory.constructions, 0);
    }

    #[test]
    fn abandon_releases_owners_without_executing_game_shutdown_hook() {
        let abandon_calls = Rc::new(Cell::new(0));
        let mut abandon_factory = ShutdownTrackingFactory {
            descriptor: factory().descriptor,
            calls: Rc::clone(&abandon_calls),
        };
        let mut abandoned = GameEngine::install(
            handle(1),
            abandon_factory.descriptor().clone(),
            &mut abandon_factory,
            init(),
            config(),
        )
        .unwrap();
        abandoned.start().unwrap();
        abandoned.abandon().unwrap();
        abandoned.abandon().unwrap();
        assert_eq!(abandon_calls.get(), 0);
        assert_eq!(abandoned.state(), GameEngineState::Closed);
        assert_eq!(abandoned.owner_snapshot().live_roles, 0);

        let shutdown_calls = Rc::new(Cell::new(0));
        let mut shutdown_factory = ShutdownTrackingFactory {
            descriptor: factory().descriptor,
            calls: Rc::clone(&shutdown_calls),
        };
        let mut shutdown = GameEngine::install(
            handle(2),
            shutdown_factory.descriptor().clone(),
            &mut shutdown_factory,
            init(),
            config(),
        )
        .unwrap();
        shutdown.start().unwrap();
        shutdown.shutdown().unwrap();
        shutdown.shutdown().unwrap();
        assert_eq!(shutdown_calls.get(), 1);
        assert_eq!(shutdown.owner_snapshot().live_roles, 0);
    }

    #[test]
    fn one_thousand_full_owner_graph_create_close_cycles_leave_zero_resources() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        for cycle in 1..=1_000 {
            let id = handle(cycle);
            let mut factory = factory_with_roles(roles.clone());
            let mut engine = GameEngine::install(
                id,
                factory.descriptor().clone(),
                &mut factory,
                init(),
                full_owner_graph_config(),
            )
            .unwrap();
            engine.start().unwrap();
            engine
                .manual_step(&InputFrame {
                    tick_id: 1,
                    bytes: vec![cycle as u8],
                })
                .unwrap();

            let entity = engine.world.entity_for_stable_key(1).unwrap();
            engine
                .render_outbox
                .enqueue(
                    engine.simulation_revision,
                    0,
                    vec![RenderOp::Spawn {
                        entity: crate::RenderEntity {
                            engine: id,
                            entity: entity.entity,
                        },
                        value: vec![1, 2, 3, 4],
                    }],
                )
                .unwrap();
            engine.render_outbox.poll_durable().unwrap().unwrap();
            engine
                .render_outbox
                .enqueue(
                    engine.simulation_revision,
                    0,
                    vec![RenderOp::Upsert {
                        entity: crate::RenderEntity {
                            engine: id,
                            entity: entity.entity,
                        },
                        value: vec![4, 3, 2, 1],
                    }],
                )
                .unwrap();
            let logic_io = engine
                .logic_io
                .prepare(vec![LogicIoCommand::AssetControl {
                    sequence: 1,
                    payload: vec![5, 6, 7],
                }])
                .unwrap();
            engine.logic_io.commit(logic_io).unwrap();
            engine
                .deliver_endpoint_event(SimulationEvent {
                    kind: 1,
                    bytes: vec![8, 9],
                })
                .unwrap();
            let domain = PresentationDomainId {
                engine: id,
                handle: handle(1),
            };
            engine
                .render_outbox
                .stage_transient(RenderTransient {
                    domain,
                    frame_id: 1,
                    required_render_revision: 0,
                    required_control_revision: 0,
                    bytes: vec![9, 8, 7],
                })
                .unwrap();
            let presentation = engine
                .presentation_state
                .prepare_frame(
                    domain,
                    1,
                    vec![PresentationStateOp::Set {
                        key: 1,
                        value: vec![10, 11],
                    }],
                )
                .unwrap();
            engine.presentation_state.commit(presentation).unwrap();
            let simulation = engine
                .simulation_state
                .prepare_stage(
                    vec![SimulationResourceOp::Set {
                        resource: 1,
                        value: vec![12, 13],
                    }],
                    vec![SimulationEvent {
                        kind: 2,
                        bytes: vec![14],
                    }],
                )
                .unwrap();
            engine.simulation_state.commit(simulation).unwrap();

            let live = engine.owner_snapshot();
            assert_eq!(live.live_roles, 4);
            assert_eq!(live.live_entities, 1);
            assert_eq!(live.pending_endpoint_events, 1);
            assert_eq!(live.pending_logic_io_commands, 1);
            assert_eq!(live.render_outbox.pending_ops, 1);
            assert!(live.render_outbox.transaction_inflight);
            assert_eq!(live.render_outbox.transient_domains, 1);

            engine.shutdown().unwrap();

            let closed = engine.owner_snapshot();
            assert_eq!(closed.state, GameEngineState::Closed);
            assert_eq!(closed.live_roles, 0);
            assert_eq!(closed.live_entities, 0);
            assert_eq!(closed.pending_endpoint_events, 0);
            assert_eq!(closed.pending_endpoint_event_bytes, 0);
            assert_eq!(closed.pending_logic_io_commands, 0);
            assert_eq!(closed.render_outbox.acked_objects, 0);
            assert_eq!(closed.render_outbox.pending_ops, 0);
            assert!(!closed.render_outbox.transaction_inflight);
            assert!(!closed.render_outbox.snapshot_inflight);
            assert!(!closed.render_outbox.snapshot_required);
            assert_eq!(closed.render_outbox.transient_domains, 0);
            assert!(engine.presentation_state.snapshot().domains.is_empty());
            assert!(engine.simulation_state.resource(1).is_none());
            assert_eq!(factory.constructions, 1);
        }
    }
}
