use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, LockResult, Mutex, MutexGuard, TryLockError,
    },
};

use voplay_protocol::{EngineId, Handle};

use crate::surface::{
    GameSurfaceId, PresentOutcome, RenderBackend, RenderBackendFailure, RenderBackendReadback,
    RenderFrameAck, RenderFrameSubmission, SurfaceMetrics,
};
use crate::{
    control::{ControlKind, StableControlRef},
    render_readback::{ReadbackFormat, ReadbackRegion, ReadbackRequestId},
};

const MAGIC: &[u8; 4] = b"VHR1";
const RESULT_MAGIC: &[u8; 4] = b"VHR2";
const ROUTED_COMMAND_MAGIC: &[u8; 4] = b"VHR3";
const ROUTED_RESULT_MAGIC: &[u8; 4] = b"VHR4";
const MAX_HOST_RENDER_FRAME_BYTES: usize = 64 * 1024 * 1024;

pub fn encode_routed_host_render_command(
    engine: EngineId,
    command: &[u8],
) -> Result<Vec<u8>, RenderBackendFailure> {
    encode_host_render_route(ROUTED_COMMAND_MAGIC, engine, command, MAGIC)
}

pub fn decode_routed_host_render_command(
    bytes: &[u8],
) -> Result<(EngineId, &[u8]), RenderBackendFailure> {
    decode_host_render_route(bytes, ROUTED_COMMAND_MAGIC, MAGIC)
}

pub fn encode_routed_host_render_result(
    engine: EngineId,
    result: &[u8],
) -> Result<Vec<u8>, RenderBackendFailure> {
    encode_host_render_route(ROUTED_RESULT_MAGIC, engine, result, RESULT_MAGIC)
}

pub fn decode_routed_host_render_result(
    bytes: &[u8],
) -> Result<(EngineId, &[u8]), RenderBackendFailure> {
    decode_host_render_route(bytes, ROUTED_RESULT_MAGIC, RESULT_MAGIC)
}

fn encode_host_render_route(
    magic: &[u8; 4],
    engine: EngineId,
    payload: &[u8],
    payload_magic: &[u8; 4],
) -> Result<Vec<u8>, RenderBackendFailure> {
    if !engine.is_valid()
        || payload.get(..4) != Some(payload_magic)
        || payload.len() > MAX_HOST_RENDER_FRAME_BYTES.saturating_sub(12)
    {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    let mut bytes = Vec::with_capacity(12 + payload.len());
    bytes.extend_from_slice(magic);
    put_handle(&mut bytes, engine);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn decode_host_render_route<'a>(
    bytes: &'a [u8],
    magic: &[u8; 4],
    payload_magic: &[u8; 4],
) -> Result<(EngineId, &'a [u8]), RenderBackendFailure> {
    if bytes.len() < 16
        || bytes.len() > MAX_HOST_RENDER_FRAME_BYTES
        || bytes.get(..4) != Some(magic)
    {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    let engine = Handle {
        index: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        generation: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
    };
    let payload = &bytes[12..];
    if !engine.is_valid() || payload.get(..4) != Some(payload_magic) {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    Ok((engine, payload))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostRenderCommand {
    Attach {
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    },
    Resize {
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    },
    Rebind {
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    },
    UploadTexture {
        texture: u64,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    RemoveTexture {
        texture: u64,
    },
    UpsertProfileAsset {
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: Vec<u8>,
    },
    RemoveProfileAsset {
        kind: u32,
        asset: u64,
        revision: u64,
    },
    SynchronizeTargets {
        targets: Vec<StableControlRef>,
    },
    Render(RenderFrameSubmission),
    Present {
        ack: RenderFrameAck,
        now_micros: u64,
        deadline_micros: u64,
    },
    Detach {
        surface: GameSurfaceId,
    },
    RequestReadback {
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    },
    CancelReadback {
        request: ReadbackRequestId,
    },
    RequestFrameTrace {
        request: u64,
        frame_id: u64,
        graph_signature: u64,
        include_attachments: bool,
        include_shader_diagnostics: bool,
        max_bytes: usize,
    },
    CancelFrameTrace {
        request: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostRenderResult {
    Readback(RenderBackendReadback),
    ReadbackFailed {
        request: ReadbackRequestId,
        failure: RenderBackendFailure,
    },
    FrameTrace {
        request: u64,
        frame_id: u64,
        graph_signature: u64,
        bytes: Vec<u8>,
    },
    FrameTraceFailed {
        request: u64,
        failure: RenderBackendFailure,
    },
}

#[derive(Default)]
struct HostRenderSynchronizationCounters {
    lock_acquisitions: AtomicU64,
    lock_contentions: AtomicU64,
    poisoned_locks: AtomicU64,
    submitted_results: AtomicU64,
    submitted_result_bytes: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostRenderSynchronizationMetrics {
    pub lock_acquisitions: u64,
    pub lock_contentions: u64,
    pub poisoned_locks: u64,
    pub submitted_results: u64,
    pub submitted_result_bytes: u64,
    pub pending_readbacks: usize,
    pub ready_readbacks: usize,
    pub pending_frame_traces: usize,
    pub ready_frame_traces: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostRenderTransportMetrics {
    pub published_commands: u64,
    pub published_command_bytes: u64,
    pub synchronization: HostRenderSynchronizationMetrics,
}

struct InstrumentedMutex<T> {
    inner: Mutex<T>,
    counters: Arc<HostRenderSynchronizationCounters>,
}

impl<T> InstrumentedMutex<T> {
    fn new(value: T, counters: Arc<HostRenderSynchronizationCounters>) -> Self {
        Self {
            inner: Mutex::new(value),
            counters,
        }
    }

    fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        match self.inner.try_lock() {
            Ok(guard) => {
                self.counters
                    .lock_acquisitions
                    .fetch_add(1, Ordering::Relaxed);
                Ok(guard)
            }
            Err(TryLockError::WouldBlock) => {
                self.counters
                    .lock_contentions
                    .fetch_add(1, Ordering::Relaxed);
                let result = self.inner.lock();
                if result.is_ok() {
                    self.counters
                        .lock_acquisitions
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    self.counters.poisoned_locks.fetch_add(1, Ordering::Relaxed);
                }
                result
            }
            Err(TryLockError::Poisoned(_)) => {
                self.counters.poisoned_locks.fetch_add(1, Ordering::Relaxed);
                self.inner.lock()
            }
        }
    }
}

#[derive(Clone)]
pub struct HostRenderResultSink {
    results: Arc<InstrumentedMutex<BTreeMap<ReadbackRequestId, HostRenderResult>>>,
    pending: Arc<InstrumentedMutex<BTreeSet<ReadbackRequestId>>>,
    frame_trace_results: Arc<InstrumentedMutex<BTreeMap<u64, HostRenderResult>>>,
    pending_frame_traces: Arc<InstrumentedMutex<BTreeSet<u64>>>,
    synchronization: Arc<HostRenderSynchronizationCounters>,
    closed: Arc<AtomicBool>,
}

impl HostRenderResultSink {
    pub fn accepts(&self, bytes: &[u8]) -> Result<bool, RenderBackendFailure> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(false);
        }
        let result = decode_host_render_result(bytes)?;
        match result {
            HostRenderResult::Readback(readback) => self
                .pending
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)
                .map(|pending| pending.contains(&readback.request)),
            HostRenderResult::ReadbackFailed { request, .. } => self
                .pending
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)
                .map(|pending| pending.contains(&request)),
            HostRenderResult::FrameTrace { request, .. }
            | HostRenderResult::FrameTraceFailed { request, .. } => self
                .pending_frame_traces
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)
                .map(|pending| pending.contains(&request)),
        }
    }

    pub fn submit(&self, bytes: &[u8]) -> Result<(), RenderBackendFailure> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let result = decode_host_render_result(bytes)?;
        self.synchronization
            .submitted_results
            .fetch_add(1, Ordering::Relaxed);
        self.synchronization
            .submitted_result_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let request = match &result {
            HostRenderResult::Readback(readback) => readback.request,
            HostRenderResult::ReadbackFailed { request, .. } => *request,
            HostRenderResult::FrameTrace { .. } | HostRenderResult::FrameTraceFailed { .. } => {
                return self.submit_frame_trace(result);
            }
        };
        if !self
            .pending
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .contains(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let mut results = self
            .results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        if results.insert(request, result).is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(())
    }

    fn submit_frame_trace(&self, result: HostRenderResult) -> Result<(), RenderBackendFailure> {
        let request = match &result {
            HostRenderResult::FrameTrace { request, .. }
            | HostRenderResult::FrameTraceFailed { request, .. } => *request,
            _ => return Err(RenderBackendFailure::RejectedBeforeSubmit),
        };
        if !self
            .pending_frame_traces
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .contains(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let mut results = self
            .frame_trace_results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        if results.insert(request, result).is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(())
    }

    pub fn synchronization_metrics(&self) -> HostRenderSynchronizationMetrics {
        synchronization_metrics(
            &self.synchronization,
            &self.pending,
            &self.results,
            &self.pending_frame_traces,
            &self.frame_trace_results,
        )
    }
}

pub struct HostRenderBackend {
    publish: Box<dyn FnMut(&[u8]) -> Result<(), RenderBackendFailure>>,
    surfaces: BTreeMap<GameSurfaceId, (SurfaceMetrics, Option<RenderFrameAck>)>,
    max_surfaces: usize,
    pending_readbacks: Arc<InstrumentedMutex<BTreeSet<ReadbackRequestId>>>,
    readback_results: Arc<InstrumentedMutex<BTreeMap<ReadbackRequestId, HostRenderResult>>>,
    pending_frame_traces: Arc<InstrumentedMutex<BTreeSet<u64>>>,
    frame_trace_results: Arc<InstrumentedMutex<BTreeMap<u64, HostRenderResult>>>,
    synchronization: Arc<HostRenderSynchronizationCounters>,
    published_commands: u64,
    published_command_bytes: u64,
    closed: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostRenderBackendOwnerSnapshot {
    pub closed: bool,
    pub surfaces: usize,
    pub pending_frames: usize,
    pub synchronization: HostRenderSynchronizationMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostRenderBackendShutdownReport {
    pub before: HostRenderBackendOwnerSnapshot,
    pub cancelled_readbacks: usize,
    pub cancelled_frame_traces: usize,
    pub detached_surfaces: usize,
    pub publish_failures: usize,
    pub cleanup_failures: usize,
    pub after: HostRenderBackendOwnerSnapshot,
}

impl HostRenderBackend {
    pub fn new(
        max_surfaces: usize,
        publish: Box<dyn FnMut(&[u8]) -> Result<(), RenderBackendFailure>>,
    ) -> Result<Self, RenderBackendFailure> {
        Self::new_with_result_sink(max_surfaces, publish).map(|(backend, _)| backend)
    }

    pub fn new_with_result_sink(
        max_surfaces: usize,
        publish: Box<dyn FnMut(&[u8]) -> Result<(), RenderBackendFailure>>,
    ) -> Result<(Self, HostRenderResultSink), RenderBackendFailure> {
        if max_surfaces == 0 {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let synchronization = Arc::new(HostRenderSynchronizationCounters::default());
        let closed = Arc::new(AtomicBool::new(false));
        let readback_results = Arc::new(InstrumentedMutex::new(
            BTreeMap::new(),
            Arc::clone(&synchronization),
        ));
        let pending_readbacks = Arc::new(InstrumentedMutex::new(
            BTreeSet::new(),
            Arc::clone(&synchronization),
        ));
        let frame_trace_results = Arc::new(InstrumentedMutex::new(
            BTreeMap::new(),
            Arc::clone(&synchronization),
        ));
        let pending_frame_traces = Arc::new(InstrumentedMutex::new(
            BTreeSet::new(),
            Arc::clone(&synchronization),
        ));
        Ok((
            Self {
                publish,
                surfaces: BTreeMap::new(),
                max_surfaces,
                pending_readbacks: Arc::clone(&pending_readbacks),
                readback_results: Arc::clone(&readback_results),
                pending_frame_traces: Arc::clone(&pending_frame_traces),
                frame_trace_results: Arc::clone(&frame_trace_results),
                synchronization: Arc::clone(&synchronization),
                published_commands: 0,
                published_command_bytes: 0,
                closed: Arc::clone(&closed),
            },
            HostRenderResultSink {
                results: readback_results,
                pending: pending_readbacks,
                frame_trace_results,
                pending_frame_traces,
                synchronization,
                closed,
            },
        ))
    }

    fn publish(&mut self, command: HostRenderCommand) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.publish_unchecked(command)
    }

    fn publish_unchecked(
        &mut self,
        command: HostRenderCommand,
    ) -> Result<(), RenderBackendFailure> {
        let bytes = encode_host_render_command(&command)?;
        (self.publish)(&bytes)?;
        self.published_commands = self.published_commands.saturating_add(1);
        self.published_command_bytes = self
            .published_command_bytes
            .saturating_add(bytes.len() as u64);
        Ok(())
    }

    pub fn owner_snapshot(&self) -> HostRenderBackendOwnerSnapshot {
        HostRenderBackendOwnerSnapshot {
            closed: self.closed.load(Ordering::Acquire),
            surfaces: self.surfaces.len(),
            pending_frames: self
                .surfaces
                .values()
                .filter(|(_, pending)| pending.is_some())
                .count(),
            synchronization: self.synchronization_metrics(),
        }
    }

    pub fn shutdown(&mut self) -> HostRenderBackendShutdownReport {
        let before = self.owner_snapshot();
        if self.closed.swap(true, Ordering::AcqRel) {
            return HostRenderBackendShutdownReport {
                before,
                cancelled_readbacks: 0,
                cancelled_frame_traces: 0,
                detached_surfaces: 0,
                publish_failures: 0,
                cleanup_failures: 0,
                after: before,
            };
        }
        let readbacks = self
            .pending_readbacks
            .lock()
            .map_or_else(|_| Vec::new(), |pending| pending.iter().copied().collect());
        let frame_traces = self
            .pending_frame_traces
            .lock()
            .map_or_else(|_| Vec::new(), |pending| pending.iter().copied().collect());
        let surfaces = self.surfaces.keys().copied().collect::<Vec<_>>();
        let mut publish_failures = 0_usize;
        for request in &readbacks {
            publish_failures += self
                .publish_unchecked(HostRenderCommand::CancelReadback { request: *request })
                .is_err() as usize;
        }
        for request in &frame_traces {
            publish_failures += self
                .publish_unchecked(HostRenderCommand::CancelFrameTrace { request: *request })
                .is_err() as usize;
        }
        for surface in &surfaces {
            publish_failures += self
                .publish_unchecked(HostRenderCommand::Detach { surface: *surface })
                .is_err() as usize;
        }
        let mut cleanup_failures = 0_usize;
        cleanup_failures += clear_instrumented(&self.pending_readbacks).is_err() as usize;
        cleanup_failures += clear_instrumented(&self.readback_results).is_err() as usize;
        cleanup_failures += clear_instrumented(&self.pending_frame_traces).is_err() as usize;
        cleanup_failures += clear_instrumented(&self.frame_trace_results).is_err() as usize;
        self.surfaces.clear();
        HostRenderBackendShutdownReport {
            before,
            cancelled_readbacks: readbacks.len(),
            cancelled_frame_traces: frame_traces.len(),
            detached_surfaces: surfaces.len(),
            publish_failures,
            cleanup_failures,
            after: self.owner_snapshot(),
        }
    }

    fn ensure_open(&self) -> Result<(), RenderBackendFailure> {
        if self.closed.load(Ordering::Acquire) {
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        } else {
            Ok(())
        }
    }

    pub fn request_frame_trace(
        &mut self,
        request: u64,
        frame_id: u64,
        graph_signature: u64,
        include_attachments: bool,
        include_shader_diagnostics: bool,
        max_bytes: usize,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if request == 0
            || frame_id == 0
            || graph_signature == 0
            || max_bytes == 0
            || max_bytes > MAX_HOST_RENDER_FRAME_BYTES
            || !self
                .pending_frame_traces
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)?
                .insert(request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let command = HostRenderCommand::RequestFrameTrace {
            request,
            frame_id,
            graph_signature,
            include_attachments,
            include_shader_diagnostics,
            max_bytes,
        };
        if let Err(error) = self.publish(command) {
            self.pending_frame_traces
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)?
                .remove(&request);
            return Err(error);
        }
        Ok(())
    }

    pub fn poll_frame_trace(
        &mut self,
        request: u64,
    ) -> Result<Option<Vec<u8>>, RenderBackendFailure> {
        self.ensure_open()?;
        if !self
            .pending_frame_traces
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .contains(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let Some(result) = self
            .frame_trace_results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request)
        else {
            return Ok(None);
        };
        self.pending_frame_traces
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request);
        match result {
            HostRenderResult::FrameTrace { bytes, .. } => Ok(Some(bytes)),
            HostRenderResult::FrameTraceFailed { failure, .. } => Err(failure),
            _ => Err(RenderBackendFailure::RejectedBeforeSubmit),
        }
    }

    pub fn cancel_frame_trace(&mut self, request: u64) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if request == 0
            || !self
                .pending_frame_traces
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)?
                .remove(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.frame_trace_results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request);
        self.publish(HostRenderCommand::CancelFrameTrace { request })
    }

    pub fn synchronization_metrics(&self) -> HostRenderSynchronizationMetrics {
        synchronization_metrics(
            &self.synchronization,
            &self.pending_readbacks,
            &self.readback_results,
            &self.pending_frame_traces,
            &self.frame_trace_results,
        )
    }

    pub fn transport_metrics(&self) -> HostRenderTransportMetrics {
        HostRenderTransportMetrics {
            published_commands: self.published_commands,
            published_command_bytes: self.published_command_bytes,
            synchronization: self.synchronization_metrics(),
        }
    }
}

fn synchronization_metrics(
    counters: &HostRenderSynchronizationCounters,
    pending_readbacks: &InstrumentedMutex<BTreeSet<ReadbackRequestId>>,
    readback_results: &InstrumentedMutex<BTreeMap<ReadbackRequestId, HostRenderResult>>,
    pending_frame_traces: &InstrumentedMutex<BTreeSet<u64>>,
    frame_trace_results: &InstrumentedMutex<BTreeMap<u64, HostRenderResult>>,
) -> HostRenderSynchronizationMetrics {
    HostRenderSynchronizationMetrics {
        lock_acquisitions: counters.lock_acquisitions.load(Ordering::Relaxed),
        lock_contentions: counters.lock_contentions.load(Ordering::Relaxed),
        poisoned_locks: counters.poisoned_locks.load(Ordering::Relaxed),
        submitted_results: counters.submitted_results.load(Ordering::Relaxed),
        submitted_result_bytes: counters.submitted_result_bytes.load(Ordering::Relaxed),
        pending_readbacks: pending_readbacks.lock().map_or(0, |items| items.len()),
        ready_readbacks: readback_results.lock().map_or(0, |items| items.len()),
        pending_frame_traces: pending_frame_traces.lock().map_or(0, |items| items.len()),
        ready_frame_traces: frame_trace_results.lock().map_or(0, |items| items.len()),
    }
}

fn clear_instrumented<T: Default>(items: &InstrumentedMutex<T>) -> Result<(), ()> {
    let mut items = items.lock().map_err(|_| ())?;
    *items = T::default();
    Ok(())
}

impl RenderBackend for HostRenderBackend {
    fn shutdown(&mut self) -> Result<(), RenderBackendFailure> {
        HostRenderBackend::shutdown(self);
        Ok(())
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), RenderBackendFailure> {
        self.publish(HostRenderCommand::SynchronizeTargets {
            targets: targets.to_vec(),
        })
    }

    #[cfg(feature = "inspection")]
    fn request_frame_trace(
        &mut self,
        request: u64,
        frame_id: u64,
        graph_signature: u64,
        include_attachments: bool,
        include_shader_diagnostics: bool,
        max_bytes: usize,
    ) -> Result<(), RenderBackendFailure> {
        HostRenderBackend::request_frame_trace(
            self,
            request,
            frame_id,
            graph_signature,
            include_attachments,
            include_shader_diagnostics,
            max_bytes,
        )
    }

    #[cfg(feature = "inspection")]
    fn poll_frame_trace(&mut self, request: u64) -> Result<Option<Vec<u8>>, RenderBackendFailure> {
        HostRenderBackend::poll_frame_trace(self, request)
    }

    #[cfg(feature = "inspection")]
    fn cancel_frame_trace(&mut self, request: u64) -> Result<(), RenderBackendFailure> {
        HostRenderBackend::cancel_frame_trace(self, request)
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
        if request.0 == 0
            || target.kind != ControlKind::RenderTarget
            || expected_target_revision == 0
            || !self
                .pending_readbacks
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)?
                .insert(request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        if let Err(error) = self.publish(HostRenderCommand::RequestReadback {
            request,
            target,
            expected_target_revision,
            region,
            format,
        }) {
            self.pending_readbacks
                .lock()
                .map_err(|_| RenderBackendFailure::DeviceLost)?
                .remove(&request);
            return Err(error);
        }
        Ok(())
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, RenderBackendFailure> {
        self.ensure_open()?;
        if !self
            .pending_readbacks
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .contains(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let result = self
            .readback_results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request);
        let Some(result) = result else {
            return Ok(None);
        };
        self.pending_readbacks
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request);
        match result {
            HostRenderResult::Readback(readback) => Ok(Some(readback)),
            HostRenderResult::ReadbackFailed { failure, .. } => Err(failure),
            HostRenderResult::FrameTrace { .. } | HostRenderResult::FrameTraceFailed { .. } => {
                Err(RenderBackendFailure::RejectedBeforeSubmit)
            }
        }
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if !self
            .pending_readbacks
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.readback_results
            .lock()
            .map_err(|_| RenderBackendFailure::DeviceLost)?
            .remove(&request);
        self.publish(HostRenderCommand::CancelReadback { request })
    }

    fn upload_rgba_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        self.publish(HostRenderCommand::UploadTexture {
            texture,
            width,
            height,
            pixels: pixels.to_vec(),
        })
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), RenderBackendFailure> {
        self.publish(HostRenderCommand::RemoveTexture { texture })
    }

    fn upsert_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), RenderBackendFailure> {
        self.publish(HostRenderCommand::UpsertProfileAsset {
            kind,
            asset,
            revision,
            bytes: bytes.to_vec(),
        })
    }

    fn remove_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), RenderBackendFailure> {
        self.publish(HostRenderCommand::RemoveProfileAsset {
            kind,
            asset,
            revision,
        })
    }

    fn attach_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if !metrics.is_valid()
            || self.surfaces.contains_key(&surface)
            || self.surfaces.len() == self.max_surfaces
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.publish(HostRenderCommand::Attach { surface, metrics })?;
        self.surfaces.insert(surface, (metrics, None));
        Ok(())
    }

    fn resize_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let state = self
            .surfaces
            .get(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if !metrics.is_valid() || state.1.is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.publish(HostRenderCommand::Resize { surface, metrics })?;
        self.surfaces.get_mut(&surface).unwrap().0 = metrics;
        Ok(())
    }

    fn rebind_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if !metrics.is_valid() || !self.surfaces.contains_key(&surface) {
            return Err(RenderBackendFailure::SurfaceLost);
        }
        self.publish(HostRenderCommand::Rebind { surface, metrics })?;
        self.surfaces.insert(surface, (metrics, None));
        Ok(())
    }

    fn render(
        &mut self,
        submission: &RenderFrameSubmission,
    ) -> Result<RenderFrameAck, RenderBackendFailure> {
        self.ensure_open()?;
        let state = self
            .surfaces
            .get(&submission.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if state.1.is_some()
            || submission.commands.is_empty()
            || submission.commands.len() > MAX_HOST_RENDER_FRAME_BYTES
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let ack = RenderFrameAck {
            surface: submission.surface,
            pulse_id: submission.pulse_id,
            frame_id: submission.frame_id,
            render_endpoint: submission.render_endpoint,
            device_generation: submission.device_generation,
            rendered_revision: submission.required_render_revision,
            observed_control_revision: submission.required_control_revision,
        };
        self.publish(HostRenderCommand::Render(submission.clone()))?;
        self.surfaces.get_mut(&submission.surface).unwrap().1 = Some(ack);
        Ok(ack)
    }

    fn present(
        &mut self,
        ack: RenderFrameAck,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, RenderBackendFailure> {
        self.ensure_open()?;
        let state = self
            .surfaces
            .get(&ack.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if state.1 != Some(ack) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        self.publish(HostRenderCommand::Present {
            ack,
            now_micros,
            deadline_micros,
        })?;
        self.surfaces.get_mut(&ack.surface).unwrap().1 = None;
        Ok(if now_micros > deadline_micros {
            PresentOutcome::DeadlineMissed
        } else {
            PresentOutcome::Presented
        })
    }

    fn detach_surface(&mut self, surface: GameSurfaceId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let state = self
            .surfaces
            .get(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if state.1.is_some() {
            return Err(RenderBackendFailure::OutcomeUnknown);
        }
        self.publish(HostRenderCommand::Detach { surface })?;
        self.surfaces.remove(&surface);
        Ok(())
    }
}

pub fn is_host_render_command(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(MAGIC)
}

pub fn decode_host_render_command(bytes: &[u8]) -> Result<HostRenderCommand, RenderBackendFailure> {
    if bytes.len() < 5 || bytes.len() > MAX_HOST_RENDER_FRAME_BYTES || bytes.get(..4) != Some(MAGIC)
    {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    let mut reader = Reader { bytes, offset: 5 };
    let command = match bytes[4] {
        action @ 1..=3 => {
            let surface = reader.surface()?;
            let metrics = reader.metrics()?;
            if !metrics.is_valid() {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            match action {
                1 => HostRenderCommand::Attach { surface, metrics },
                2 => HostRenderCommand::Resize { surface, metrics },
                _ => HostRenderCommand::Rebind { surface, metrics },
            }
        }
        4 => {
            let texture = reader.u64()?;
            let width = reader.u32()?;
            let height = reader.u32()?;
            let pixels = reader.blob()?.to_vec();
            let expected = (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
            if texture == 0 || width == 0 || height == 0 || pixels.len() != expected {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::UploadTexture {
                texture,
                width,
                height,
                pixels,
            }
        }
        5 => {
            let texture = reader.u64()?;
            if texture == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::RemoveTexture { texture }
        }
        6 => {
            let kind = reader.u32()?;
            let asset = reader.u64()?;
            let revision = reader.u64()?;
            let asset_bytes = reader.blob()?.to_vec();
            if kind < 2 || asset == 0 || revision == 0 || asset_bytes.is_empty() {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::UpsertProfileAsset {
                kind,
                asset,
                revision,
                bytes: asset_bytes,
            }
        }
        7 => {
            let kind = reader.u32()?;
            let asset = reader.u64()?;
            let revision = reader.u64()?;
            if kind < 2 || asset == 0 || revision == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::RemoveProfileAsset {
                kind,
                asset,
                revision,
            }
        }
        8 => {
            let surface = reader.surface()?;
            let pulse_id = reader.u64()?;
            let frame_id = reader.u64()?;
            let device_generation = reader.u64()?;
            let required_render_revision = reader.u64()?;
            let required_control_revision = reader.u64()?;
            let graph_signature = reader.u64()?;
            let render_endpoint = reader.handle()?;
            let commands = reader.blob()?.to_vec();
            if pulse_id == 0
                || frame_id == 0
                || device_generation == 0
                || required_render_revision == 0
                || required_control_revision == 0
                || graph_signature == 0
                || !render_endpoint.is_valid()
                || commands.is_empty()
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::Render(RenderFrameSubmission {
                surface,
                pulse_id,
                frame_id,
                render_endpoint,
                device_generation,
                required_render_revision,
                required_control_revision,
                graph_signature,
                commands,
            })
        }
        9 => {
            let ack = reader.ack()?;
            let now_micros = reader.u64()?;
            let deadline_micros = reader.u64()?;
            HostRenderCommand::Present {
                ack,
                now_micros,
                deadline_micros,
            }
        }
        10 => HostRenderCommand::Detach {
            surface: reader.surface()?,
        },
        11 => {
            let request = ReadbackRequestId(reader.u64()?);
            let target = StableControlRef {
                engine: reader.handle()?,
                kind: ControlKind::RenderTarget,
                handle: reader.handle()?,
            };
            let expected_target_revision = reader.u64()?;
            let region = ReadbackRegion {
                x: reader.u32()?,
                y: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
            };
            let format = decode_readback_format(reader.u32()?)?;
            if request.0 == 0
                || expected_target_revision == 0
                || region.width == 0
                || region.height == 0
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::RequestReadback {
                request,
                target,
                expected_target_revision,
                region,
                format,
            }
        }
        12 => {
            let request = ReadbackRequestId(reader.u64()?);
            if request.0 == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::CancelReadback { request }
        }
        13 => {
            let request = reader.u64()?;
            let frame_id = reader.u64()?;
            let graph_signature = reader.u64()?;
            let flags = *reader.take(1)?.first().unwrap();
            let max_bytes = reader.u32()? as usize;
            if request == 0
                || frame_id == 0
                || graph_signature == 0
                || flags & !0b11 != 0
                || max_bytes == 0
                || max_bytes > MAX_HOST_RENDER_FRAME_BYTES
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::RequestFrameTrace {
                request,
                frame_id,
                graph_signature,
                include_attachments: flags & 1 != 0,
                include_shader_diagnostics: flags & 2 != 0,
                max_bytes,
            }
        }
        14 => {
            let request = reader.u64()?;
            if request == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderCommand::CancelFrameTrace { request }
        }
        15 => {
            let count = reader.u32()? as usize;
            if count > 4096 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            let mut targets = Vec::with_capacity(count);
            let mut seen = BTreeSet::new();
            for _ in 0..count {
                let target = StableControlRef {
                    engine: reader.handle()?,
                    kind: ControlKind::RenderTarget,
                    handle: reader.handle()?,
                };
                if !target.engine.is_valid() || !target.handle.is_valid() || !seen.insert(target) {
                    return Err(RenderBackendFailure::RejectedBeforeSubmit);
                }
                targets.push(target);
            }
            HostRenderCommand::SynchronizeTargets { targets }
        }
        _ => return Err(RenderBackendFailure::RejectedBeforeSubmit),
    };
    if reader.offset != bytes.len() {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    Ok(command)
}

pub fn encode_host_render_command(
    command: &HostRenderCommand,
) -> Result<Vec<u8>, RenderBackendFailure> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    match command {
        HostRenderCommand::Attach { surface, metrics }
        | HostRenderCommand::Resize { surface, metrics }
        | HostRenderCommand::Rebind { surface, metrics } => {
            bytes.push(match command {
                HostRenderCommand::Attach { .. } => 1,
                HostRenderCommand::Resize { .. } => 2,
                _ => 3,
            });
            put_surface(&mut bytes, *surface);
            put_metrics(&mut bytes, *metrics);
        }
        HostRenderCommand::UploadTexture {
            texture,
            width,
            height,
            pixels,
        } => {
            if *texture == 0 || *width == 0 || *height == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(4);
            bytes.extend_from_slice(&texture.to_le_bytes());
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            put_blob(&mut bytes, pixels)?;
        }
        HostRenderCommand::RemoveTexture { texture } => {
            bytes.push(5);
            bytes.extend_from_slice(&texture.to_le_bytes());
        }
        HostRenderCommand::UpsertProfileAsset {
            kind,
            asset,
            revision,
            bytes: asset_bytes,
        } => {
            bytes.push(6);
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&asset.to_le_bytes());
            bytes.extend_from_slice(&revision.to_le_bytes());
            put_blob(&mut bytes, asset_bytes)?;
        }
        HostRenderCommand::RemoveProfileAsset {
            kind,
            asset,
            revision,
        } => {
            bytes.push(7);
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&asset.to_le_bytes());
            bytes.extend_from_slice(&revision.to_le_bytes());
        }
        HostRenderCommand::Render(frame) => {
            bytes.push(8);
            put_surface(&mut bytes, frame.surface);
            for value in [
                frame.pulse_id,
                frame.frame_id,
                frame.device_generation,
                frame.required_render_revision,
                frame.required_control_revision,
                frame.graph_signature,
            ] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            put_handle(&mut bytes, frame.render_endpoint);
            put_blob(&mut bytes, &frame.commands)?;
        }
        HostRenderCommand::Present {
            ack,
            now_micros,
            deadline_micros,
        } => {
            bytes.push(9);
            put_ack(&mut bytes, *ack);
            bytes.extend_from_slice(&now_micros.to_le_bytes());
            bytes.extend_from_slice(&deadline_micros.to_le_bytes());
        }
        HostRenderCommand::Detach { surface } => {
            bytes.push(10);
            put_surface(&mut bytes, *surface);
        }
        HostRenderCommand::RequestReadback {
            request,
            target,
            expected_target_revision,
            region,
            format,
        } => {
            if request.0 == 0
                || target.kind != ControlKind::RenderTarget
                || expected_target_revision == &0
                || region.width == 0
                || region.height == 0
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(11);
            bytes.extend_from_slice(&request.0.to_le_bytes());
            put_handle(&mut bytes, target.engine);
            put_handle(&mut bytes, target.handle);
            bytes.extend_from_slice(&expected_target_revision.to_le_bytes());
            for value in [region.x, region.y, region.width, region.height] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&encode_readback_format(*format).to_le_bytes());
        }
        HostRenderCommand::CancelReadback { request } => {
            if request.0 == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(12);
            bytes.extend_from_slice(&request.0.to_le_bytes());
        }
        HostRenderCommand::RequestFrameTrace {
            request,
            frame_id,
            graph_signature,
            include_attachments,
            include_shader_diagnostics,
            max_bytes,
        } => {
            if *request == 0
                || *frame_id == 0
                || *graph_signature == 0
                || *max_bytes == 0
                || *max_bytes > MAX_HOST_RENDER_FRAME_BYTES
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(13);
            bytes.extend_from_slice(&request.to_le_bytes());
            bytes.extend_from_slice(&frame_id.to_le_bytes());
            bytes.extend_from_slice(&graph_signature.to_le_bytes());
            bytes.push(
                u8::from(*include_attachments) | (u8::from(*include_shader_diagnostics) << 1),
            );
            bytes.extend_from_slice(
                &u32::try_from(*max_bytes)
                    .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?
                    .to_le_bytes(),
            );
        }
        HostRenderCommand::CancelFrameTrace { request } => {
            if *request == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(14);
            bytes.extend_from_slice(&request.to_le_bytes());
        }
        HostRenderCommand::SynchronizeTargets { targets } => {
            if targets.len() > 4096
                || targets.iter().any(|target| {
                    !target.engine.is_valid()
                        || !target.handle.is_valid()
                        || target.kind != ControlKind::RenderTarget
                })
                || targets.iter().copied().collect::<BTreeSet<_>>().len() != targets.len()
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(15);
            bytes.extend_from_slice(&(targets.len() as u32).to_le_bytes());
            for target in targets {
                put_handle(&mut bytes, target.engine);
                put_handle(&mut bytes, target.handle);
            }
        }
    }
    if bytes.len() > MAX_HOST_RENDER_FRAME_BYTES {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    Ok(bytes)
}

pub fn encode_host_render_result(
    result: &HostRenderResult,
) -> Result<Vec<u8>, RenderBackendFailure> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(RESULT_MAGIC);
    match result {
        HostRenderResult::Readback(readback) => {
            if readback.request.0 == 0
                || readback.target.kind != ControlKind::RenderTarget
                || readback.target_revision == 0
                || readback.row_bytes == 0
                || readback.bytes.is_empty()
            {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(1);
            bytes.extend_from_slice(&readback.request.0.to_le_bytes());
            put_handle(&mut bytes, readback.target.engine);
            put_handle(&mut bytes, readback.target.handle);
            bytes.extend_from_slice(&readback.target_revision.to_le_bytes());
            bytes.extend_from_slice(&readback.row_bytes.to_le_bytes());
            put_blob(&mut bytes, &readback.bytes)?;
        }
        HostRenderResult::ReadbackFailed { request, failure } => {
            if request.0 == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(2);
            bytes.extend_from_slice(&request.0.to_le_bytes());
            bytes.push(encode_failure(*failure));
        }
        HostRenderResult::FrameTrace {
            request,
            frame_id,
            graph_signature,
            bytes: trace,
        } => {
            if *request == 0 || *frame_id == 0 || *graph_signature == 0 || trace.is_empty() {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(3);
            bytes.extend_from_slice(&request.to_le_bytes());
            bytes.extend_from_slice(&frame_id.to_le_bytes());
            bytes.extend_from_slice(&graph_signature.to_le_bytes());
            put_blob(&mut bytes, trace)?;
        }
        HostRenderResult::FrameTraceFailed { request, failure } => {
            if *request == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            bytes.push(4);
            bytes.extend_from_slice(&request.to_le_bytes());
            bytes.push(encode_failure(*failure));
        }
    }
    if bytes.len() > MAX_HOST_RENDER_FRAME_BYTES {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    Ok(bytes)
}

pub fn decode_host_render_result(bytes: &[u8]) -> Result<HostRenderResult, RenderBackendFailure> {
    if bytes.len() < 5
        || bytes.len() > MAX_HOST_RENDER_FRAME_BYTES
        || bytes.get(..4) != Some(RESULT_MAGIC)
    {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    let mut reader = Reader { bytes, offset: 5 };
    let result = match bytes[4] {
        1 => {
            let request = ReadbackRequestId(reader.u64()?);
            let target = StableControlRef {
                engine: reader.handle()?,
                kind: ControlKind::RenderTarget,
                handle: reader.handle()?,
            };
            let target_revision = reader.u64()?;
            let row_bytes = reader.u32()?;
            let payload = reader.blob()?.to_vec();
            if request.0 == 0 || target_revision == 0 || row_bytes == 0 || payload.is_empty() {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderResult::Readback(RenderBackendReadback {
                request,
                target,
                target_revision,
                row_bytes,
                bytes: payload,
            })
        }
        2 => {
            let request = ReadbackRequestId(reader.u64()?);
            let failure = decode_failure(*reader.take(1)?.first().unwrap())?;
            if request.0 == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderResult::ReadbackFailed { request, failure }
        }
        3 => {
            let request = reader.u64()?;
            let frame_id = reader.u64()?;
            let graph_signature = reader.u64()?;
            let trace = reader.blob()?.to_vec();
            if request == 0 || frame_id == 0 || graph_signature == 0 || trace.is_empty() {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderResult::FrameTrace {
                request,
                frame_id,
                graph_signature,
                bytes: trace,
            }
        }
        4 => {
            let request = reader.u64()?;
            let failure = decode_failure(*reader.take(1)?.first().unwrap())?;
            if request == 0 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            HostRenderResult::FrameTraceFailed { request, failure }
        }
        _ => return Err(RenderBackendFailure::RejectedBeforeSubmit),
    };
    if reader.offset != bytes.len() {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    Ok(result)
}

fn encode_readback_format(format: ReadbackFormat) -> u32 {
    match format {
        ReadbackFormat::Rgba8 => 1,
        ReadbackFormat::Bgra8 => 2,
        ReadbackFormat::Rgba16Float => 3,
        ReadbackFormat::Depth32Float => 4,
    }
}

fn decode_readback_format(value: u32) -> Result<ReadbackFormat, RenderBackendFailure> {
    match value {
        1 => Ok(ReadbackFormat::Rgba8),
        2 => Ok(ReadbackFormat::Bgra8),
        3 => Ok(ReadbackFormat::Rgba16Float),
        4 => Ok(ReadbackFormat::Depth32Float),
        _ => Err(RenderBackendFailure::RejectedBeforeSubmit),
    }
}

fn encode_failure(failure: RenderBackendFailure) -> u8 {
    match failure {
        RenderBackendFailure::RejectedBeforeSubmit => 1,
        RenderBackendFailure::OutcomeUnknown => 2,
        RenderBackendFailure::SurfaceLost => 3,
        RenderBackendFailure::DeviceLost => 4,
    }
}

fn decode_failure(value: u8) -> Result<RenderBackendFailure, RenderBackendFailure> {
    match value {
        1 => Ok(RenderBackendFailure::RejectedBeforeSubmit),
        2 => Ok(RenderBackendFailure::OutcomeUnknown),
        3 => Ok(RenderBackendFailure::SurfaceLost),
        4 => Ok(RenderBackendFailure::DeviceLost),
        _ => Err(RenderBackendFailure::RejectedBeforeSubmit),
    }
}

fn put_surface(bytes: &mut Vec<u8>, surface: GameSurfaceId) {
    put_handle(bytes, surface.engine);
    put_app_handle(bytes, surface.surface);
    put_handle(bytes, surface.domain.engine);
    put_handle(bytes, surface.domain.handle);
}

fn put_metrics(bytes: &mut Vec<u8>, metrics: SurfaceMetrics) {
    for value in [
        metrics.width,
        metrics.height,
        metrics.scale_numerator,
        metrics.scale_denominator,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn put_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn put_app_handle(bytes: &mut Vec<u8>, handle: vo_app_protocol::GenerationalHandle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn put_ack(bytes: &mut Vec<u8>, ack: RenderFrameAck) {
    put_surface(bytes, ack.surface);
    for value in [
        ack.pulse_id,
        ack.frame_id,
        ack.device_generation,
        ack.rendered_revision,
        ack.observed_control_revision,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    put_handle(bytes, ack.render_endpoint);
}

fn put_blob(bytes: &mut Vec<u8>, blob: &[u8]) -> Result<(), RenderBackendFailure> {
    let len = u32::try_from(blob.len()).map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(blob);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], RenderBackendFailure> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, RenderBackendFailure> {
        self.take(4)?
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn u64(&mut self) -> Result<u64, RenderBackendFailure> {
        self.take(8)?
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn handle(&mut self) -> Result<Handle, RenderBackendFailure> {
        let handle = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !handle.is_valid() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(handle)
    }

    fn app_handle(&mut self) -> Result<vo_app_protocol::GenerationalHandle, RenderBackendFailure> {
        let handle = vo_app_protocol::GenerationalHandle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !handle.is_valid() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(handle)
    }

    fn surface(&mut self) -> Result<GameSurfaceId, RenderBackendFailure> {
        let engine = self.handle()?;
        let surface = self.app_handle()?;
        let domain_engine = self.handle()?;
        let domain_handle = self.handle()?;
        if domain_engine != engine {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(GameSurfaceId {
            engine,
            surface,
            domain: crate::outbox::PresentationDomainId {
                engine: domain_engine,
                handle: domain_handle,
            },
        })
    }

    fn metrics(&mut self) -> Result<SurfaceMetrics, RenderBackendFailure> {
        Ok(SurfaceMetrics {
            width: self.u32()?,
            height: self.u32()?,
            scale_numerator: self.u32()?,
            scale_denominator: self.u32()?,
        })
    }

    fn blob(&mut self) -> Result<&'a [u8], RenderBackendFailure> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    fn ack(&mut self) -> Result<RenderFrameAck, RenderBackendFailure> {
        let surface = self.surface()?;
        let pulse_id = self.u64()?;
        let frame_id = self.u64()?;
        let device_generation = self.u64()?;
        let rendered_revision = self.u64()?;
        let observed_control_revision = self.u64()?;
        let render_endpoint = self.handle()?;
        if pulse_id == 0
            || frame_id == 0
            || device_generation == 0
            || rendered_revision == 0
            || observed_control_revision == 0
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        Ok(RenderFrameAck {
            surface,
            pulse_id,
            frame_id,
            render_endpoint,
            device_generation,
            rendered_revision,
            observed_control_revision,
        })
    }
}
