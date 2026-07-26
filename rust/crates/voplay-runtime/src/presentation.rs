use vo_app_protocol::SurfaceHandle;
use voplay_protocol::{EngineId, Handle};

use crate::{
    outbox::{DurablePoll, OutboxError, PresentationDomainId, RenderOutbox, RenderTransient},
    render_control::RenderControlFramePlan,
    surface::{
        GameSurfaceId, PresentOutcome, PresentationPulse, RemoteFrameOutcome,
        RemoteFramePreparation, RenderBackend, RenderFrameAck, SurfaceMetrics, SurfaceRuntime,
        SurfaceRuntimeConfig, SurfaceRuntimeError,
    },
    Engine, EngineConfig, EngineError, RenderCommitAck, RenderEntity, RenderStateSnapshot,
    RenderStateTransaction,
};

#[derive(Clone, Copy, Debug)]
pub struct RenderObjectView<'a> {
    pub entity: RenderEntity,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderFrameDelta {
    pub revision: u64,
    pub full_snapshot: bool,
    pub objects: Vec<(RenderEntity, Vec<u8>)>,
    pub despawned: Vec<RenderEntity>,
}

#[derive(Clone, Debug)]
pub struct FrameEncodeRequest<'a> {
    pub engine: EngineId,
    pub domain: PresentationDomainId,
    pub pulse_id: u64,
    pub render_revision: u64,
    pub control_revision: u64,
    pub graph_signature: u64,
    pub control_frame: Option<&'a RenderControlFramePlan>,
    pub full_snapshot: bool,
    pub objects: Vec<RenderObjectView<'a>>,
    pub despawned: &'a [RenderEntity],
    pub transient: Option<&'a RenderTransient>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameEncoderError {
    Closed,
    InvalidInput,
    Capacity,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameEncoderOwnerSnapshot {
    pub closed: bool,
    pub retained_objects: usize,
    pub retained_assets: usize,
    pub retained_bytes: usize,
}

pub trait FrameCommandEncoder {
    fn owner_snapshot(&self) -> FrameEncoderOwnerSnapshot {
        FrameEncoderOwnerSnapshot::default()
    }

    fn shutdown(&mut self) {}

    fn register_render_feature(&mut self, _bytes: &[u8]) -> Result<(), FrameEncoderError> {
        Err(FrameEncoderError::Unsupported)
    }

    fn upsert_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
        _bytes: &[u8],
    ) -> Result<(), FrameEncoderError> {
        Ok(())
    }

    fn remove_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
    ) -> Result<(), FrameEncoderError> {
        Ok(())
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationRuntimeConfig {
    pub engine: EngineConfig,
    pub surfaces: SurfaceRuntimeConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresentationRuntimeError {
    Engine(EngineError),
    Surface(SurfaceRuntimeError),
    Outbox(OutboxError),
    Encoder(FrameEncoderError),
    WrongEngine,
    WrongDomain,
    FrameSequence,
    RenderBarrier,
    ControlBarrier,
}

impl From<EngineError> for PresentationRuntimeError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<SurfaceRuntimeError> for PresentationRuntimeError {
    fn from(error: SurfaceRuntimeError) -> Self {
        Self::Surface(error)
    }
}

impl From<OutboxError> for PresentationRuntimeError {
    fn from(error: OutboxError) -> Self {
        Self::Outbox(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderSyncOutcome {
    Idle,
    Applied { revision: u64 },
    Inflight,
    SnapshotRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutboxPresentOutcome {
    pub synchronization: RenderSyncOutcome,
    pub presentation: PresentOutcome,
}

pub struct PresentationRuntime {
    engine: Engine,
    surfaces: SurfaceRuntime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationRuntimeOwnerSnapshot {
    pub engine: crate::RenderEngineOwnerSnapshot,
    pub surfaces: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationRuntimeShutdownReport {
    pub engine: crate::RenderEngineShutdownReport,
    pub abandoned_surfaces: usize,
}

impl PresentationRuntime {
    pub fn new(
        engine: EngineId,
        config: PresentationRuntimeConfig,
    ) -> Result<Self, PresentationRuntimeError> {
        Ok(Self {
            engine: Engine::new(engine, config.engine)?,
            surfaces: SurfaceRuntime::new(engine, config.surfaces)?,
        })
    }

    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn owner_snapshot(&self) -> PresentationRuntimeOwnerSnapshot {
        PresentationRuntimeOwnerSnapshot {
            engine: self.engine.owner_snapshot(),
            surfaces: self.surfaces.live_surface_count(),
        }
    }

    pub fn shutdown(
        &mut self,
    ) -> Result<PresentationRuntimeShutdownReport, PresentationRuntimeError> {
        self.engine.preflight_shutdown()?;
        let abandoned_surfaces = self.surfaces.shutdown();
        let engine = self.engine.shutdown()?;
        Ok(PresentationRuntimeShutdownReport {
            engine,
            abandoned_surfaces,
        })
    }

    pub fn apply_render_state(
        &mut self,
        transaction: RenderStateTransaction,
    ) -> Result<RenderCommitAck, PresentationRuntimeError> {
        Ok(self.engine.apply_render_state(transaction)?)
    }

    pub fn apply_render_snapshot(
        &mut self,
        snapshot: RenderStateSnapshot,
    ) -> Result<RenderCommitAck, PresentationRuntimeError> {
        Ok(self.engine.apply_render_snapshot(snapshot)?)
    }

    pub fn set_control_revision(&mut self, revision: u64) {
        self.engine.set_render_control_revision(revision);
    }

    pub fn restart_renderer(
        &mut self,
    ) -> Result<Vec<crate::RenderEventResult>, PresentationRuntimeError> {
        Ok(self.engine.restart_renderer()?)
    }

    pub fn synchronize_outbox(
        &mut self,
        outbox: &mut RenderOutbox,
    ) -> Result<RenderSyncOutcome, PresentationRuntimeError> {
        match outbox.poll_durable()? {
            Ok(transaction) => {
                let ack = self.apply_render_state(transaction)?;
                let revision = ack.revision;
                outbox.acknowledge(ack)?;
                Ok(RenderSyncOutcome::Applied { revision })
            }
            Err(DurablePoll::Idle) => Ok(RenderSyncOutcome::Idle),
            Err(DurablePoll::Inflight) => Ok(RenderSyncOutcome::Inflight),
            Err(DurablePoll::SnapshotRequired) => Ok(RenderSyncOutcome::SnapshotRequired),
        }
    }

    pub fn attach_surface(
        &mut self,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        render_endpoint: Handle,
        device_generation: u64,
        backend: &mut dyn RenderBackend,
    ) -> Result<GameSurfaceId, PresentationRuntimeError> {
        Ok(self.surfaces.attach(
            surface,
            domain,
            metrics,
            render_endpoint,
            device_generation,
            backend,
        )?)
    }

    pub fn attach_surface_remote(
        &mut self,
        surface: SurfaceHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        render_endpoint: Handle,
        device_generation: u64,
    ) -> Result<GameSurfaceId, PresentationRuntimeError> {
        Ok(self.surfaces.attach_remote(
            surface,
            domain,
            metrics,
            render_endpoint,
            device_generation,
        )?)
    }

    pub fn resize_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.resize(surface, metrics, backend)?)
    }

    pub fn resize_surface_remote(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.resize_remote(surface, metrics)?)
    }

    pub fn replace_surface_render_binding(
        &mut self,
        surface: GameSurfaceId,
        render_endpoint: Handle,
        device_generation: u64,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.replace_render_binding(
            surface,
            render_endpoint,
            device_generation,
            backend,
        )?)
    }

    pub fn replace_surface_render_binding_remote(
        &mut self,
        surface: GameSurfaceId,
        render_endpoint: Handle,
        device_generation: u64,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.replace_remote_render_binding(
            surface,
            render_endpoint,
            device_generation,
        )?)
    }

    pub fn begin_pulse(
        &mut self,
        surface: GameSurfaceId,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentationPulse, PresentationRuntimeError> {
        Ok(self
            .surfaces
            .begin_pulse(surface, now_micros, deadline_micros)?)
    }

    pub fn begin_scheduled_pulse(
        &mut self,
        surface: GameSurfaceId,
        pulse_id: u64,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentationPulse, PresentationRuntimeError> {
        Ok(self
            .surfaces
            .begin_scheduled_pulse(surface, pulse_id, now_micros, deadline_micros)?)
    }

    pub fn present(
        &mut self,
        pulse: PresentationPulse,
        now_micros: u64,
        graph_signature: u64,
        transient: Option<RenderTransient>,
        encoder: &mut dyn FrameCommandEncoder,
        backend: &mut dyn RenderBackend,
    ) -> Result<PresentOutcome, PresentationRuntimeError> {
        if pulse.surface.engine != self.engine.id() {
            return Err(PresentationRuntimeError::WrongEngine);
        }
        if let Some(transient) = &transient {
            if transient.domain != pulse.surface.domain {
                return Err(PresentationRuntimeError::WrongDomain);
            }
            if transient.frame_id != pulse.pulse_id {
                return Err(PresentationRuntimeError::FrameSequence);
            }
            if transient.required_render_revision > self.engine.render_revision() {
                return Err(PresentationRuntimeError::RenderBarrier);
            }
            if transient.required_control_revision > self.engine.render_control_revision() {
                return Err(PresentationRuntimeError::ControlBarrier);
            }
        }
        let delta = self.engine.prepare_render_delta();
        let objects = delta
            .objects
            .iter()
            .map(|(entity, bytes)| RenderObjectView {
                entity: *entity,
                bytes: bytes.as_slice(),
            })
            .collect();
        let commands = encoder
            .encode(FrameEncodeRequest {
                engine: self.engine.id(),
                domain: pulse.surface.domain,
                pulse_id: pulse.pulse_id,
                render_revision: self.engine.render_revision(),
                control_revision: self.engine.render_control_revision(),
                graph_signature,
                control_frame: None,
                full_snapshot: delta.full_snapshot,
                objects,
                despawned: &delta.despawned,
                transient: transient.as_ref(),
            })
            .map_err(PresentationRuntimeError::Encoder)?;
        self.engine.commit_render_delta(delta.revision)?;
        Ok(self.surfaces.render_and_present(
            pulse,
            now_micros,
            self.engine.render_revision(),
            self.engine.render_control_revision(),
            graph_signature,
            commands,
            backend,
        )?)
    }

    pub fn prepare_remote_present(
        &mut self,
        pulse: PresentationPulse,
        now_micros: u64,
        graph_signature: u64,
        transient: Option<RenderTransient>,
        encoder: &mut dyn FrameCommandEncoder,
    ) -> Result<RemoteFramePreparation, PresentationRuntimeError> {
        if pulse.surface.engine != self.engine.id() {
            return Err(PresentationRuntimeError::WrongEngine);
        }
        if let Some(transient) = &transient {
            if transient.domain != pulse.surface.domain {
                return Err(PresentationRuntimeError::WrongDomain);
            }
            if transient.frame_id != pulse.pulse_id {
                return Err(PresentationRuntimeError::FrameSequence);
            }
            if transient.required_render_revision > self.engine.render_revision() {
                return Err(PresentationRuntimeError::RenderBarrier);
            }
            if transient.required_control_revision > self.engine.render_control_revision() {
                return Err(PresentationRuntimeError::ControlBarrier);
            }
        }
        let delta = self.engine.prepare_render_delta();
        let objects = delta
            .objects
            .iter()
            .map(|(entity, bytes)| RenderObjectView {
                entity: *entity,
                bytes: bytes.as_slice(),
            })
            .collect();
        let commands = encoder
            .encode(FrameEncodeRequest {
                engine: self.engine.id(),
                domain: pulse.surface.domain,
                pulse_id: pulse.pulse_id,
                render_revision: self.engine.render_revision(),
                control_revision: self.engine.render_control_revision(),
                graph_signature,
                control_frame: None,
                full_snapshot: delta.full_snapshot,
                objects,
                despawned: &delta.despawned,
                transient: transient.as_ref(),
            })
            .map_err(PresentationRuntimeError::Encoder)?;
        self.engine.commit_render_delta(delta.revision)?;
        Ok(self.surfaces.prepare_remote_frame(
            pulse,
            now_micros,
            self.engine.render_revision(),
            self.engine.render_control_revision(),
            graph_signature,
            commands,
        )?)
    }

    pub fn complete_remote_present(
        &mut self,
        ack: RenderFrameAck,
        outcome: RemoteFrameOutcome,
    ) -> Result<PresentOutcome, PresentationRuntimeError> {
        Ok(self.surfaces.complete_remote_frame(ack, outcome)?)
    }

    pub fn present_from_outbox(
        &mut self,
        pulse: PresentationPulse,
        now_micros: u64,
        graph_signature: u64,
        outbox: &mut RenderOutbox,
        encoder: &mut dyn FrameCommandEncoder,
        backend: &mut dyn RenderBackend,
    ) -> Result<OutboxPresentOutcome, PresentationRuntimeError> {
        let synchronization = self.synchronize_outbox(outbox)?;
        let transient = outbox.take_ready_transient_for_frame(
            pulse.surface.domain,
            pulse.pulse_id,
            self.engine.render_revision(),
            self.engine.render_control_revision(),
        );
        let presentation = self.present(
            pulse,
            now_micros,
            graph_signature,
            transient,
            encoder,
            backend,
        )?;
        Ok(OutboxPresentOutcome {
            synchronization,
            presentation,
        })
    }

    pub fn suspend_surface(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.suspend(surface)?)
    }

    pub fn resume_surface(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.resume(surface)?)
    }

    pub fn close_surface(
        &mut self,
        surface: GameSurfaceId,
        backend: &mut dyn RenderBackend,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.close(surface, backend)?)
    }

    pub fn close_surface_remote(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.close_remote(surface)?)
    }

    pub(crate) fn close_surface_remote_on_shutdown(
        &mut self,
        surface: GameSurfaceId,
    ) -> Result<(), PresentationRuntimeError> {
        Ok(self.surfaces.close_remote_on_shutdown(surface)?)
    }

    pub(crate) fn abandon_surface(&mut self, surface: GameSurfaceId) {
        self.surfaces.abandon_surface(surface);
    }

    pub fn remote_frame_pending(
        &self,
        surface: GameSurfaceId,
    ) -> Result<bool, PresentationRuntimeError> {
        Ok(self.surfaces.remote_frame_pending(surface)?)
    }

    pub fn surface_metrics(
        &self,
        surface: GameSurfaceId,
    ) -> Result<SurfaceMetrics, PresentationRuntimeError> {
        Ok(self.surfaces.metrics(surface)?)
    }
}
