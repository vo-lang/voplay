use std::collections::{BTreeMap, BTreeSet};

use crate::surface::{
    GameSurfaceId, PresentOutcome, RenderBackend, RenderBackendFailure, RenderBackendReadback,
    RenderFrameAck, RenderFrameSubmission, SurfaceMetrics,
};
use crate::{
    control::StableControlRef,
    render_readback::{ReadbackFormat, ReadbackRegion, ReadbackRequestId},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformSubmission {
    pub fence_value: u64,
    pub device_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformAdapterError {
    RejectedBeforeSubmit,
    OutcomeUnknown,
    SurfaceLost,
    DeviceLost,
}

pub trait PlatformSurfaceAdapter {
    fn shutdown(&mut self) -> Result<(), PlatformAdapterError> {
        Ok(())
    }

    fn upload_rgba_texture(
        &mut self,
        _texture: u64,
        _width: u32,
        _height: u32,
        _pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn remove_texture(&mut self, _texture: u64) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn upsert_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
        _bytes: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn remove_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn synchronize_render_targets(
        &mut self,
        _targets: &[StableControlRef],
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn request_readback(
        &mut self,
        _request: ReadbackRequestId,
        _target: StableControlRef,
        _expected_target_revision: u64,
        _region: ReadbackRegion,
        _format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn poll_readback(
        &mut self,
        _request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        Ok(None)
    }
    fn cancel_readback(&mut self, _request: ReadbackRequestId) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn attach(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError>;
    fn resize(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError>;
    fn rebind(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError>;
    fn submit(
        &mut self,
        frame: &RenderFrameSubmission,
    ) -> Result<PlatformSubmission, PlatformAdapterError>;
    fn present(
        &mut self,
        surface: GameSurfaceId,
        submission: PlatformSubmission,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, PlatformAdapterError>;
    fn detach(&mut self, surface: GameSurfaceId) -> Result<(), PlatformAdapterError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformRenderBackendConfig {
    pub max_surfaces: usize,
}

impl Default for PlatformRenderBackendConfig {
    fn default() -> Self {
        Self { max_surfaces: 16 }
    }
}

struct PlatformSurface {
    metrics: SurfaceMetrics,
    pending: Option<(RenderFrameAck, PlatformSubmission)>,
}

pub struct PlatformRenderBackend<A> {
    config: PlatformRenderBackendConfig,
    adapter: A,
    surfaces: BTreeMap<GameSurfaceId, PlatformSurface>,
    readbacks: BTreeSet<ReadbackRequestId>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformRenderBackendOwnerSnapshot {
    pub closed: bool,
    pub surfaces: usize,
    pub pending_frames: usize,
    pub readbacks: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformRenderBackendShutdownReport {
    pub before: PlatformRenderBackendOwnerSnapshot,
    pub cancelled_readbacks: Vec<ReadbackRequestId>,
    pub detached_surfaces: Vec<GameSurfaceId>,
    pub readback_cancel_failures: Vec<(ReadbackRequestId, PlatformAdapterError)>,
    pub surface_detach_failures: Vec<(GameSurfaceId, PlatformAdapterError)>,
    pub adapter_shutdown_failure: Option<PlatformAdapterError>,
    pub after: PlatformRenderBackendOwnerSnapshot,
}

impl<A: PlatformSurfaceAdapter> PlatformRenderBackend<A> {
    pub fn new(
        config: PlatformRenderBackendConfig,
        adapter: A,
    ) -> Result<Self, PlatformAdapterError> {
        if config.max_surfaces == 0 {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        Ok(Self {
            config,
            adapter,
            surfaces: BTreeMap::new(),
            readbacks: BTreeSet::new(),
            closed: false,
        })
    }

    pub fn adapter(&self) -> &A {
        &self.adapter
    }

    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
    }

    pub fn owner_snapshot(&self) -> PlatformRenderBackendOwnerSnapshot {
        PlatformRenderBackendOwnerSnapshot {
            closed: self.closed,
            surfaces: self.surfaces.len(),
            pending_frames: self
                .surfaces
                .values()
                .filter(|surface| surface.pending.is_some())
                .count(),
            readbacks: self.readbacks.len(),
        }
    }

    pub fn shutdown(&mut self) -> PlatformRenderBackendShutdownReport {
        let before = self.owner_snapshot();
        if self.closed {
            return PlatformRenderBackendShutdownReport {
                before,
                cancelled_readbacks: Vec::new(),
                detached_surfaces: Vec::new(),
                readback_cancel_failures: Vec::new(),
                surface_detach_failures: Vec::new(),
                adapter_shutdown_failure: None,
                after: before,
            };
        }
        let cancelled_readbacks = self.readbacks.iter().copied().collect::<Vec<_>>();
        let detached_surfaces = self.surfaces.keys().copied().collect::<Vec<_>>();
        let mut readback_cancel_failures = Vec::new();
        for request in &cancelled_readbacks {
            if let Err(error) = self.adapter.cancel_readback(*request) {
                readback_cancel_failures.push((*request, error));
            }
        }
        let mut surface_detach_failures = Vec::new();
        for surface in &detached_surfaces {
            if let Err(error) = self.adapter.detach(*surface) {
                surface_detach_failures.push((*surface, error));
            }
        }
        self.readbacks.clear();
        self.surfaces.clear();
        let adapter_shutdown_failure = self.adapter.shutdown().err();
        self.closed = true;
        PlatformRenderBackendShutdownReport {
            before,
            cancelled_readbacks,
            detached_surfaces,
            readback_cancel_failures,
            surface_detach_failures,
            adapter_shutdown_failure,
            after: self.owner_snapshot(),
        }
    }

    fn ensure_open(&self) -> Result<(), RenderBackendFailure> {
        if self.closed {
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        } else {
            Ok(())
        }
    }
}

impl<A: PlatformSurfaceAdapter> RenderBackend for PlatformRenderBackend<A> {
    fn shutdown(&mut self) -> Result<(), RenderBackendFailure> {
        PlatformRenderBackend::shutdown(self);
        Ok(())
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter
            .synchronize_render_targets(targets)
            .map_err(map_error)
    }
    fn upload_rgba_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter
            .upload_rgba_texture(texture, width, height, pixels)
            .map_err(map_error)
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter.remove_texture(texture).map_err(map_error)
    }

    fn upsert_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter
            .upsert_profile_asset(kind, asset, revision, bytes)
            .map_err(map_error)
    }

    fn remove_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter
            .remove_profile_asset(kind, asset, revision)
            .map_err(map_error)
    }

    fn request_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.adapter
            .request_readback(request, target, expected_target_revision, region, format)
            .map_err(map_error)?;
        self.readbacks.insert(request);
        Ok(())
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, RenderBackendFailure> {
        self.ensure_open()?;
        if !self.readbacks.contains(&request) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        match self.adapter.poll_readback(request).map_err(map_error) {
            Ok(Some(result)) => {
                self.readbacks.remove(&request);
                Ok(Some(result))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                self.readbacks.remove(&request);
                Err(error)
            }
        }
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if !self.readbacks.contains(&request) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.adapter.cancel_readback(request).map_err(map_error)?;
        self.readbacks.remove(&request);
        Ok(())
    }

    fn attach_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if self.surfaces.contains_key(&surface) || self.surfaces.len() == self.config.max_surfaces {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.adapter.attach(surface, metrics).map_err(map_error)?;
        self.surfaces.insert(
            surface,
            PlatformSurface {
                metrics,
                pending: None,
            },
        );
        Ok(())
    }

    fn resize_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let record = self
            .surfaces
            .get(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if record.pending.is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.adapter.resize(surface, metrics).map_err(map_error)?;
        self.surfaces
            .get_mut(&surface)
            .expect("surface remains live across synchronous adapter resize")
            .metrics = metrics;
        Ok(())
    }

    fn rebind_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.surfaces
            .get(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        self.adapter.rebind(surface, metrics).map_err(map_error)?;
        let record = self
            .surfaces
            .get_mut(&surface)
            .expect("surface remains live across synchronous adapter rebind");
        record.metrics = metrics;
        record.pending = None;
        Ok(())
    }

    fn render(
        &mut self,
        frame: &RenderFrameSubmission,
    ) -> Result<RenderFrameAck, RenderBackendFailure> {
        self.ensure_open()?;
        let record = self
            .surfaces
            .get(&frame.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if record.pending.is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let submission = self.adapter.submit(frame).map_err(map_error)?;
        if submission.device_generation != frame.device_generation {
            return Err(RenderBackendFailure::DeviceLost);
        }
        let ack = RenderFrameAck {
            surface: frame.surface,
            pulse_id: frame.pulse_id,
            frame_id: frame.frame_id,
            render_endpoint: frame.render_endpoint,
            device_generation: frame.device_generation,
            rendered_revision: frame.required_render_revision,
            observed_control_revision: frame.required_control_revision,
        };
        self.surfaces
            .get_mut(&frame.surface)
            .expect("surface remains live across synchronous adapter submit")
            .pending = Some((ack, submission));
        Ok(ack)
    }

    fn present(
        &mut self,
        ack: RenderFrameAck,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, RenderBackendFailure> {
        self.ensure_open()?;
        let record = self
            .surfaces
            .get(&ack.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        let (pending_ack, submission) = record
            .pending
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        if pending_ack != ack {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let outcome = self
            .adapter
            .present(ack.surface, submission, now_micros, deadline_micros)
            .map_err(map_error)?;
        self.surfaces
            .get_mut(&ack.surface)
            .expect("surface remains live across synchronous adapter present")
            .pending = None;
        Ok(outcome)
    }

    fn detach_surface(&mut self, surface: GameSurfaceId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let record = self
            .surfaces
            .get(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if record.pending.is_some() {
            return Err(RenderBackendFailure::OutcomeUnknown);
        }
        self.adapter.detach(surface).map_err(map_error)?;
        self.surfaces.remove(&surface);
        Ok(())
    }
}

fn map_error(error: PlatformAdapterError) -> RenderBackendFailure {
    match error {
        PlatformAdapterError::RejectedBeforeSubmit => RenderBackendFailure::RejectedBeforeSubmit,
        PlatformAdapterError::OutcomeUnknown => RenderBackendFailure::OutcomeUnknown,
        PlatformAdapterError::SurfaceLost => RenderBackendFailure::SurfaceLost,
        PlatformAdapterError::DeviceLost => RenderBackendFailure::DeviceLost,
    }
}
