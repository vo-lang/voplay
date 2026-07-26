use std::collections::BTreeMap;

use vo_app_protocol::SurfaceHandle;
use voplay_protocol::{EngineId, Handle};

use crate::{
    control::StableControlRef,
    outbox::PresentationDomainId,
    render_readback::{ReadbackFormat, ReadbackRegion, ReadbackRequestId},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GameSurfaceId {
    pub engine: EngineId,
    pub surface: SurfaceHandle,
    pub domain: PresentationDomainId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceMetrics {
    pub width: u32,
    pub height: u32,
    pub scale_numerator: u32,
    pub scale_denominator: u32,
}

impl SurfaceMetrics {
    pub const fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0 && self.scale_numerator > 0 && self.scale_denominator > 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameSurfaceState {
    Attached,
    Active,
    Suspended,
    Lost,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationPulse {
    pub surface: GameSurfaceId,
    pub pulse_id: u64,
    pub deadline_micros: u64,
    pub metrics: SurfaceMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderFrameSubmission {
    pub surface: GameSurfaceId,
    pub pulse_id: u64,
    pub frame_id: u64,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub required_render_revision: u64,
    pub required_control_revision: u64,
    pub graph_signature: u64,
    pub commands: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderFrameAck {
    pub surface: GameSurfaceId,
    pub pulse_id: u64,
    pub frame_id: u64,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub rendered_revision: u64,
    pub observed_control_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentOutcome {
    Presented,
    DeadlineMissed,
    Suspended,
    SurfaceLost,
    DeviceLost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderBackendFailure {
    RejectedBeforeSubmit,
    OutcomeUnknown,
    SurfaceLost,
    DeviceLost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderBackendReadback {
    pub request: ReadbackRequestId,
    pub target: StableControlRef,
    pub target_revision: u64,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFrameOutcome {
    Presented,
    DeadlineMissed,
    Suspended,
    SurfaceLost,
    DeviceLost,
    RejectedBeforeSubmit,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteFramePreparation {
    Submit(RenderFrameSubmission),
    Terminal(PresentOutcome),
}

pub trait RenderBackend {
    fn shutdown(&mut self) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    #[cfg(feature = "inspection")]
    fn request_frame_trace(
        &mut self,
        _request: u64,
        _frame_id: u64,
        _graph_signature: u64,
        _include_attachments: bool,
        _include_shader_diagnostics: bool,
        _max_bytes: usize,
    ) -> Result<(), RenderBackendFailure> {
        Err(RenderBackendFailure::RejectedBeforeSubmit)
    }

    #[cfg(feature = "inspection")]
    fn poll_frame_trace(&mut self, _request: u64) -> Result<Option<Vec<u8>>, RenderBackendFailure> {
        Ok(None)
    }

    #[cfg(feature = "inspection")]
    fn cancel_frame_trace(&mut self, _request: u64) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    fn upload_rgba_texture(
        &mut self,
        _texture: u64,
        _width: u32,
        _height: u32,
        _pixels: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        Err(RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn remove_texture(&mut self, _texture: u64) -> Result<(), RenderBackendFailure> {
        Err(RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn upsert_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
        _bytes: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    fn remove_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
    ) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    fn synchronize_render_targets(
        &mut self,
        _targets: &[StableControlRef],
    ) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    fn request_readback(
        &mut self,
        _request: ReadbackRequestId,
        _target: StableControlRef,
        _expected_target_revision: u64,
        _region: ReadbackRegion,
        _format: ReadbackFormat,
    ) -> Result<(), RenderBackendFailure> {
        Err(RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn poll_readback(
        &mut self,
        _request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, RenderBackendFailure> {
        Ok(None)
    }

    fn cancel_readback(&mut self, _request: ReadbackRequestId) -> Result<(), RenderBackendFailure> {
        Ok(())
    }

    fn attach_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure>;

    fn resize_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure>;

    fn rebind_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure>;

    fn render(
        &mut self,
        submission: &RenderFrameSubmission,
    ) -> Result<RenderFrameAck, RenderBackendFailure>;

    fn present(
        &mut self,
        ack: RenderFrameAck,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, RenderBackendFailure>;

    fn detach_surface(&mut self, surface: GameSurfaceId) -> Result<(), RenderBackendFailure>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceRuntimeConfig {
    pub max_surfaces: usize,
    pub max_command_bytes: usize,
}

impl Default for SurfaceRuntimeConfig {
    fn default() -> Self {
        Self {
            max_surfaces: 16,
            max_command_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceRuntimeError {
    InvalidConfig,
    WrongEngine,
    InvalidSurface,
    SurfaceCapacity,
    DuplicateSurface,
    DuplicateDomain,
    StaleSurface,
    InvalidState,
    PulseSequence,
    FrameSequence,
    ClockRegression,
    DeadlineExpired,
    CommandCapacity,
    RenderBarrier,
    ControlBarrier,
    StaleRenderEndpoint,
    StaleDeviceGeneration,
    BackendRejected,
    BackendOutcomeUnknown,
    AckMismatch,
    GenerationExhausted,
    Closed,
}

struct SurfaceRecord {
    id: GameSurfaceId,
    state: GameSurfaceState,
    metrics: SurfaceMetrics,
    next_pulse_id: u64,
    next_frame_id: u64,
    rendered_revision: u64,
    observed_control_revision: u64,
    render_endpoint: Handle,
    device_generation: u64,
    pending_remote: Option<RenderFrameSubmission>,
}

pub struct SurfaceRuntime {
    engine: EngineId,
    config: SurfaceRuntimeConfig,
    surfaces: BTreeMap<SurfaceHandle, SurfaceRecord>,
    domains: BTreeMap<PresentationDomainId, SurfaceHandle>,
    closed: bool,
}

impl SurfaceRuntime {
    pub fn new(
        engine: EngineId,
        config: SurfaceRuntimeConfig,
    ) -> Result<Self, SurfaceRuntimeError> {
        if !engine.is_valid() || config.max_surfaces == 0 || config.max_command_bytes == 0 {
            return Err(SurfaceRuntimeError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            surfaces: BTreeMap::new(),
            domains: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn state(&self, id: GameSurfaceId) -> Result<GameSurfaceState, SurfaceRuntimeError> {
        Ok(self.record(id)?.state)
    }

    pub fn live_surface_count(&self) -> usize {
        self.surfaces.len()
    }

    pub(crate) fn abandon_all(&mut self) -> usize {
        let released = self.surfaces.len();
        self.surfaces.clear();
        self.domains.clear();
        released
    }

    pub fn shutdown(&mut self) -> usize {
        if self.closed {
            return 0;
        }
        let released = self.abandon_all();
        self.closed = true;
        released
    }

    pub fn attach(
        &mut self,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        render_endpoint: Handle,
        device_generation: u64,
        backend: &mut dyn RenderBackend,
    ) -> Result<GameSurfaceId, SurfaceRuntimeError> {
        self.ensure_open()?;
        if !surface.is_valid()
            || !domain.handle.is_valid()
            || !metrics.is_valid()
            || !render_endpoint.is_valid()
            || device_generation == 0
        {
            return Err(SurfaceRuntimeError::InvalidSurface);
        }
        if domain.engine != self.engine {
            return Err(SurfaceRuntimeError::WrongEngine);
        }
        if self.surfaces.contains_key(&surface) {
            return Err(SurfaceRuntimeError::DuplicateSurface);
        }
        if self.domains.contains_key(&domain) {
            return Err(SurfaceRuntimeError::DuplicateDomain);
        }
        if self.surfaces.len() == self.config.max_surfaces {
            return Err(SurfaceRuntimeError::SurfaceCapacity);
        }
        let id = GameSurfaceId {
            engine: self.engine,
            surface,
            domain,
        };
        backend
            .attach_surface(id, metrics)
            .map_err(map_backend_failure)?;
        self.surfaces.insert(
            surface,
            SurfaceRecord {
                id,
                state: GameSurfaceState::Active,
                metrics,
                next_pulse_id: 1,
                next_frame_id: 1,
                rendered_revision: 0,
                observed_control_revision: 0,
                render_endpoint,
                device_generation,
                pending_remote: None,
            },
        );
        self.domains.insert(domain, surface);
        Ok(id)
    }

    pub fn attach_remote(
        &mut self,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        render_endpoint: Handle,
        device_generation: u64,
    ) -> Result<GameSurfaceId, SurfaceRuntimeError> {
        self.ensure_open()?;
        if !surface.is_valid()
            || !domain.handle.is_valid()
            || !metrics.is_valid()
            || !render_endpoint.is_valid()
            || device_generation == 0
        {
            return Err(SurfaceRuntimeError::InvalidSurface);
        }
        if domain.engine != self.engine {
            return Err(SurfaceRuntimeError::WrongEngine);
        }
        if self.surfaces.contains_key(&surface) {
            return Err(SurfaceRuntimeError::DuplicateSurface);
        }
        if self.domains.contains_key(&domain) {
            return Err(SurfaceRuntimeError::DuplicateDomain);
        }
        if self.surfaces.len() == self.config.max_surfaces {
            return Err(SurfaceRuntimeError::SurfaceCapacity);
        }
        let id = GameSurfaceId {
            engine: self.engine,
            surface,
            domain,
        };
        self.surfaces.insert(
            surface,
            SurfaceRecord {
                id,
                state: GameSurfaceState::Active,
                metrics,
                next_pulse_id: 1,
                next_frame_id: 1,
                rendered_revision: 0,
                observed_control_revision: 0,
                render_endpoint,
                device_generation,
                pending_remote: None,
            },
        );
        self.domains.insert(domain, surface);
        Ok(id)
    }

    pub fn resize_remote(
        &mut self,
        id: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), SurfaceRuntimeError> {
        if !metrics.is_valid() {
            return Err(SurfaceRuntimeError::InvalidSurface);
        }
        let record = self.record(id)?;
        if record.pending_remote.is_some()
            || !matches!(
                record.state,
                GameSurfaceState::Active | GameSurfaceState::Suspended
            )
        {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        self.record_mut(id)?.metrics = metrics;
        Ok(())
    }

    pub fn resize(
        &mut self,
        id: GameSurfaceId,
        metrics: SurfaceMetrics,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), SurfaceRuntimeError> {
        if !metrics.is_valid() {
            return Err(SurfaceRuntimeError::InvalidSurface);
        }
        let record = self.record(id)?;
        if !matches!(
            record.state,
            GameSurfaceState::Active | GameSurfaceState::Suspended
        ) {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        backend
            .resize_surface(id, metrics)
            .map_err(map_backend_failure)?;
        self.record_mut(id)?.metrics = metrics;
        Ok(())
    }

    pub fn suspend(&mut self, id: GameSurfaceId) -> Result<(), SurfaceRuntimeError> {
        let record = self.record_mut(id)?;
        if record.state != GameSurfaceState::Active {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        record.state = GameSurfaceState::Suspended;
        Ok(())
    }

    pub fn resume(&mut self, id: GameSurfaceId) -> Result<(), SurfaceRuntimeError> {
        let record = self.record_mut(id)?;
        if record.state != GameSurfaceState::Suspended {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        record.state = GameSurfaceState::Active;
        Ok(())
    }

    pub fn begin_pulse(
        &mut self,
        id: GameSurfaceId,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentationPulse, SurfaceRuntimeError> {
        let record = self.record(id)?;
        self.begin_exact_pulse(
            id,
            record.next_pulse_id,
            now_micros,
            deadline_micros,
            record.metrics,
        )
    }

    pub fn begin_exact_pulse(
        &mut self,
        id: GameSurfaceId,
        pulse_id: u64,
        now_micros: u64,
        deadline_micros: u64,
        metrics: SurfaceMetrics,
    ) -> Result<PresentationPulse, SurfaceRuntimeError> {
        let record = self.record(id)?;
        if !matches!(
            record.state,
            GameSurfaceState::Active | GameSurfaceState::Suspended
        ) {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if deadline_micros < now_micros {
            return Err(SurfaceRuntimeError::DeadlineExpired);
        }
        if pulse_id != record.next_pulse_id || metrics != record.metrics {
            return Err(SurfaceRuntimeError::PulseSequence);
        }
        let next_pulse_id = pulse_id
            .checked_add(1)
            .ok_or(SurfaceRuntimeError::GenerationExhausted)?;
        self.record_mut(id)?.next_pulse_id = next_pulse_id;
        Ok(PresentationPulse {
            surface: id,
            pulse_id,
            deadline_micros,
            metrics,
        })
    }

    pub fn begin_scheduled_pulse(
        &mut self,
        id: GameSurfaceId,
        pulse_id: u64,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentationPulse, SurfaceRuntimeError> {
        let record = self.record(id)?;
        if !matches!(
            record.state,
            GameSurfaceState::Active | GameSurfaceState::Suspended
        ) {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if deadline_micros < now_micros {
            return Err(SurfaceRuntimeError::DeadlineExpired);
        }
        if pulse_id < record.next_pulse_id {
            return Err(SurfaceRuntimeError::PulseSequence);
        }
        let next_pulse_id = pulse_id
            .checked_add(1)
            .ok_or(SurfaceRuntimeError::GenerationExhausted)?;
        let metrics = record.metrics;
        self.record_mut(id)?.next_pulse_id = next_pulse_id;
        Ok(PresentationPulse {
            surface: id,
            pulse_id,
            deadline_micros,
            metrics,
        })
    }

    pub fn render_and_present(
        &mut self,
        pulse: PresentationPulse,
        now_micros: u64,
        required_render_revision: u64,
        required_control_revision: u64,
        graph_signature: u64,
        commands: Vec<u8>,
        backend: &mut dyn RenderBackend,
    ) -> Result<PresentOutcome, SurfaceRuntimeError> {
        if commands.len() > self.config.max_command_bytes {
            return Err(SurfaceRuntimeError::CommandCapacity);
        }
        if now_micros > pulse.deadline_micros {
            return Ok(PresentOutcome::DeadlineMissed);
        }
        let record = self.record(pulse.surface)?;
        if record.pending_remote.is_some() {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if record.state == GameSurfaceState::Suspended {
            return Ok(PresentOutcome::Suspended);
        }
        if record.state != GameSurfaceState::Active {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if pulse.pulse_id.checked_add(1) != Some(record.next_pulse_id) {
            return Err(SurfaceRuntimeError::PulseSequence);
        }
        if required_render_revision < record.rendered_revision {
            return Err(SurfaceRuntimeError::RenderBarrier);
        }
        if required_control_revision < record.observed_control_revision {
            return Err(SurfaceRuntimeError::ControlBarrier);
        }
        let frame_id = record.next_frame_id;
        let submission = RenderFrameSubmission {
            surface: pulse.surface,
            pulse_id: pulse.pulse_id,
            frame_id,
            render_endpoint: record.render_endpoint,
            device_generation: record.device_generation,
            required_render_revision,
            required_control_revision,
            graph_signature,
            commands,
        };
        let ack = match backend.render(&submission) {
            Ok(ack) => ack,
            Err(RenderBackendFailure::SurfaceLost) => {
                self.record_mut(pulse.surface)?.state = GameSurfaceState::Lost;
                return Ok(PresentOutcome::SurfaceLost);
            }
            Err(RenderBackendFailure::DeviceLost) => return Ok(PresentOutcome::DeviceLost),
            Err(error) => return Err(map_backend_failure(error)),
        };
        if ack.surface != submission.surface
            || ack.pulse_id != submission.pulse_id
            || ack.frame_id != submission.frame_id
            || ack.render_endpoint != submission.render_endpoint
            || ack.device_generation != submission.device_generation
            || ack.rendered_revision < submission.required_render_revision
            || ack.observed_control_revision < submission.required_control_revision
        {
            return Err(SurfaceRuntimeError::AckMismatch);
        }
        let outcome = backend
            .present(ack, now_micros, pulse.deadline_micros)
            .map_err(map_backend_failure)?;
        let record = self.record_mut(pulse.surface)?;
        record.next_frame_id = frame_id
            .checked_add(1)
            .ok_or(SurfaceRuntimeError::GenerationExhausted)?;
        record.rendered_revision = ack.rendered_revision;
        record.observed_control_revision = ack.observed_control_revision;
        match outcome {
            PresentOutcome::SurfaceLost => record.state = GameSurfaceState::Lost,
            PresentOutcome::DeviceLost
            | PresentOutcome::Presented
            | PresentOutcome::DeadlineMissed
            | PresentOutcome::Suspended => {}
        }
        Ok(outcome)
    }

    pub fn prepare_remote_frame(
        &mut self,
        pulse: PresentationPulse,
        now_micros: u64,
        required_render_revision: u64,
        required_control_revision: u64,
        graph_signature: u64,
        commands: Vec<u8>,
    ) -> Result<RemoteFramePreparation, SurfaceRuntimeError> {
        if commands.len() > self.config.max_command_bytes {
            return Err(SurfaceRuntimeError::CommandCapacity);
        }
        if now_micros > pulse.deadline_micros {
            return Ok(RemoteFramePreparation::Terminal(
                PresentOutcome::DeadlineMissed,
            ));
        }
        let record = self.record(pulse.surface)?;
        if record.pending_remote.is_some() {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if record.state == GameSurfaceState::Suspended {
            return Ok(RemoteFramePreparation::Terminal(PresentOutcome::Suspended));
        }
        if record.state != GameSurfaceState::Active {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if pulse.pulse_id.checked_add(1) != Some(record.next_pulse_id) {
            return Err(SurfaceRuntimeError::PulseSequence);
        }
        if required_render_revision < record.rendered_revision {
            return Err(SurfaceRuntimeError::RenderBarrier);
        }
        if required_control_revision < record.observed_control_revision {
            return Err(SurfaceRuntimeError::ControlBarrier);
        }
        let submission = RenderFrameSubmission {
            surface: pulse.surface,
            pulse_id: pulse.pulse_id,
            frame_id: record.next_frame_id,
            render_endpoint: record.render_endpoint,
            device_generation: record.device_generation,
            required_render_revision,
            required_control_revision,
            graph_signature,
            commands,
        };
        self.record_mut(pulse.surface)?.pending_remote = Some(submission.clone());
        Ok(RemoteFramePreparation::Submit(submission))
    }

    pub fn complete_remote_frame(
        &mut self,
        ack: RenderFrameAck,
        outcome: RemoteFrameOutcome,
    ) -> Result<PresentOutcome, SurfaceRuntimeError> {
        let pending = self
            .record(ack.surface)?
            .pending_remote
            .as_ref()
            .ok_or(SurfaceRuntimeError::AckMismatch)?;
        if ack.surface != pending.surface
            || ack.pulse_id != pending.pulse_id
            || ack.frame_id != pending.frame_id
            || ack.render_endpoint != pending.render_endpoint
            || ack.device_generation != pending.device_generation
            || ack.rendered_revision < pending.required_render_revision
            || ack.observed_control_revision < pending.required_control_revision
        {
            return Err(SurfaceRuntimeError::AckMismatch);
        }
        let record = self.record_mut(ack.surface)?;
        record.pending_remote = None;
        if outcome == RemoteFrameOutcome::RejectedBeforeSubmit {
            return Err(SurfaceRuntimeError::BackendRejected);
        }
        record.next_frame_id = ack
            .frame_id
            .checked_add(1)
            .ok_or(SurfaceRuntimeError::GenerationExhausted)?;
        let terminal = match outcome {
            RemoteFrameOutcome::Presented => {
                record.rendered_revision = ack.rendered_revision;
                record.observed_control_revision = ack.observed_control_revision;
                PresentOutcome::Presented
            }
            RemoteFrameOutcome::DeadlineMissed => {
                record.rendered_revision = ack.rendered_revision;
                record.observed_control_revision = ack.observed_control_revision;
                PresentOutcome::DeadlineMissed
            }
            RemoteFrameOutcome::Suspended => PresentOutcome::Suspended,
            RemoteFrameOutcome::SurfaceLost => {
                record.state = GameSurfaceState::Lost;
                PresentOutcome::SurfaceLost
            }
            RemoteFrameOutcome::DeviceLost => PresentOutcome::DeviceLost,
            RemoteFrameOutcome::OutcomeUnknown => {
                record.state = GameSurfaceState::Lost;
                return Err(SurfaceRuntimeError::BackendOutcomeUnknown);
            }
            RemoteFrameOutcome::RejectedBeforeSubmit => unreachable!(),
        };
        Ok(terminal)
    }

    pub fn rebind(
        &mut self,
        id: GameSurfaceId,
        replacement_surface: SurfaceHandle,
        render_endpoint: Handle,
        device_generation: u64,
        metrics: SurfaceMetrics,
        backend: &mut dyn RenderBackend,
    ) -> Result<GameSurfaceId, SurfaceRuntimeError> {
        let current = self.record(id)?;
        if current.state != GameSurfaceState::Lost {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if !replacement_surface.is_valid()
            || replacement_surface == id.surface
            || !render_endpoint.is_valid()
            || device_generation <= current.device_generation
            || !metrics.is_valid()
        {
            return Err(SurfaceRuntimeError::StaleSurface);
        }
        let replacement = GameSurfaceId {
            engine: self.engine,
            surface: replacement_surface,
            domain: id.domain,
        };
        backend
            .attach_surface(replacement, metrics)
            .map_err(map_backend_failure)?;
        let mut current = self.surfaces.remove(&id.surface).unwrap();
        current.id = replacement;
        current.state = GameSurfaceState::Active;
        current.metrics = metrics;
        current.render_endpoint = render_endpoint;
        current.device_generation = device_generation;
        self.surfaces.insert(replacement_surface, current);
        self.domains.insert(id.domain, replacement_surface);
        Ok(replacement)
    }

    pub fn replace_render_binding(
        &mut self,
        id: GameSurfaceId,
        render_endpoint: Handle,
        device_generation: u64,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), SurfaceRuntimeError> {
        let current = self.record(id)?;
        if current.pending_remote.is_some() {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if current.render_endpoint == render_endpoint
            && current.device_generation == device_generation
        {
            return Ok(());
        }
        if !render_endpoint.is_valid()
            || render_endpoint.index != current.render_endpoint.index
            || render_endpoint.generation <= current.render_endpoint.generation
            || device_generation < current.device_generation
        {
            return Err(SurfaceRuntimeError::StaleRenderEndpoint);
        }
        let metrics = current.metrics;
        backend
            .rebind_surface(id, metrics)
            .map_err(map_backend_failure)?;
        let current = self.record_mut(id)?;
        current.render_endpoint = render_endpoint;
        current.device_generation = device_generation;
        current.rendered_revision = 0;
        current.observed_control_revision = 0;
        if current.state == GameSurfaceState::Lost {
            current.state = GameSurfaceState::Active;
        }
        Ok(())
    }

    pub fn recover_backend_surfaces(
        &mut self,
        backend: &mut dyn RenderBackend,
    ) -> Result<Vec<GameSurfaceId>, SurfaceRuntimeError> {
        if self
            .surfaces
            .values()
            .any(|record| record.pending_remote.is_some())
        {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        let surfaces = self
            .surfaces
            .values()
            .map(|record| (record.id, record.metrics))
            .collect::<Vec<_>>();
        for (id, metrics) in &surfaces {
            backend
                .rebind_surface(*id, *metrics)
                .map_err(map_backend_failure)?;
        }
        for (id, _) in &surfaces {
            let record = self.record_mut(*id)?;
            record.rendered_revision = 0;
            record.observed_control_revision = 0;
            record.pending_remote = None;
            if record.state == GameSurfaceState::Lost {
                record.state = GameSurfaceState::Active;
            }
        }
        Ok(surfaces.into_iter().map(|(id, _)| id).collect())
    }

    pub fn replace_remote_render_binding(
        &mut self,
        id: GameSurfaceId,
        render_endpoint: Handle,
        device_generation: u64,
    ) -> Result<(), SurfaceRuntimeError> {
        let current = self.record(id)?;
        if current.pending_remote.is_some() {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        if current.render_endpoint == render_endpoint
            && current.device_generation == device_generation
        {
            return Ok(());
        }
        if !render_endpoint.is_valid()
            || render_endpoint.index != current.render_endpoint.index
            || render_endpoint.generation <= current.render_endpoint.generation
            || device_generation < current.device_generation
        {
            return Err(SurfaceRuntimeError::StaleRenderEndpoint);
        }
        let current = self.record_mut(id)?;
        current.render_endpoint = render_endpoint;
        current.device_generation = device_generation;
        current.rendered_revision = 0;
        current.observed_control_revision = 0;
        if current.state == GameSurfaceState::Lost {
            current.state = GameSurfaceState::Active;
        }
        Ok(())
    }

    pub fn close(
        &mut self,
        id: GameSurfaceId,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), SurfaceRuntimeError> {
        let state = self.record(id)?.state;
        if matches!(state, GameSurfaceState::Closing | GameSurfaceState::Closed) {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        self.record_mut(id)?.state = GameSurfaceState::Closing;
        backend.detach_surface(id).map_err(map_backend_failure)?;
        self.surfaces.remove(&id.surface);
        self.domains.remove(&id.domain);
        Ok(())
    }

    pub fn close_remote(&mut self, id: GameSurfaceId) -> Result<(), SurfaceRuntimeError> {
        let record = self.record(id)?;
        if record.pending_remote.is_some()
            || matches!(
                record.state,
                GameSurfaceState::Closing | GameSurfaceState::Closed
            )
        {
            return Err(SurfaceRuntimeError::InvalidState);
        }
        self.surfaces.remove(&id.surface);
        self.domains.remove(&id.domain);
        Ok(())
    }

    pub(crate) fn close_remote_on_shutdown(
        &mut self,
        id: GameSurfaceId,
    ) -> Result<(), SurfaceRuntimeError> {
        self.record(id)?;
        self.surfaces.remove(&id.surface);
        self.domains.remove(&id.domain);
        Ok(())
    }

    pub(crate) fn abandon_surface(&mut self, id: GameSurfaceId) {
        self.surfaces.remove(&id.surface);
        self.domains.remove(&id.domain);
    }

    pub fn remote_frame_pending(&self, id: GameSurfaceId) -> Result<bool, SurfaceRuntimeError> {
        Ok(self.record(id)?.pending_remote.is_some())
    }

    pub fn metrics(&self, id: GameSurfaceId) -> Result<SurfaceMetrics, SurfaceRuntimeError> {
        Ok(self.record(id)?.metrics)
    }

    fn record(&self, id: GameSurfaceId) -> Result<&SurfaceRecord, SurfaceRuntimeError> {
        self.ensure_open()?;
        if id.engine != self.engine {
            return Err(SurfaceRuntimeError::WrongEngine);
        }
        self.surfaces
            .get(&id.surface)
            .filter(|record| record.id == id)
            .ok_or(SurfaceRuntimeError::StaleSurface)
    }

    fn record_mut(&mut self, id: GameSurfaceId) -> Result<&mut SurfaceRecord, SurfaceRuntimeError> {
        self.ensure_open()?;
        if id.engine != self.engine {
            return Err(SurfaceRuntimeError::WrongEngine);
        }
        self.surfaces
            .get_mut(&id.surface)
            .filter(|record| record.id == id)
            .ok_or(SurfaceRuntimeError::StaleSurface)
    }

    fn ensure_open(&self) -> Result<(), SurfaceRuntimeError> {
        if self.closed {
            Err(SurfaceRuntimeError::Closed)
        } else {
            Ok(())
        }
    }
}

fn map_backend_failure(error: RenderBackendFailure) -> SurfaceRuntimeError {
    match error {
        RenderBackendFailure::RejectedBeforeSubmit | RenderBackendFailure::SurfaceLost => {
            SurfaceRuntimeError::BackendRejected
        }
        RenderBackendFailure::OutcomeUnknown | RenderBackendFailure::DeviceLost => {
            SurfaceRuntimeError::BackendOutcomeUnknown
        }
    }
}
