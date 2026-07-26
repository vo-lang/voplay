use std::collections::BTreeSet;

use vo_app_protocol::SurfaceHandle;
use voplay_protocol::{EngineId, Handle};

use crate::{
    device_hub::{DeviceHub, DeviceHubError, DeviceRef, EngineDeviceLease, EngineDeviceState},
    game_engine::{
        Game, GameEngine, GameEngineConfig, GameEngineError, GameEntryDescriptor, OwnedInitData,
        TargetIslandFactory,
    },
    input::{
        ActionBinding, InputDomain, InputDomainConfig, InputDomainShutdownReport, InputError,
        InputScopeId, NormalizedInputEvent, PlatformInputAdmission, TickInputFrame,
    },
    outbox::{OutboxError, PresentationDomainId},
    presentation::{
        FrameCommandEncoder, OutboxPresentOutcome, PresentationRuntime, PresentationRuntimeConfig,
        PresentationRuntimeError, RenderSyncOutcome,
    },
    surface::{
        GameSurfaceId, PresentOutcome, RemoteFrameOutcome, RemoteFramePreparation, RenderBackend,
        RenderBackendFailure, RenderFrameAck, SurfaceMetrics,
    },
    world::{InputFrame, SimulationTick},
    RenderEventResult,
};

#[derive(Clone, Debug)]
pub struct VoplayRuntimeConfig {
    pub game: GameEngineConfig,
    pub presentation: PresentationRuntimeConfig,
    pub input: InputDomainConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VoplayRuntimeError {
    Game(GameEngineError),
    Presentation(PresentationRuntimeError),
    Outbox(OutboxError),
    Input(InputError),
    DeviceHub(DeviceHubError),
    Backend(RenderBackendFailure),
    DeviceUnavailable,
    DeviceBindingChanged,
    InvalidRecoveryState,
    RemoteSurfaceRequired,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RendererRecoveryReport {
    pub snapshot_revision: u64,
    pub rebound_surfaces: usize,
    pub event_results: Vec<RenderEventResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteOutboxFrame {
    pub synchronization: RenderSyncOutcome,
    pub presentation: RemoteFramePreparation,
    pub source_simulation_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoplayRuntimeOwnerSnapshot {
    pub engine: EngineId,
    pub closed: bool,
    pub surfaces: usize,
    pub remote_surfaces: usize,
    pub input_scopes: usize,
    pub device_lease: EngineDeviceLease,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub game: crate::game_engine::GameEngineOwnerSnapshot,
    pub presentation: crate::presentation::PresentationRuntimeOwnerSnapshot,
    pub encoder: crate::presentation::FrameEncoderOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoplayRuntimeShutdownReport {
    pub engine: EngineId,
    pub released_surfaces: usize,
    pub released_remote_surfaces: usize,
    pub input: InputDomainShutdownReport,
    pub presentation: crate::presentation::PresentationRuntimeShutdownReport,
    pub game_before: crate::game_engine::GameEngineOwnerSnapshot,
    pub game_after: crate::game_engine::GameEngineOwnerSnapshot,
    pub encoder_before: crate::presentation::FrameEncoderOwnerSnapshot,
    pub encoder_after: crate::presentation::FrameEncoderOwnerSnapshot,
    pub device_detached: bool,
}

impl From<GameEngineError> for VoplayRuntimeError {
    fn from(error: GameEngineError) -> Self {
        Self::Game(error)
    }
}

impl From<PresentationRuntimeError> for VoplayRuntimeError {
    fn from(error: PresentationRuntimeError) -> Self {
        Self::Presentation(error)
    }
}

impl From<DeviceHubError> for VoplayRuntimeError {
    fn from(error: DeviceHubError) -> Self {
        Self::DeviceHub(error)
    }
}

impl From<OutboxError> for VoplayRuntimeError {
    fn from(error: OutboxError) -> Self {
        Self::Outbox(error)
    }
}

impl From<InputError> for VoplayRuntimeError {
    fn from(error: InputError) -> Self {
        Self::Input(error)
    }
}

impl From<RenderBackendFailure> for VoplayRuntimeError {
    fn from(error: RenderBackendFailure) -> Self {
        Self::Backend(error)
    }
}

pub struct VoplayRuntime<G, B, E>
where
    G: Game,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    game: GameEngine<G>,
    presentation: PresentationRuntime,
    input: InputDomain,
    backend: B,
    encoder: E,
    device_lease: EngineDeviceLease,
    render_endpoint: Handle,
    device_generation: u64,
    surfaces: BTreeSet<GameSurfaceId>,
    remote_surfaces: BTreeSet<GameSurfaceId>,
    closed: bool,
}

impl<G, B, E> VoplayRuntime<G, B, E>
where
    G: Game,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    pub const fn game(&self) -> &GameEngine<G> {
        &self.game
    }

    pub const fn presentation(&self) -> &PresentationRuntime {
        &self.presentation
    }

    pub const fn input(&self) -> &InputDomain {
        &self.input
    }

    pub fn configure_input(
        &mut self,
        revision: u64,
        bindings: Vec<ActionBinding>,
    ) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        self.input.configure_bindings_at(revision, bindings)?;
        Ok(())
    }

    pub fn route_input(
        &mut self,
        event: NormalizedInputEvent,
    ) -> Result<PlatformInputAdmission, VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.input.apply_platform_event(event)?)
    }

    pub fn step_input_scope(
        &mut self,
        scope: InputScopeId,
        tick_id: u64,
        through_timestamp_micros: u64,
    ) -> Result<(TickInputFrame, SimulationTick), VoplayRuntimeError> {
        self.require_open()?;
        let captured = self
            .input
            .capture_tick_through(scope, tick_id, through_timestamp_micros)?;
        let input = InputFrame {
            tick_id: captured.tick_id,
            bytes: captured.deterministic_bytes(),
        };
        let tick = self.game.manual_step(&input)?;
        Ok((captured, tick))
    }

    pub const fn device_lease(&self) -> EngineDeviceLease {
        self.device_lease
    }

    pub const fn render_endpoint(&self) -> Handle {
        self.render_endpoint
    }

    pub const fn device_generation(&self) -> u64 {
        self.device_generation
    }

    pub fn owner_snapshot(&self) -> VoplayRuntimeOwnerSnapshot {
        VoplayRuntimeOwnerSnapshot {
            engine: self.game.id(),
            closed: self.closed,
            surfaces: self.surfaces.len(),
            remote_surfaces: self.remote_surfaces.len(),
            input_scopes: self.input.scope_count(),
            device_lease: self.device_lease,
            render_endpoint: self.render_endpoint,
            device_generation: self.device_generation,
            game: self.game.owner_snapshot(),
            presentation: self.presentation.owner_snapshot(),
            encoder: self.encoder.owner_snapshot(),
        }
    }

    pub fn manual_step(
        &mut self,
        input: &InputFrame,
    ) -> Result<SimulationTick, VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.game.manual_step(input)?)
    }

    pub fn attach(
        &mut self,
        hub: &DeviceHub,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
    ) -> Result<GameSurfaceId, VoplayRuntimeError> {
        self.require_current_device(hub)?;
        let surface = self.presentation.attach_surface(
            surface,
            domain,
            metrics,
            self.render_endpoint,
            self.device_generation,
            &mut self.backend,
        )?;
        if let Err(error) = self.input.register_scope(InputScopeId {
            engine: surface.engine,
            handle: surface.domain.handle,
        }) {
            let _ = self.presentation.close_surface(surface, &mut self.backend);
            return Err(VoplayRuntimeError::Input(error));
        }
        self.surfaces.insert(surface);
        Ok(surface)
    }

    pub fn attach_remote(
        &mut self,
        hub: &DeviceHub,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
    ) -> Result<GameSurfaceId, VoplayRuntimeError> {
        self.require_current_device(hub)?;
        let surface = self.presentation.attach_surface_remote(
            surface,
            domain,
            metrics,
            self.render_endpoint,
            self.device_generation,
        )?;
        if let Err(error) = self.input.register_scope(InputScopeId {
            engine: surface.engine,
            handle: surface.domain.handle,
        }) {
            let _ = self.presentation.close_surface_remote(surface);
            return Err(VoplayRuntimeError::Input(error));
        }
        self.surfaces.insert(surface);
        self.remote_surfaces.insert(surface);
        Ok(surface)
    }

    pub fn resize(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        self.presentation
            .resize_surface(surface, metrics, &mut self.backend)?;
        Ok(())
    }

    pub fn pulse(
        &mut self,
        hub: &DeviceHub,
        surface: GameSurfaceId,
        now_micros: u64,
        deadline_micros: u64,
        graph_signature: u64,
        input: Option<&InputFrame>,
    ) -> Result<OutboxPresentOutcome, VoplayRuntimeError> {
        self.require_current_device(hub)?;
        if !self.surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::Closed);
        }
        let pulse = self
            .presentation
            .begin_pulse(surface, now_micros, deadline_micros)?;
        self.game.presentation_pulse(pulse, input)?;
        self.presentation
            .set_control_revision(self.game.render_control_revision());
        let outcome = self.presentation.present_from_outbox(
            pulse,
            now_micros,
            graph_signature,
            self.game.render_outbox_mut(),
            &mut self.encoder,
            &mut self.backend,
        )?;
        Ok(outcome)
    }

    pub fn pulse_remote(
        &mut self,
        hub: &DeviceHub,
        surface: GameSurfaceId,
        now_micros: u64,
        deadline_micros: u64,
        graph_signature: u64,
        input: Option<&InputFrame>,
    ) -> Result<RemoteOutboxFrame, VoplayRuntimeError> {
        self.require_current_device(hub)?;
        if !self.surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::Closed);
        }
        let pulse = self
            .presentation
            .begin_pulse(surface, now_micros, deadline_micros)?;
        self.game.presentation_pulse(pulse, input)?;
        self.presentation
            .set_control_revision(self.game.render_control_revision());
        let synchronization = self
            .presentation
            .synchronize_outbox(self.game.render_outbox_mut())?;
        let transient = self
            .game
            .render_outbox_mut()
            .take_ready_transient_for_frame(
                pulse.surface.domain,
                pulse.pulse_id,
                self.presentation.engine().render_revision(),
                self.presentation.engine().render_control_revision(),
            );
        let presentation = self.presentation.prepare_remote_present(
            pulse,
            now_micros,
            graph_signature,
            transient,
            &mut self.encoder,
        )?;
        Ok(RemoteOutboxFrame {
            synchronization,
            presentation,
            source_simulation_revision: self.game.simulation_revision(),
        })
    }

    pub fn pulse_remote_scheduled(
        &mut self,
        hub: &DeviceHub,
        surface: GameSurfaceId,
        pulse_id: u64,
        now_micros: u64,
        deadline_micros: u64,
        graph_signature: u64,
        input: Option<&InputFrame>,
    ) -> Result<RemoteOutboxFrame, VoplayRuntimeError> {
        self.require_current_device(hub)?;
        if !self.surfaces.contains(&surface) || !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::Closed);
        }
        let pulse = self.presentation.begin_scheduled_pulse(
            surface,
            pulse_id,
            now_micros,
            deadline_micros,
        )?;
        self.game.presentation_pulse(pulse, input)?;
        self.presentation
            .set_control_revision(self.game.render_control_revision());
        let synchronization = self
            .presentation
            .synchronize_outbox(self.game.render_outbox_mut())?;
        let transient = self
            .game
            .render_outbox_mut()
            .take_ready_transient_for_frame(
                pulse.surface.domain,
                pulse.pulse_id,
                self.presentation.engine().render_revision(),
                self.presentation.engine().render_control_revision(),
            );
        let presentation = self.presentation.prepare_remote_present(
            pulse,
            now_micros,
            graph_signature,
            transient,
            &mut self.encoder,
        )?;
        Ok(RemoteOutboxFrame {
            synchronization,
            presentation,
            source_simulation_revision: self.game.simulation_revision(),
        })
    }

    pub fn complete_remote_frame(
        &mut self,
        ack: RenderFrameAck,
        outcome: RemoteFrameOutcome,
    ) -> Result<PresentOutcome, VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.presentation.complete_remote_present(ack, outcome)?)
    }

    pub fn detach(&mut self, surface: GameSurfaceId) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        if !self.surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::Closed);
        }
        self.presentation
            .close_surface(surface, &mut self.backend)?;
        self.surfaces.remove(&surface);
        self.unregister_input_scope_if_unused(surface)?;
        Ok(())
    }

    pub fn detach_remote(&mut self, surface: GameSurfaceId) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        if !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::RemoteSurfaceRequired);
        }
        self.presentation.close_surface_remote(surface)?;
        self.surfaces.remove(&surface);
        self.remote_surfaces.remove(&surface);
        self.unregister_input_scope_if_unused(surface)?;
        Ok(())
    }

    pub fn remote_frame_pending(&self, surface: GameSurfaceId) -> Result<bool, VoplayRuntimeError> {
        self.require_open()?;
        if !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::RemoteSurfaceRequired);
        }
        Ok(self.presentation.remote_frame_pending(surface)?)
    }

    pub fn resize_remote_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        if !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::RemoteSurfaceRequired);
        }
        Ok(self.presentation.resize_surface_remote(surface, metrics)?)
    }

    pub fn suspend_remote_surface(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        if !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::RemoteSurfaceRequired);
        }
        Ok(self.presentation.suspend_surface(surface)?)
    }

    pub fn resume_remote_surface(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        if !self.remote_surfaces.contains(&surface) {
            return Err(VoplayRuntimeError::RemoteSurfaceRequired);
        }
        Ok(self.presentation.resume_surface(surface)?)
    }

    pub fn surface_metrics(
        &self,
        surface: GameSurfaceId,
    ) -> Result<SurfaceMetrics, VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.presentation.surface_metrics(surface)?)
    }

    pub fn pause(&mut self) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.game.pause()?)
    }

    pub fn resume(&mut self) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        Ok(self.game.resume()?)
    }

    pub fn recover_renderer(
        &mut self,
        hub: &mut DeviceHub,
    ) -> Result<RendererRecoveryReport, VoplayRuntimeError> {
        self.require_open()?;
        let status = hub.status(self.device_lease)?;
        if !matches!(
            status.state,
            EngineDeviceState::RendererRecoveryRequired | EngineDeviceState::AwaitingDeviceRebind
        ) {
            return Err(VoplayRuntimeError::InvalidRecoveryState);
        }
        let simulation_revision = self.game.simulation_revision();
        let control_revision = self.game.render_control_revision();
        let snapshot = self
            .game
            .render_outbox_mut()
            .begin_renderer_recovery(simulation_revision, control_revision)?;
        let snapshot_revision = snapshot.revision;
        let event_results = self.presentation.restart_renderer()?;
        self.presentation
            .set_control_revision(self.game.render_control_revision());
        let ack = self.presentation.apply_render_snapshot(snapshot)?;
        self.game.render_outbox_mut().acknowledge_snapshot(ack)?;
        let surfaces = self.surfaces.iter().copied().collect::<Vec<_>>();
        for surface in &surfaces {
            if self.remote_surfaces.contains(surface) {
                self.presentation.replace_surface_render_binding_remote(
                    *surface,
                    status.render_endpoint,
                    status.device_generation,
                )?;
            } else {
                self.presentation.replace_surface_render_binding(
                    *surface,
                    status.render_endpoint,
                    status.device_generation,
                    &mut self.backend,
                )?;
            }
        }
        match status.state {
            EngineDeviceState::RendererRecoveryRequired => hub.complete_renderer_rebind(
                self.device_lease,
                status.device_generation,
                status.render_endpoint,
            )?,
            EngineDeviceState::AwaitingDeviceRebind => hub.complete_device_rebind(
                self.device_lease,
                status.device_generation,
                status.render_endpoint,
            )?,
            EngineDeviceState::Ready | EngineDeviceState::DeviceLost => {
                return Err(VoplayRuntimeError::InvalidRecoveryState);
            }
        }
        self.render_endpoint = status.render_endpoint;
        self.device_generation = status.device_generation;
        Ok(RendererRecoveryReport {
            snapshot_revision,
            rebound_surfaces: surfaces.len(),
            event_results,
        })
    }

    pub fn shutdown(&mut self, hub: &mut DeviceHub) -> Result<(), VoplayRuntimeError> {
        self.shutdown_with_report(hub).map(|_| ())
    }

    pub fn shutdown_with_report(
        &mut self,
        hub: &mut DeviceHub,
    ) -> Result<VoplayRuntimeShutdownReport, VoplayRuntimeError> {
        if self.closed {
            return Ok(VoplayRuntimeShutdownReport {
                engine: self.game.id(),
                released_surfaces: 0,
                released_remote_surfaces: 0,
                input: self.input.shutdown(),
                presentation: self.presentation.shutdown()?,
                game_before: self.game.owner_snapshot(),
                game_after: self.game.owner_snapshot(),
                encoder_before: self.encoder.owner_snapshot(),
                encoder_after: self.encoder.owner_snapshot(),
                device_detached: false,
            });
        }
        let mut close_error = None;
        let surfaces = self.surfaces.iter().copied().collect::<Vec<_>>();
        let released_surfaces = surfaces.len();
        let released_remote_surfaces = self.remote_surfaces.len();
        for surface in surfaces {
            let close_result = if self.remote_surfaces.remove(&surface) {
                self.presentation.close_surface_remote_on_shutdown(surface)
            } else {
                self.presentation.close_surface(surface, &mut self.backend)
            };
            if let Err(error) = close_result {
                close_error.get_or_insert(VoplayRuntimeError::Presentation(error));
                self.presentation.abandon_surface(surface);
            }
            self.surfaces.remove(&surface);
        }
        self.remote_surfaces.clear();
        let input = self.input.shutdown();
        let encoder_before = self.encoder.owner_snapshot();
        self.encoder.shutdown();
        let encoder_after = self.encoder.owner_snapshot();
        let backend_result = self.backend.shutdown();
        let presentation_result = self.presentation.shutdown();
        let game_before = self.game.owner_snapshot();
        let game_result = self.game.shutdown();
        let game_after = self.game.owner_snapshot();
        let authoritative_lease = hub.lease_for_engine(self.game.id());
        let device_result = authoritative_lease.map_or(Ok(()), |lease| hub.detach_engine(lease));
        let device_detached = authoritative_lease.is_some() && device_result.is_ok();
        self.closed = true;
        if let Some(error) = close_error {
            return Err(error);
        }
        let presentation = presentation_result?;
        backend_result?;
        game_result?;
        device_result?;
        Ok(VoplayRuntimeShutdownReport {
            engine: self.game.id(),
            released_surfaces,
            released_remote_surfaces,
            input,
            presentation,
            game_before,
            game_after,
            encoder_before,
            encoder_after,
            device_detached,
        })
    }

    fn require_open(&self) -> Result<(), VoplayRuntimeError> {
        if self.closed {
            return Err(VoplayRuntimeError::Closed);
        }
        Ok(())
    }

    fn require_current_device(&self, hub: &DeviceHub) -> Result<(), VoplayRuntimeError> {
        self.require_open()?;
        let status = hub.status(self.device_lease)?;
        if status.state != EngineDeviceState::Ready {
            return Err(VoplayRuntimeError::DeviceUnavailable);
        }
        if status.render_endpoint != self.render_endpoint
            || status.device_generation != self.device_generation
        {
            return Err(VoplayRuntimeError::DeviceBindingChanged);
        }
        Ok(())
    }

    fn unregister_input_scope_if_unused(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), VoplayRuntimeError> {
        if self
            .surfaces
            .iter()
            .any(|candidate| candidate.domain == surface.domain)
        {
            return Ok(());
        }
        self.input.unregister_scope(InputScopeId {
            engine: surface.engine,
            handle: surface.domain.handle,
        })?;
        Ok(())
    }
}

pub fn run<F, B, E>(
    id: EngineId,
    descriptor: GameEntryDescriptor,
    factory: &mut F,
    init: OwnedInitData,
    hub: &mut DeviceHub,
    device: DeviceRef,
    render_endpoint: Handle,
    backend: B,
    encoder: E,
    config: VoplayRuntimeConfig,
) -> Result<VoplayRuntime<F::Game, B, E>, VoplayRuntimeError>
where
    F: TargetIslandFactory,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    let mut game = GameEngine::install(id, descriptor, factory, init, config.game)?;
    let presentation = PresentationRuntime::new(id, config.presentation)?;
    let input = InputDomain::new(id, config.input)?;
    let device_lease = hub.attach_engine(id, device, render_endpoint)?;
    let status = hub.status(device_lease)?;
    if let Err(error) = game.start() {
        let _ = hub.detach_engine(device_lease);
        return Err(VoplayRuntimeError::Game(error));
    }
    Ok(VoplayRuntime {
        game,
        presentation,
        input,
        backend,
        encoder,
        device_lease,
        render_endpoint: status.render_endpoint,
        device_generation: status.device_generation,
        surfaces: BTreeSet::new(),
        remote_surfaces: BTreeSet::new(),
        closed: false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::{
        buffer_lease::BufferLeaseMetrics,
        control::EngineControlConfig,
        game_engine::{GameConfigure, GameContext},
        headless_renderer::{HeadlessRenderBackend, HeadlessRendererConfig},
        host_render_backend::HostRenderTransportMetrics,
        instrumentation::VoplayInstrumentationSnapshot,
        logic_io::LogicIoConfig,
        outbox::RenderOutboxConfig,
        presentation::PresentationRuntimeConfig,
        presentation_state::PresentationStateConfig,
        render_commands::{PortableFrameEncoder, PortableFrameEncoderConfig},
        schedule::SystemSpec,
        simulation_state::SimulationStateConfig,
        supervisor::{InstanceGroupConfig, Role},
        surface::{
            PresentOutcome, RenderBackendFailure, RenderFrameAck, RenderFrameSubmission,
            SurfaceRuntimeConfig,
        },
        world::WorldConfig,
        EngineConfig,
    };

    use super::*;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    struct EmptyGame {
        fail_shutdown: bool,
    }

    impl Game for EmptyGame {
        fn configure(&self, configure: &mut GameConfigure) -> Result<(), GameEngineError> {
            if self.fail_shutdown {
                configure.add_system(SystemSpec {
                    name: "fail-shutdown".to_owned(),
                    stage: crate::schedule::Stage::Shutdown,
                    deterministic: false,
                    access: crate::schedule::AccessSet::default(),
                    before: Vec::new(),
                    after: Vec::new(),
                });
            }
            Ok(())
        }

        fn execute(
            &mut self,
            system: &SystemSpec,
            _context: &mut GameContext<'_>,
            _input: Option<&InputFrame>,
        ) -> Result<(), GameEngineError> {
            if system.name == "fail-shutdown" {
                return Err(GameEngineError::GameFailure);
            }
            Ok(())
        }
    }

    struct EmptyFactory {
        descriptor: GameEntryDescriptor,
        fail_shutdown: bool,
    }

    impl TargetIslandFactory for EmptyFactory {
        type Game = EmptyGame;

        fn descriptor(&self) -> &GameEntryDescriptor {
            &self.descriptor
        }

        fn construct_logic(&mut self, _init: OwnedInitData) -> Result<Self::Game, GameEngineError> {
            Ok(EmptyGame {
                fail_shutdown: self.fail_shutdown,
            })
        }
    }

    fn descriptor(roles: BTreeSet<Role>) -> GameEntryDescriptor {
        GameEntryDescriptor {
            entry_factory_id: 1,
            code_artifact_digest: [1; 32],
            entry_schema_fingerprint: [2; 32],
            init_config_fingerprint: [3; 32],
            required_provider_template: 1,
            roles,
        }
    }

    fn config(roles: BTreeSet<Role>) -> VoplayRuntimeConfig {
        VoplayRuntimeConfig {
            game: GameEngineConfig {
                world: WorldConfig {
                    max_entities: 4,
                    max_components_per_entity: 4,
                    max_component_bytes: 64,
                    max_snapshot_bytes: 1024,
                },
                group: InstanceGroupConfig {
                    roles,
                    allow_logic_recovery: false,
                    control: EngineControlConfig::default(),
                },
                actors: BTreeMap::from([
                    (Role::Logic, handle(10)),
                    (Role::Asset, handle(20)),
                    (Role::Render, handle(30)),
                    (Role::Audio, handle(40)),
                ]),
                render_outbox: RenderOutboxConfig::default(),
                presentation_state: PresentationStateConfig::default(),
                simulation_state: SimulationStateConfig::default(),
                logic_io: LogicIoConfig::default(),
                max_init_bytes: 64,
                max_stage_world_commands: 64,
            },
            presentation: PresentationRuntimeConfig {
                engine: EngineConfig::default(),
                surfaces: SurfaceRuntimeConfig::default(),
            },
            input: InputDomainConfig::default(),
        }
    }

    fn metrics() -> SurfaceMetrics {
        SurfaceMetrics {
            width: 8,
            height: 8,
            scale_numerator: 1,
            scale_denominator: 1,
        }
    }

    struct FailDetachBackend;

    impl RenderBackend for FailDetachBackend {
        fn attach_surface(
            &mut self,
            _surface: GameSurfaceId,
            _metrics: SurfaceMetrics,
        ) -> Result<(), RenderBackendFailure> {
            Ok(())
        }

        fn resize_surface(
            &mut self,
            _surface: GameSurfaceId,
            _metrics: SurfaceMetrics,
        ) -> Result<(), RenderBackendFailure> {
            Ok(())
        }

        fn rebind_surface(
            &mut self,
            _surface: GameSurfaceId,
            _metrics: SurfaceMetrics,
        ) -> Result<(), RenderBackendFailure> {
            Ok(())
        }

        fn render(
            &mut self,
            _submission: &RenderFrameSubmission,
        ) -> Result<RenderFrameAck, RenderBackendFailure> {
            unreachable!("shutdown failure fixture never renders")
        }

        fn present(
            &mut self,
            _ack: RenderFrameAck,
            _now_micros: u64,
            _deadline_micros: u64,
        ) -> Result<PresentOutcome, RenderBackendFailure> {
            unreachable!("shutdown failure fixture never presents")
        }

        fn detach_surface(&mut self, _surface: GameSurfaceId) -> Result<(), RenderBackendFailure> {
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        }
    }

    #[test]
    fn one_thousand_runtime_create_close_cycles_release_surfaces_scopes_and_device_bindings() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig {
                max_devices: 1,
                max_engine_bindings: 1,
            },
        )
        .unwrap();
        let device = hub.register_device().unwrap();

        for cycle in 1..=1_000 {
            let engine = handle(cycle);
            let entry = descriptor(roles.clone());
            let mut factory = EmptyFactory {
                descriptor: entry.clone(),
                fail_shutdown: false,
            };
            let mut runtime = run(
                engine,
                entry,
                &mut factory,
                OwnedInitData {
                    config_fingerprint: [3; 32],
                    bytes: Vec::new(),
                },
                &mut hub,
                device,
                handle(100_000 + cycle),
                HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
                PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
                config(roles.clone()),
            )
            .unwrap();
            runtime
                .attach(
                    &hub,
                    SurfaceHandle {
                        index: 1,
                        generation: 1,
                    },
                    PresentationDomainId {
                        engine,
                        handle: handle(1),
                    },
                    metrics(),
                )
                .unwrap();
            runtime
                .attach_remote(
                    &hub,
                    SurfaceHandle {
                        index: 2,
                        generation: 1,
                    },
                    PresentationDomainId {
                        engine,
                        handle: handle(2),
                    },
                    metrics(),
                )
                .unwrap();

            let live = runtime.owner_snapshot();
            assert_eq!(live.surfaces, 2);
            assert_eq!(live.remote_surfaces, 1);
            assert_eq!(live.input_scopes, 2);
            assert_eq!(hub.metrics().engine_bindings, 1);

            runtime.shutdown(&mut hub).unwrap();

            let closed = runtime.owner_snapshot();
            assert!(closed.closed);
            assert_eq!(closed.surfaces, 0);
            assert_eq!(closed.remote_surfaces, 0);
            assert_eq!(closed.input_scopes, 0);
            assert_eq!(hub.metrics().engine_bindings, 0);
            let snapshot = VoplayInstrumentationSnapshot::capture(
                &runtime,
                &hub,
                [],
                HostRenderTransportMetrics::default(),
                BufferLeaseMetrics::default(),
            );
            assert!(snapshot.leak_summary().is_zero());
        }
    }

    #[test]
    fn failing_game_shutdown_hook_still_closes_runtime_and_detaches_device() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let engine = handle(1);
        let entry = descriptor(roles.clone());
        let mut factory = EmptyFactory {
            descriptor: entry.clone(),
            fail_shutdown: true,
        };
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig::default(),
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut runtime = run(
            engine,
            entry,
            &mut factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();

        assert_eq!(
            runtime.shutdown(&mut hub),
            Err(VoplayRuntimeError::Game(GameEngineError::GameFailure))
        );
        let closed = runtime.owner_snapshot();
        assert!(closed.closed);
        assert_eq!(
            closed.game.state,
            crate::game_engine::GameEngineState::Closed
        );
        assert_eq!(closed.game.live_roles, 0);
        assert_eq!(hub.metrics().engine_bindings, 0);
    }

    #[test]
    fn failing_backend_detach_is_reported_after_all_runtime_owners_are_released() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let engine = handle(1);
        let entry = descriptor(roles.clone());
        let mut factory = EmptyFactory {
            descriptor: entry.clone(),
            fail_shutdown: false,
        };
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig::default(),
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut runtime = run(
            engine,
            entry,
            &mut factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            FailDetachBackend,
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();
        runtime
            .attach(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine,
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();

        assert_eq!(
            runtime.shutdown(&mut hub),
            Err(VoplayRuntimeError::Presentation(
                PresentationRuntimeError::Surface(
                    crate::surface::SurfaceRuntimeError::BackendRejected
                )
            ))
        );
        let closed = runtime.owner_snapshot();
        assert!(closed.closed);
        assert_eq!(closed.surfaces, 0);
        assert_eq!(closed.input_scopes, 0);
        assert_eq!(
            closed.game.state,
            crate::game_engine::GameEngineState::Closed
        );
        assert_eq!(hub.metrics().engine_bindings, 0);
    }

    #[test]
    fn shared_device_hub_isolates_renderer_fault_and_recovers_both_engines_after_device_loss() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig {
                max_devices: 1,
                max_engine_bindings: 2,
            },
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut first_factory = EmptyFactory {
            descriptor: descriptor(roles.clone()),
            fail_shutdown: false,
        };
        let mut second_factory = EmptyFactory {
            descriptor: descriptor(roles.clone()),
            fail_shutdown: false,
        };
        let mut first = run(
            handle(1),
            first_factory.descriptor().clone(),
            &mut first_factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles.clone()),
        )
        .unwrap();
        let mut second = run(
            handle(2),
            second_factory.descriptor().clone(),
            &mut second_factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(200),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();
        let first_surface = first
            .attach(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine: handle(1),
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();
        let second_surface = second
            .attach(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine: handle(2),
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();

        let second_binding_before = hub.status(second.device_lease()).unwrap();
        let first_replacement = hub
            .report_renderer_fault(first.device_lease(), first.render_endpoint())
            .unwrap();
        assert_eq!(
            hub.status(first.device_lease()).unwrap().state,
            EngineDeviceState::RendererRecoveryRequired
        );
        assert_eq!(
            hub.status(second.device_lease()).unwrap(),
            second_binding_before
        );
        assert_eq!(
            second
                .pulse(&hub, second_surface, 1, 10, 1, None)
                .unwrap()
                .presentation,
            PresentOutcome::Presented
        );

        let first_recovery = first.recover_renderer(&mut hub).unwrap();
        assert_eq!(first_recovery.rebound_surfaces, 1);
        assert_eq!(first.render_endpoint(), first_replacement);
        assert_eq!(
            hub.status(second.device_lease()).unwrap(),
            second_binding_before
        );

        hub.report_device_lost(device, 1, crate::device_hub::DeviceLossReason::Reset)
            .unwrap();
        let replacement_device_generation = hub.begin_device_recovery(device).unwrap();
        assert_eq!(replacement_device_generation, 2);
        assert_eq!(
            hub.status(first.device_lease()).unwrap().state,
            EngineDeviceState::AwaitingDeviceRebind
        );
        assert_eq!(
            hub.status(second.device_lease()).unwrap().state,
            EngineDeviceState::AwaitingDeviceRebind
        );

        let first_device_recovery = first.recover_renderer(&mut hub).unwrap();
        assert_eq!(first_device_recovery.rebound_surfaces, 1);
        assert_eq!(
            hub.device_status(device).unwrap().state,
            crate::device_hub::DeviceState::Recovering
        );
        let second_device_recovery = second.recover_renderer(&mut hub).unwrap();
        assert_eq!(second_device_recovery.rebound_surfaces, 1);
        assert_eq!(
            hub.device_status(device).unwrap().state,
            crate::device_hub::DeviceState::Ready
        );
        assert_eq!(first.device_generation(), 2);
        assert_eq!(second.device_generation(), 2);

        assert_eq!(
            first
                .pulse(&hub, first_surface, 11, 20, 1, None)
                .unwrap()
                .presentation,
            PresentOutcome::Presented
        );
        assert_eq!(
            second
                .pulse(&hub, second_surface, 11, 20, 1, None)
                .unwrap()
                .presentation,
            PresentOutcome::Presented
        );
        first.shutdown(&mut hub).unwrap();
        second.shutdown(&mut hub).unwrap();
        assert_eq!(hub.metrics().engine_bindings, 0);
    }

    #[test]
    fn shutting_down_waiting_engine_completes_shared_device_recovery() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig {
                max_devices: 1,
                max_engine_bindings: 2,
            },
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut first_factory = EmptyFactory {
            descriptor: descriptor(roles.clone()),
            fail_shutdown: false,
        };
        let mut second_factory = EmptyFactory {
            descriptor: descriptor(roles.clone()),
            fail_shutdown: false,
        };
        let mut first = run(
            handle(1),
            first_factory.descriptor().clone(),
            &mut first_factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles.clone()),
        )
        .unwrap();
        let mut second = run(
            handle(2),
            second_factory.descriptor().clone(),
            &mut second_factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(200),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();

        hub.report_device_lost(device, 1, crate::device_hub::DeviceLossReason::Reset)
            .unwrap();
        hub.begin_device_recovery(device).unwrap();
        first.recover_renderer(&mut hub).unwrap();
        assert_eq!(
            hub.device_status(device).unwrap().state,
            crate::device_hub::DeviceState::Recovering
        );
        assert_eq!(
            hub.status(second.device_lease()).unwrap().state,
            EngineDeviceState::AwaitingDeviceRebind
        );

        second.shutdown(&mut hub).unwrap();
        assert_eq!(
            hub.device_status(device).unwrap().state,
            crate::device_hub::DeviceState::Ready
        );
        let surface = first
            .attach(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine: handle(1),
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();
        assert_eq!(
            first
                .pulse(&hub, surface, 1, 10, 1, None)
                .unwrap()
                .presentation,
            PresentOutcome::Presented
        );
        first.shutdown(&mut hub).unwrap();
        assert_eq!(hub.metrics().engine_bindings, 0);
    }

    #[test]
    fn suspended_remote_domain_does_not_block_peer_domain_frame_completion() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let engine = handle(1);
        let entry = descriptor(roles.clone());
        let mut factory = EmptyFactory {
            descriptor: entry.clone(),
            fail_shutdown: false,
        };
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig::default(),
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut runtime = run(
            engine,
            entry,
            &mut factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();
        let suspended = runtime
            .attach_remote(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine,
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();
        let peer = runtime
            .attach_remote(
                &hub,
                SurfaceHandle {
                    index: 2,
                    generation: 1,
                },
                PresentationDomainId {
                    engine,
                    handle: handle(2),
                },
                metrics(),
            )
            .unwrap();
        runtime.suspend_remote_surface(suspended).unwrap();

        let suspended_frame = runtime
            .pulse_remote_scheduled(&hub, suspended, 1, 1, 10, 1, None)
            .unwrap();
        assert_eq!(
            suspended_frame.presentation,
            RemoteFramePreparation::Terminal(PresentOutcome::Suspended)
        );
        let peer_frame = runtime
            .pulse_remote_scheduled(&hub, peer, 1, 1, 10, 1, None)
            .unwrap();
        let submission = match peer_frame.presentation {
            RemoteFramePreparation::Submit(submission) => submission,
            RemoteFramePreparation::Terminal(outcome) => {
                panic!("peer domain unexpectedly returned terminal {outcome:?}")
            }
        };
        assert_eq!(
            runtime
                .complete_remote_frame(
                    RenderFrameAck {
                        surface: submission.surface,
                        pulse_id: submission.pulse_id,
                        frame_id: submission.frame_id,
                        render_endpoint: submission.render_endpoint,
                        device_generation: submission.device_generation,
                        rendered_revision: submission.required_render_revision,
                        observed_control_revision: submission.required_control_revision,
                    },
                    RemoteFrameOutcome::Presented,
                )
                .unwrap(),
            PresentOutcome::Presented
        );
        assert!(!runtime.remote_frame_pending(peer).unwrap());
        runtime.resume_remote_surface(suspended).unwrap();
        runtime.shutdown(&mut hub).unwrap();
        assert_eq!(hub.metrics().engine_bindings, 0);
    }

    #[test]
    fn runtime_shutdown_abandons_pending_remote_frame_and_releases_all_owners() {
        let roles = BTreeSet::from([Role::Logic, Role::Asset, Role::Render, Role::Audio]);
        let engine = handle(1);
        let entry = descriptor(roles.clone());
        let mut factory = EmptyFactory {
            descriptor: entry.clone(),
            fail_shutdown: false,
        };
        let mut hub = DeviceHub::new(
            crate::device_hub::DeviceHubId(handle(50)),
            crate::device_hub::DeviceHubConfig::default(),
        )
        .unwrap();
        let device = hub.register_device().unwrap();
        let mut runtime = run(
            engine,
            entry,
            &mut factory,
            OwnedInitData {
                config_fingerprint: [3; 32],
                bytes: Vec::new(),
            },
            &mut hub,
            device,
            handle(100),
            HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap(),
            PortableFrameEncoder::new(PortableFrameEncoderConfig::default()).unwrap(),
            config(roles),
        )
        .unwrap();
        let surface = runtime
            .attach_remote(
                &hub,
                SurfaceHandle {
                    index: 1,
                    generation: 1,
                },
                PresentationDomainId {
                    engine,
                    handle: handle(1),
                },
                metrics(),
            )
            .unwrap();
        let frame = runtime
            .pulse_remote_scheduled(&hub, surface, 1, 1, 10, 1, None)
            .unwrap();
        assert!(matches!(
            frame.presentation,
            RemoteFramePreparation::Submit(_)
        ));
        assert!(runtime.remote_frame_pending(surface).unwrap());

        runtime.shutdown(&mut hub).unwrap();

        let closed = runtime.owner_snapshot();
        assert!(closed.closed);
        assert_eq!(closed.surfaces, 0);
        assert_eq!(closed.remote_surfaces, 0);
        assert_eq!(closed.input_scopes, 0);
        assert_eq!(closed.game.live_roles, 0);
        assert_eq!(hub.metrics().engine_bindings, 0);
    }
}
