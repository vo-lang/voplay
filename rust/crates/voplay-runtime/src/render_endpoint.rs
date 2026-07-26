use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(feature = "inspection")]
use std::time::Instant;

use vo_app_protocol::SurfaceHandle;
use voplay_protocol::{generated::MessageKind, EngineId, Handle, PacketHeader};

#[cfg(feature = "inspection")]
use crate::{
    asset::ArtifactId,
    buffer_lease::{BufferLeaseConfig, BufferLeaseRegistry},
    fault_injection::{
        FaultInjectionConfig, FaultInjectionError, FaultInjectionMetrics, FaultInjector,
        FaultPoint, FaultRule, InjectedFault,
    },
    fault_wire::{apply_fault_wire_command, fault_wire_response_packet, is_fault_wire_command},
    frame_debug_capture::{
        CapturedAttachment, CapturedNode, FrameCaptureConfig, FrameCaptureId, FrameCaptureResult,
        FrameDebugCaptureQueue,
    },
    frame_debug_wire::{
        decode_frame_capture_command, encode_frame_capture_attachment_response,
        encode_frame_capture_result, FrameCaptureAttachmentResponse, FrameCaptureWireCommand,
        MAX_FRAME_CAPTURE_WIRE_BYTES,
    },
    native_frame_trace::decode_native_frame_trace,
    render_graph::GraphNodeId,
};

use crate::{
    control::{
        is_tombstone_update, ControlDomain, ControlKind, ControlSnapshotState,
        ControlStateSnapshot, RealizationOutcome, RetirementFence, RetirementProgress,
        StableControlRef,
    },
    control_authority_wire::{
        decode_control_observed_ack, encode_control_observed, encode_control_realization_results,
    },
    control_wire::decode_render_control_snapshot_parts,
    endpoint::{
        DispatchOutcome, EndpointDriver, EndpointDriverError, EndpointQueueMetrics,
        OwnedEndpointPacket,
    },
    outbox::{PresentationDomainId, RenderTransient},
    presentation::{FrameCommandEncoder, FrameEncodeRequest, RenderObjectView},
    readback_wire::{
        decode_readback_request, encode_readback_result, stable_readback_target,
        ReadbackWireCommand, MAX_WIRE_READBACK_BYTES,
    },
    render_control::{
        decode_render_control_snapshot, RenderControlDecodeConfig, RenderControlFrameConfig,
        RenderControlState,
    },
    render_readback::{ReadbackConfig, ReadbackQueue, ReadbackRequestId, ReadbackResult},
    render_wire::{
        decode_render_snapshot_parts, decode_render_transaction_parts,
        decode_render_transient_parts,
    },
    surface::{
        GameSurfaceId, PresentOutcome, RenderBackend, RenderBackendFailure, SurfaceMetrics,
        SurfaceRuntime, SurfaceRuntimeConfig,
    },
    Engine, EngineConfig, RenderEvent, RenderEventOutcome, RenderEventPoll, RenderEventResult,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderEndpointState {
    Created,
    Running,
    Suspended,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderEndpointDriverOwnerSnapshot {
    pub state: RenderEndpointState,
    pub engine: crate::RenderEngineOwnerSnapshot,
    pub control_entries: usize,
    pub retiring_render_targets: usize,
    pub surfaces: usize,
    pub transient_domains: usize,
    pub transient_bytes: usize,
    pub texture_assets: usize,
    pub texture_bytes: usize,
    pub profile_assets: usize,
    pub profile_asset_bytes: usize,
    pub pending_readbacks: usize,
    pub encoder: crate::presentation::FrameEncoderOwnerSnapshot,
    pub event_executor_closed: bool,
    pub owned_render_closed: bool,
    pub output: EndpointQueueMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderEndpointDriverShutdownReport {
    pub engine: crate::RenderEngineShutdownReport,
    pub readbacks: Vec<(PacketHeader, ReadbackResult)>,
    pub released_control_entries: usize,
    pub released_retiring_render_targets: usize,
    pub encoder_before: crate::presentation::FrameEncoderOwnerSnapshot,
    pub encoder_after: crate::presentation::FrameEncoderOwnerSnapshot,
}

pub trait RenderEventExecutor: Send {
    fn execute(&mut self, event: &RenderEvent, engine: &Engine) -> RenderEventOutcome;

    fn close(&mut self, deadline_micros: u64) -> Result<(), ()>;
}

pub struct DefaultRenderEventExecutor;

impl RenderEventExecutor for DefaultRenderEventExecutor {
    fn execute(&mut self, _event: &RenderEvent, _engine: &Engine) -> RenderEventOutcome {
        RenderEventOutcome::Failed
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderEndpointPresentationConfig {
    pub surfaces: SurfaceRuntimeConfig,
    pub max_transient_domains: usize,
    pub max_transient_bytes: usize,
    pub max_texture_assets: usize,
    pub max_texture_bytes: usize,
    pub max_profile_assets: usize,
    pub max_profile_asset_bytes: usize,
    pub endpoint_generation: Handle,
    pub readback: ReadbackConfig,
    #[cfg(feature = "inspection")]
    pub frame_capture: FrameCaptureConfig,
}

impl Default for RenderEndpointPresentationConfig {
    fn default() -> Self {
        Self {
            surfaces: SurfaceRuntimeConfig::default(),
            max_transient_domains: 16,
            max_transient_bytes: 4 * 1024 * 1024,
            max_texture_assets: 4096,
            max_texture_bytes: 512 * 1024 * 1024,
            max_profile_assets: 65_536,
            max_profile_asset_bytes: 1024 * 1024 * 1024,
            endpoint_generation: Handle {
                index: 1,
                generation: 1,
            },
            readback: ReadbackConfig {
                max_bytes_per_request: MAX_WIRE_READBACK_BYTES,
                ..ReadbackConfig::default()
            },
            #[cfg(feature = "inspection")]
            frame_capture: FrameCaptureConfig::default(),
        }
    }
}

struct RenderEndpointPresentation {
    surfaces: SurfaceRuntime,
    ids: BTreeMap<SurfaceHandle, GameSurfaceId>,
    engine_suspended: BTreeSet<GameSurfaceId>,
    transients: BTreeMap<PresentationDomainId, RenderTransient>,
    texture_revisions: BTreeMap<u64, u64>,
    textures: BTreeMap<u64, TextureAssetRecord>,
    texture_bytes: usize,
    profile_asset_revisions: BTreeMap<(u32, u64), u64>,
    profile_assets: BTreeMap<(u32, u64), Vec<u8>>,
    profile_asset_bytes: usize,
    readbacks: ReadbackQueue,
    queued_readbacks: VecDeque<ReadbackRequestId>,
    active_readbacks: BTreeSet<ReadbackRequestId>,
    readback_sources: BTreeMap<ReadbackRequestId, PacketHeader>,
    #[cfg(feature = "inspection")]
    captures: FrameDebugCaptureQueue,
    #[cfg(feature = "inspection")]
    capture_sources: BTreeMap<FrameCaptureId, PacketHeader>,
    #[cfg(feature = "inspection")]
    native_trace_pending: BTreeSet<FrameCaptureId>,
    #[cfg(feature = "inspection")]
    capture_leases: BufferLeaseRegistry,
    #[cfg(feature = "inspection")]
    capture_shutdown: bool,
    transient_bytes: usize,
    config: RenderEndpointPresentationConfig,
    backend: Box<dyn RenderBackend>,
    encoder: Box<dyn FrameCommandEncoder>,
    last_outcome: Option<PresentOutcome>,
}

impl RenderEndpointPresentation {
    fn new(
        engine: EngineId,
        config: RenderEndpointPresentationConfig,
        backend: Box<dyn RenderBackend>,
        encoder: Box<dyn FrameCommandEncoder>,
    ) -> Result<Self, EndpointDriverError> {
        if config.max_transient_domains == 0
            || config.max_transient_bytes == 0
            || config.max_texture_assets == 0
            || config.max_texture_bytes == 0
            || config.max_profile_assets == 0
            || config.max_profile_asset_bytes == 0
            || !config.endpoint_generation.is_valid()
        {
            return Err(EndpointDriverError::Rejected);
        }
        let readbacks = ReadbackQueue::new(engine, config.endpoint_generation, config.readback)
            .map_err(|_| EndpointDriverError::Rejected)?;
        #[cfg(feature = "inspection")]
        let captures =
            FrameDebugCaptureQueue::new(engine, config.endpoint_generation, config.frame_capture)
                .map_err(|_| EndpointDriverError::Rejected)?;
        #[cfg(feature = "inspection")]
        let capture_leases = BufferLeaseRegistry::new(
            engine,
            config.endpoint_generation,
            BufferLeaseConfig {
                max_leases: config
                    .frame_capture
                    .max_results
                    .saturating_mul(config.frame_capture.max_attachments_per_capture),
                max_total_bytes: config
                    .frame_capture
                    .max_bytes_per_capture
                    .saturating_mul(config.frame_capture.max_pending),
                max_chunk_bytes: config.frame_capture.max_bytes_per_capture,
            },
        )
        .map_err(|_| EndpointDriverError::Rejected)?;
        Ok(Self {
            surfaces: SurfaceRuntime::new(engine, config.surfaces)
                .map_err(|_| EndpointDriverError::Rejected)?,
            ids: BTreeMap::new(),
            engine_suspended: BTreeSet::new(),
            transients: BTreeMap::new(),
            texture_revisions: BTreeMap::new(),
            textures: BTreeMap::new(),
            texture_bytes: 0,
            profile_asset_revisions: BTreeMap::new(),
            profile_assets: BTreeMap::new(),
            profile_asset_bytes: 0,
            readbacks,
            queued_readbacks: VecDeque::new(),
            active_readbacks: BTreeSet::new(),
            readback_sources: BTreeMap::new(),
            #[cfg(feature = "inspection")]
            captures,
            #[cfg(feature = "inspection")]
            capture_sources: BTreeMap::new(),
            #[cfg(feature = "inspection")]
            native_trace_pending: BTreeSet::new(),
            #[cfg(feature = "inspection")]
            capture_leases,
            #[cfg(feature = "inspection")]
            capture_shutdown: false,
            transient_bytes: 0,
            config,
            backend,
            encoder,
            last_outcome: None,
        })
    }

    const fn endpoint_generation(&self) -> Handle {
        self.config.endpoint_generation
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), RenderBackendFailure> {
        self.backend.synchronize_render_targets(targets)
    }

    fn advance_readback_clock(&mut self, now_micros: u64) -> Result<(), EndpointDriverError> {
        self.readbacks
            .advance_clock(now_micros / 1_000)
            .map(|_| ())
            .map_err(|_| EndpointDriverError::Rejected)
    }

    fn submit_readback(
        &mut self,
        source: PacketHeader,
        command: ReadbackWireCommand,
        control: Option<&RenderControlState>,
    ) -> Result<(), EndpointDriverError> {
        match command {
            ReadbackWireCommand::Submit(request) => {
                let target = stable_readback_target(request);
                let control = control.ok_or(EndpointDriverError::Rejected)?;
                if request.required_control_revision > control.revision
                    || !control.targets.iter().any(|entry| entry.target == target)
                    || self.readback_sources.contains_key(&request.id)
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.readbacks
                    .submit(request)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.readback_sources.insert(request.id, source);
                self.queued_readbacks.push_back(request.id);
            }
            ReadbackWireCommand::Cancel(id) => {
                if self.active_readbacks.contains(&id) {
                    self.backend
                        .cancel_readback(id)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                    self.active_readbacks.remove(&id);
                    self.readbacks
                        .fail(id, b"readback cancelled after GPU dispatch".to_vec())
                        .map_err(|_| EndpointDriverError::Failed)?;
                } else {
                    self.queued_readbacks.retain(|queued| *queued != id);
                    self.readbacks
                        .cancel(id)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                }
            }
        }
        Ok(())
    }

    fn pump_readbacks(
        &mut self,
    ) -> Result<Vec<(PacketHeader, ReadbackResult)>, EndpointDriverError> {
        for id in self.active_readbacks.iter().copied().collect::<Vec<_>>() {
            match self.backend.poll_readback(id) {
                Ok(Some(completion)) => {
                    self.readbacks
                        .complete(
                            id,
                            self.config.endpoint_generation,
                            completion.target_revision,
                            completion.bytes,
                            None,
                        )
                        .map_err(|_| EndpointDriverError::Failed)?;
                    self.active_readbacks.remove(&id);
                }
                Ok(None) => {}
                Err(_) => {
                    self.readbacks
                        .fail(id, b"render backend readback failed".to_vec())
                        .map_err(|_| EndpointDriverError::Failed)?;
                    self.active_readbacks.remove(&id);
                }
            }
        }
        while let Some(id) = self.queued_readbacks.pop_front() {
            let dispatch = match self.readbacks.dispatch(
                id,
                self.readback_sources
                    .get(&id)
                    .map(|source| source.base_revision)
                    .unwrap_or(0),
            ) {
                Ok(dispatch) => dispatch,
                Err(crate::render_readback::ReadbackError::TargetRevisionMismatch) => continue,
                Err(_) => return Err(EndpointDriverError::Failed),
            };
            let target = stable_readback_target(dispatch.request);
            if self
                .backend
                .request_readback(
                    id,
                    target,
                    dispatch.request.expected_target_revision,
                    dispatch.request.region,
                    dispatch.request.format,
                )
                .is_err()
            {
                self.readbacks
                    .fail(id, b"render backend rejected readback".to_vec())
                    .map_err(|_| EndpointDriverError::Failed)?;
            } else {
                self.active_readbacks.insert(id);
            }
        }
        let mut terminal = Vec::new();
        while let Some(result) = self.readbacks.take_terminal() {
            let source = self
                .readback_sources
                .remove(&result.id)
                .ok_or(EndpointDriverError::Failed)?;
            terminal.push((source, result));
        }
        Ok(terminal)
    }

    fn abort_readbacks(
        &mut self,
        diagnostic: &[u8],
    ) -> Result<Vec<(PacketHeader, ReadbackResult)>, EndpointDriverError> {
        for id in self.active_readbacks.iter().copied().collect::<Vec<_>>() {
            let _ = self.backend.cancel_readback(id);
        }
        self.active_readbacks.clear();
        self.queued_readbacks.clear();
        for id in self.readback_sources.keys().copied().collect::<Vec<_>>() {
            self.readbacks
                .fail(id, diagnostic.to_vec())
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.pump_readbacks()
    }

    #[cfg(feature = "inspection")]
    fn submit_frame_capture(
        &mut self,
        source: PacketHeader,
        command: FrameCaptureWireCommand,
    ) -> Result<Option<FrameCaptureAttachmentResponse>, EndpointDriverError> {
        match command {
            FrameCaptureWireCommand::Submit(request) => {
                if self.capture_sources.contains_key(&request.id) {
                    return Err(EndpointDriverError::Rejected);
                }
                self.captures
                    .submit(request)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.capture_sources.insert(request.id, source);
                Ok(None)
            }
            FrameCaptureWireCommand::Cancel(id) => {
                if !self.capture_sources.contains_key(&id) {
                    return Err(EndpointDriverError::Rejected);
                }
                self.captures
                    .cancel(id)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if self.native_trace_pending.remove(&id) {
                    self.backend
                        .cancel_frame_trace(id.0)
                        .map_err(|_| EndpointDriverError::Failed)?;
                }
                Ok(None)
            }
            FrameCaptureWireCommand::ReadAttachment {
                request,
                lease,
                offset,
                len,
                now_millis,
            } => self
                .capture_leases
                .read_chunk(
                    lease,
                    self.config.endpoint_generation,
                    now_millis,
                    offset,
                    len,
                )
                .map(|bytes| {
                    Some(FrameCaptureAttachmentResponse::Chunk {
                        request,
                        lease,
                        offset,
                        bytes,
                    })
                })
                .map_err(|_| EndpointDriverError::Rejected),
            FrameCaptureWireCommand::ReleaseAttachment { request, lease } => {
                self.capture_leases
                    .release(lease, self.config.endpoint_generation)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                Ok(Some(FrameCaptureAttachmentResponse::Released {
                    request,
                    lease,
                }))
            }
        }
    }

    #[cfg(feature = "inspection")]
    fn take_frame_capture_results(
        &mut self,
    ) -> Result<Vec<(PacketHeader, FrameCaptureResult)>, EndpointDriverError> {
        self.captures
            .take_results(self.config.frame_capture.max_results)
            .into_iter()
            .map(|result| {
                let source = self
                    .capture_sources
                    .remove(&result.request.id)
                    .ok_or(EndpointDriverError::Failed)?;
                Ok((source, result))
            })
            .collect()
    }

    #[cfg(feature = "inspection")]
    fn expire_frame_captures(
        &mut self,
        now_millis: u64,
    ) -> Result<Vec<(PacketHeader, FrameCaptureResult)>, EndpointDriverError> {
        self.captures
            .expire(now_millis)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.take_frame_capture_results()
    }

    #[cfg(feature = "inspection")]
    fn record_capture_node(
        &mut self,
        captures: &[FrameCaptureId],
        node: CapturedNode,
    ) -> Result<(), EndpointDriverError> {
        for id in captures {
            if self.captures.record_node(*id, node.clone()).is_err() {
                self.captures
                    .fail(*id, "frame capture node budget exhausted")
                    .map_err(|_| EndpointDriverError::Failed)?;
            }
        }
        Ok(())
    }

    #[cfg(feature = "inspection")]
    fn pump_native_frame_traces(&mut self) -> Result<(), EndpointDriverError> {
        for id in self
            .native_trace_pending
            .iter()
            .copied()
            .collect::<Vec<_>>()
        {
            let trace = match self.backend.poll_frame_trace(id.0) {
                Ok(Some(bytes)) => {
                    decode_native_frame_trace(&bytes).map_err(|_| EndpointDriverError::Failed)
                }
                Ok(None) => continue,
                Err(_) => Err(EndpointDriverError::Failed),
            };
            self.native_trace_pending.remove(&id);
            match trace {
                Ok(trace) => {
                    let request = self
                        .captures
                        .request(id)
                        .ok_or(EndpointDriverError::Failed)?;
                    if trace.frame_id != request.frame_id
                        || trace.graph_signature != request.expected_graph_signature
                    {
                        self.captures
                            .fail(id, "native frame trace identity mismatch")
                            .map_err(|_| EndpointDriverError::Failed)?;
                        continue;
                    }
                    for node in trace.nodes {
                        self.captures
                            .record_node(id, node)
                            .map_err(|_| EndpointDriverError::Failed)?;
                    }
                    if request.include_attachments {
                        if trace.attachments.is_empty() {
                            self.captures
                                .fail(id, "native frame trace returned no attachments")
                                .map_err(|_| EndpointDriverError::Failed)?;
                            continue;
                        }
                        if trace.attachments.iter().any(|attachment| {
                            (attachment.row_bytes as usize)
                                .checked_mul(attachment.height as usize)
                                .is_none_or(|expected| attachment.bytes.len() != expected)
                        }) {
                            self.captures
                                .fail(id, "native frame attachment byte size mismatch")
                                .map_err(|_| EndpointDriverError::Failed)?;
                            continue;
                        }
                        for attachment in trace.attachments {
                            let mut artifact = [0_u8; 16];
                            artifact[..8].copy_from_slice(&id.0.to_le_bytes());
                            artifact[8..12].copy_from_slice(&attachment.resource.0.to_le_bytes());
                            artifact[12..].copy_from_slice(&attachment.version.to_le_bytes());
                            let lease = self
                                .capture_leases
                                .issue(
                                    self.config.endpoint_generation,
                                    ArtifactId(artifact),
                                    attachment.bytes,
                                    request.deadline_millis,
                                )
                                .map_err(|_| EndpointDriverError::Failed)?;
                            self.captures
                                .record_attachment(
                                    id,
                                    CapturedAttachment {
                                        resource: attachment.resource,
                                        version: attachment.version,
                                        width: attachment.width,
                                        height: attachment.height,
                                        format: attachment.format,
                                        row_bytes: attachment.row_bytes,
                                        lease,
                                    },
                                )
                                .map_err(|_| EndpointDriverError::Failed)?;
                        }
                    }
                    if request.include_shader_diagnostics {
                        for diagnostic in trace.shader_diagnostics {
                            self.captures
                                .record_shader_diagnostic(id, diagnostic)
                                .map_err(|_| EndpointDriverError::Failed)?;
                        }
                    }
                    self.captures
                        .complete(id)
                        .map_err(|_| EndpointDriverError::Failed)?;
                }
                Err(_) => {
                    self.captures
                        .fail(id, "native frame trace failed")
                        .map_err(|_| EndpointDriverError::Failed)?;
                }
            }
        }
        Ok(())
    }

    fn stage_transient(&mut self, transient: RenderTransient) -> Result<(), EndpointDriverError> {
        if self
            .transients
            .get(&transient.domain)
            .is_some_and(|current| current.frame_id >= transient.frame_id)
        {
            return Err(EndpointDriverError::Rejected);
        }
        let replaced_bytes = self
            .transients
            .get(&transient.domain)
            .map_or(0, |current| current.bytes.len());
        let next_bytes = self
            .transient_bytes
            .checked_sub(replaced_bytes)
            .and_then(|bytes| bytes.checked_add(transient.bytes.len()))
            .filter(|bytes| *bytes <= self.config.max_transient_bytes)
            .ok_or(EndpointDriverError::Rejected)?;
        if !self.transients.contains_key(&transient.domain)
            && self.transients.len() == self.config.max_transient_domains
        {
            return Err(EndpointDriverError::Rejected);
        }
        self.transients.insert(transient.domain, transient);
        self.transient_bytes = next_bytes;
        Ok(())
    }

    fn apply_texture_asset(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<u32, EndpointDriverError> {
        let command = decode_texture_asset(payload)?;
        let (texture, revision) = command.identity();
        if header.commit_id != texture
            || header.new_revision != revision
            || self
                .texture_revisions
                .get(&texture)
                .is_some_and(|current| *current >= revision)
        {
            return Err(EndpointDriverError::Rejected);
        }
        match command {
            TextureAssetCommand::Upsert {
                texture,
                revision: _,
                width,
                height,
                pixels,
            } => {
                let previous_bytes = self
                    .textures
                    .get(&texture)
                    .map_or(0, |record| record.pixels.len());
                if (!self.textures.contains_key(&texture)
                    && self.textures.len() == self.config.max_texture_assets)
                    || self
                        .texture_bytes
                        .checked_sub(previous_bytes)
                        .and_then(|bytes| bytes.checked_add(pixels.len()))
                        .is_none_or(|bytes| bytes > self.config.max_texture_bytes)
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.backend
                    .upload_rgba_texture(texture, width, height, pixels)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.texture_bytes = self.texture_bytes - previous_bytes + pixels.len();
                self.textures.insert(
                    texture,
                    TextureAssetRecord {
                        width,
                        height,
                        pixels: pixels.to_vec(),
                    },
                );
            }
            TextureAssetCommand::Remove { texture, .. } => self
                .backend
                .remove_texture(texture)
                .map_err(|_| EndpointDriverError::Rejected)?,
        }
        if matches!(command, TextureAssetCommand::Remove { .. }) {
            if let Some(record) = self.textures.remove(&texture) {
                self.texture_bytes -= record.pixels.len();
            }
        }
        self.texture_revisions.insert(texture, revision);
        Ok(1)
    }

    fn recover_texture_assets(&mut self) -> Result<(), EndpointDriverError> {
        for (texture, record) in &self.textures {
            self.backend
                .upload_rgba_texture(*texture, record.width, record.height, &record.pixels)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        Ok(())
    }

    fn apply_profile_asset(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<u32, EndpointDriverError> {
        let command = decode_profile_asset(payload)?;
        let (kind, asset, revision) = command.identity();
        let key = (kind, asset);
        if header.commit_id != asset
            || header.new_revision != revision
            || self
                .profile_asset_revisions
                .get(&key)
                .is_some_and(|current| *current >= revision)
        {
            return Err(EndpointDriverError::Rejected);
        }
        match command {
            ProfileAssetCommand::Upsert { bytes, .. } => {
                let previous = self.profile_assets.get(&key).map_or(0, Vec::len);
                if (!self.profile_assets.contains_key(&key)
                    && self.profile_assets.len() == self.config.max_profile_assets)
                    || self
                        .profile_asset_bytes
                        .checked_sub(previous)
                        .and_then(|total| total.checked_add(bytes.len()))
                        .is_none_or(|total| total > self.config.max_profile_asset_bytes)
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.encoder
                    .upsert_asset(kind, asset, revision, bytes)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.backend
                    .upsert_profile_asset(kind, asset, revision, bytes)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.profile_asset_bytes = self.profile_asset_bytes - previous + bytes.len();
                self.profile_assets.insert(key, bytes.to_vec());
            }
            ProfileAssetCommand::Remove { .. } => {
                self.encoder
                    .remove_asset(kind, asset, revision)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.backend
                    .remove_profile_asset(kind, asset, revision)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if let Some(bytes) = self.profile_assets.remove(&key) {
                    self.profile_asset_bytes -= bytes.len();
                }
            }
        }
        self.profile_asset_revisions.insert(key, revision);
        Ok(kind)
    }

    fn recover_profile_assets(&mut self) -> Result<(), EndpointDriverError> {
        for ((kind, asset), bytes) in &self.profile_assets {
            let revision = *self
                .profile_asset_revisions
                .get(&(*kind, *asset))
                .ok_or(EndpointDriverError::Failed)?;
            self.backend
                .upsert_profile_asset(*kind, *asset, revision, bytes)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        Ok(())
    }

    fn recover_backend_surfaces(&mut self) -> Result<(), EndpointDriverError> {
        self.surfaces
            .recover_backend_surfaces(self.backend.as_mut())
            .map(|_| ())
            .map_err(|_| EndpointDriverError::Failed)
    }

    fn attach(
        &mut self,
        engine: EngineId,
        command: SurfaceCommand,
    ) -> Result<(), EndpointDriverError> {
        let SurfaceCommand::Attach {
            surface,
            domain,
            metrics,
            render_endpoint,
            device_generation,
        } = command
        else {
            return Err(EndpointDriverError::Rejected);
        };
        let id = self
            .surfaces
            .attach(
                surface,
                PresentationDomainId {
                    engine,
                    handle: domain,
                },
                metrics,
                render_endpoint,
                device_generation,
                self.backend.as_mut(),
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
        self.ids.insert(surface, id);
        Ok(())
    }

    fn apply_surface_command(
        &mut self,
        engine: EngineId,
        command: SurfaceCommand,
    ) -> Result<(), EndpointDriverError> {
        if matches!(command, SurfaceCommand::Attach { .. }) {
            return self.attach(engine, command);
        }
        match command {
            SurfaceCommand::Resize {
                surface,
                domain,
                metrics,
            } => {
                let id = self.id(engine, surface, domain)?;
                self.surfaces
                    .resize(id, metrics, self.backend.as_mut())
                    .map_err(|_| EndpointDriverError::Rejected)
            }
            SurfaceCommand::Suspend { surface, domain } => {
                let id = self.id(engine, surface, domain)?;
                self.surfaces
                    .suspend(id)
                    .map_err(|_| EndpointDriverError::Rejected)
            }
            SurfaceCommand::Resume { surface, domain } => {
                let id = self.id(engine, surface, domain)?;
                self.surfaces
                    .resume(id)
                    .map_err(|_| EndpointDriverError::Rejected)
            }
            SurfaceCommand::Close { surface, domain } => {
                let id = self.id(engine, surface, domain)?;
                self.surfaces
                    .close(id, self.backend.as_mut())
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.ids.remove(&surface);
                self.engine_suspended.remove(&id);
                if let Some(transient) = self.transients.remove(&id.domain) {
                    self.transient_bytes -= transient.bytes.len();
                }
                Ok(())
            }
            SurfaceCommand::ReplaceBinding {
                surface,
                domain,
                render_endpoint,
                device_generation,
            } => {
                let id = self.id(engine, surface, domain)?;
                self.surfaces
                    .replace_render_binding(
                        id,
                        render_endpoint,
                        device_generation,
                        self.backend.as_mut(),
                    )
                    .map_err(|_| EndpointDriverError::Rejected)
            }
            SurfaceCommand::Rebind {
                surface,
                domain,
                replacement_surface,
                render_endpoint,
                device_generation,
                metrics,
            } => {
                let id = self.id(engine, surface, domain)?;
                let replacement = self
                    .surfaces
                    .rebind(
                        id,
                        replacement_surface,
                        render_endpoint,
                        device_generation,
                        metrics,
                        self.backend.as_mut(),
                    )
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.ids.remove(&surface);
                self.ids.insert(replacement_surface, replacement);
                if self.engine_suspended.remove(&id) {
                    self.engine_suspended.insert(replacement);
                }
                Ok(())
            }
            SurfaceCommand::Attach { .. } => unreachable!(),
        }
    }

    fn present(
        &mut self,
        engine: &mut Engine,
        request: FramePulseRequest,
        control: Option<&RenderControlState>,
    ) -> Result<(), EndpointDriverError> {
        let id = self.id(engine.id(), request.surface, request.domain)?;
        let transient = self.transients.get(&id.domain).cloned();
        if let Some(transient) = &transient {
            if transient.frame_id != request.pulse_id
                || transient.required_render_revision > engine.render_revision()
                || transient.required_control_revision > engine.render_control_revision()
            {
                return Err(EndpointDriverError::Rejected);
            }
        }
        let frame_plan = control
            .ok_or(EndpointDriverError::Rejected)?
            .resolve_frame(
                id.domain,
                request.metrics,
                RenderControlFrameConfig::default(),
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
        let graph_signature = frame_plan.signature;
        #[cfg(feature = "inspection")]
        let capture_clock = Instant::now();
        #[cfg(feature = "inspection")]
        let active_captures = self
            .captures
            .begin_frame(
                request.pulse_id,
                graph_signature,
                request.now_micros / 1_000,
            )
            .map_err(|_| EndpointDriverError::Failed)?;
        #[cfg(feature = "inspection")]
        for capture in &active_captures {
            let capture_request = self
                .captures
                .request(*capture)
                .ok_or(EndpointDriverError::Failed)?;
            if self
                .backend
                .request_frame_trace(
                    capture.0,
                    capture_request.frame_id,
                    capture_request.expected_graph_signature,
                    capture_request.include_attachments,
                    capture_request.include_shader_diagnostics,
                    capture_request.max_bytes,
                )
                .is_ok()
            {
                self.native_trace_pending.insert(*capture);
            }
        }
        let delta = engine.prepare_render_delta();
        let objects = delta
            .objects
            .iter()
            .map(|(entity, bytes)| RenderObjectView {
                entity: *entity,
                bytes: bytes.as_slice(),
            })
            .collect();
        let commands = self
            .encoder
            .encode(FrameEncodeRequest {
                engine: engine.id(),
                domain: id.domain,
                pulse_id: request.pulse_id,
                render_revision: engine.render_revision(),
                control_revision: engine.render_control_revision(),
                graph_signature,
                control_frame: Some(&frame_plan),
                full_snapshot: delta.full_snapshot,
                objects,
                despawned: &delta.despawned,
                transient: transient.as_ref(),
            })
            .map_err(|_| EndpointDriverError::Rejected)?;
        #[cfg(feature = "inspection")]
        let encoded_nanos = u64::try_from(capture_clock.elapsed().as_nanos()).unwrap_or(u64::MAX);
        #[cfg(feature = "inspection")]
        self.record_capture_node(
            &active_captures,
            CapturedNode {
                node: GraphNodeId(1),
                label: String::from("frame-command-encode"),
                queue: 1,
                started_nanos: 0,
                finished_nanos: encoded_nanos,
                pipeline_hash: (graph_signature ^ 0x656e_636f_6465).max(1),
                draw_calls: 0,
                dispatch_calls: 0,
            },
        )?;
        engine
            .commit_render_delta(delta.revision)
            .map_err(|_| EndpointDriverError::Rejected)?;
        let pulse = self
            .surfaces
            .begin_exact_pulse(
                id,
                request.pulse_id,
                request.now_micros,
                request.deadline_micros,
                request.metrics,
            )
            .map_err(|_| EndpointDriverError::Rejected)?;
        if let Some(transient) = self.transients.remove(&id.domain) {
            self.transient_bytes -= transient.bytes.len();
        }
        self.last_outcome = Some(
            self.surfaces
                .render_and_present(
                    pulse,
                    request.now_micros,
                    engine.render_revision(),
                    engine.render_control_revision(),
                    graph_signature,
                    commands,
                    self.backend.as_mut(),
                )
                .map_err(|_| EndpointDriverError::Rejected)?,
        );
        #[cfg(feature = "inspection")]
        let presented_nanos = u64::try_from(capture_clock.elapsed().as_nanos()).unwrap_or(u64::MAX);
        #[cfg(feature = "inspection")]
        self.record_capture_node(
            &active_captures,
            CapturedNode {
                node: GraphNodeId(2),
                label: String::from("render-and-present"),
                queue: 2,
                started_nanos: encoded_nanos,
                finished_nanos: presented_nanos,
                pipeline_hash: (graph_signature ^ 0x7072_6573_656e_74).max(1),
                draw_calls: 1,
                dispatch_calls: 0,
            },
        )?;
        #[cfg(feature = "inspection")]
        for capture in active_captures {
            if !self.native_trace_pending.contains(&capture) {
                self.captures
                    .complete(capture)
                    .map_err(|_| EndpointDriverError::Failed)?;
            }
        }
        Ok(())
    }

    fn suspend_all(&mut self) -> Result<(), EndpointDriverError> {
        for id in self.ids.values().copied().collect::<Vec<_>>() {
            if self
                .surfaces
                .state(id)
                .map_err(|_| EndpointDriverError::Failed)?
                == crate::surface::GameSurfaceState::Active
            {
                self.surfaces
                    .suspend(id)
                    .map_err(|_| EndpointDriverError::Failed)?;
                self.engine_suspended.insert(id);
            }
        }
        Ok(())
    }

    fn resume_all(&mut self) -> Result<(), EndpointDriverError> {
        for id in std::mem::take(&mut self.engine_suspended) {
            self.surfaces
                .resume(id)
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        Ok(())
    }

    fn close_all(&mut self) -> Result<Vec<(PacketHeader, ReadbackResult)>, EndpointDriverError> {
        #[cfg(feature = "inspection")]
        self.capture_leases
            .preflight_restart_provider()
            .map_err(|_| EndpointDriverError::Failed)?;
        let readbacks = self.abort_readbacks(b"render endpoint closed")?;
        self.readbacks.shutdown();
        #[cfg(feature = "inspection")]
        for id in std::mem::take(&mut self.native_trace_pending) {
            let _ = self.backend.cancel_frame_trace(id.0);
        }
        #[cfg(feature = "inspection")]
        self.captures
            .fail_all("render endpoint closed")
            .map_err(|_| EndpointDriverError::Failed)?;
        for id in self.ids.values().copied().collect::<Vec<_>>() {
            self.surfaces
                .close(id, self.backend.as_mut())
                .map_err(|_| EndpointDriverError::Failed)?;
        }
        self.surfaces.shutdown();
        self.ids.clear();
        self.engine_suspended.clear();
        self.transients.clear();
        self.transient_bytes = 0;
        self.textures.clear();
        self.texture_revisions.clear();
        self.texture_bytes = 0;
        self.profile_assets.clear();
        self.profile_asset_revisions.clear();
        self.profile_asset_bytes = 0;
        self.encoder.shutdown();
        self.backend
            .shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        Ok(readbacks)
    }

    #[cfg(feature = "inspection")]
    fn finalize_capture_shutdown(&mut self) -> Result<(), EndpointDriverError> {
        if self.capture_shutdown {
            return Ok(());
        }
        self.captures.shutdown();
        self.capture_sources.clear();
        self.native_trace_pending.clear();
        self.capture_leases
            .shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        self.capture_shutdown = true;
        Ok(())
    }

    fn id(
        &self,
        engine: EngineId,
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
    ) -> Result<GameSurfaceId, EndpointDriverError> {
        let id = self
            .ids
            .get(&surface)
            .copied()
            .ok_or(EndpointDriverError::Rejected)?;
        if id.engine != engine || id.domain.handle != domain {
            return Err(EndpointDriverError::Rejected);
        }
        Ok(id)
    }
}

pub struct RenderEndpointDriver {
    engine: Engine,
    control: Option<RenderControlState>,
    control_snapshot: Option<ControlStateSnapshot>,
    last_control_source: Option<PacketHeader>,
    last_observed_progress: Option<(u64, u64, RetirementProgress)>,
    observed_domain_frame: u64,
    observed_event_sequence: u64,
    retiring_render_targets: BTreeMap<StableControlRef, RetirementFence>,
    control_decode: RenderControlDecodeConfig,
    event_executor: Box<dyn RenderEventExecutor>,
    event_executor_closed: bool,
    owned_render_closed: bool,
    presentation: Option<RenderEndpointPresentation>,
    state: RenderEndpointState,
    output: VecDeque<OwnedEndpointPacket>,
    output_bytes: usize,
    max_output_packets: usize,
    max_output_bytes: usize,
    output_peak_packets: usize,
    output_peak_bytes: usize,
    output_capacity_rejections: u64,
    #[cfg(feature = "inspection")]
    faults: FaultInjector,
}

impl RenderEndpointDriver {
    pub fn new(engine: EngineId, config: EngineConfig) -> Result<Self, crate::EngineError> {
        let max_output_bytes = config
            .max_staged_event_bytes
            .saturating_add(MAX_WIRE_READBACK_BYTES)
            .saturating_add(64 * 1024);
        #[cfg(feature = "inspection")]
        let max_output_bytes = max_output_bytes.saturating_add(MAX_FRAME_CAPTURE_WIRE_BYTES);
        #[cfg(feature = "inspection")]
        let faults = FaultInjector::new(FaultInjectionConfig::default())
            .map_err(|_| crate::EngineError::InvalidConfig)?;
        Ok(Self {
            engine: Engine::new(engine, config)?,
            control: None,
            control_snapshot: None,
            last_control_source: None,
            last_observed_progress: None,
            observed_domain_frame: 0,
            observed_event_sequence: 0,
            retiring_render_targets: BTreeMap::new(),
            control_decode: RenderControlDecodeConfig::default(),
            event_executor: Box::new(DefaultRenderEventExecutor),
            event_executor_closed: false,
            owned_render_closed: false,
            presentation: None,
            state: RenderEndpointState::Created,
            output: VecDeque::new(),
            output_bytes: 0,
            max_output_packets: config.max_staged_events.saturating_add(16),
            max_output_bytes,
            output_peak_packets: 0,
            output_peak_bytes: 0,
            output_capacity_rejections: 0,
            #[cfg(feature = "inspection")]
            faults,
        })
    }

    pub fn new_with_control_limits(
        engine: EngineId,
        config: EngineConfig,
        control_decode: RenderControlDecodeConfig,
    ) -> Result<Self, crate::EngineError> {
        let mut endpoint = Self::new(engine, config)?;
        endpoint.control_decode = control_decode;
        Ok(endpoint)
    }

    pub fn new_with_executor(
        engine: EngineId,
        config: EngineConfig,
        control_decode: RenderControlDecodeConfig,
        event_executor: Box<dyn RenderEventExecutor>,
    ) -> Result<Self, crate::EngineError> {
        let mut endpoint = Self::new_with_control_limits(engine, config, control_decode)?;
        endpoint.event_executor = event_executor;
        Ok(endpoint)
    }

    pub fn new_with_presentation(
        engine: EngineId,
        config: EngineConfig,
        control_decode: RenderControlDecodeConfig,
        event_executor: Box<dyn RenderEventExecutor>,
        presentation_config: RenderEndpointPresentationConfig,
        backend: Box<dyn RenderBackend>,
        encoder: Box<dyn FrameCommandEncoder>,
    ) -> Result<Self, EndpointDriverError> {
        let mut endpoint = Self::new_with_executor(engine, config, control_decode, event_executor)
            .map_err(|_| EndpointDriverError::Rejected)?;
        endpoint.presentation = Some(RenderEndpointPresentation::new(
            engine,
            presentation_config,
            backend,
            encoder,
        )?);
        Ok(endpoint)
    }

    pub const fn state(&self) -> RenderEndpointState {
        self.state
    }

    pub const fn control_state(&self) -> Option<&RenderControlState> {
        self.control.as_ref()
    }

    pub fn last_present_outcome(&self) -> Option<PresentOutcome> {
        self.presentation
            .as_ref()
            .and_then(|presentation| presentation.last_outcome)
    }

    pub fn set_work_counters_enabled(&mut self, enabled: bool) {
        self.engine.set_work_counters_enabled(enabled);
    }

    pub fn work_counters(&self) -> crate::EngineWorkCounters {
        self.engine.work_counters()
    }

    pub fn take_work_counters(&self) -> crate::EngineWorkCounters {
        self.engine.take_work_counters()
    }

    pub fn queue_metrics(&self) -> EndpointQueueMetrics {
        EndpointQueueMetrics {
            packets: self.output.len(),
            peak_packets: self.output_peak_packets,
            bytes: self.output_bytes,
            peak_bytes: self.output_peak_bytes,
            capacity_rejections: self.output_capacity_rejections,
        }
    }

    pub fn owner_snapshot(&self) -> RenderEndpointDriverOwnerSnapshot {
        let presentation = self.presentation.as_ref();
        RenderEndpointDriverOwnerSnapshot {
            state: self.state,
            engine: self.engine.owner_snapshot(),
            control_entries: self
                .control_snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.entries.len()),
            retiring_render_targets: self.retiring_render_targets.len(),
            surfaces: presentation.map_or(0, |presentation| presentation.ids.len()),
            transient_domains: presentation.map_or(0, |presentation| presentation.transients.len()),
            transient_bytes: presentation.map_or(0, |presentation| presentation.transient_bytes),
            texture_assets: presentation.map_or(0, |presentation| presentation.textures.len()),
            texture_bytes: presentation.map_or(0, |presentation| presentation.texture_bytes),
            profile_assets: presentation
                .map_or(0, |presentation| presentation.profile_assets.len()),
            profile_asset_bytes: presentation
                .map_or(0, |presentation| presentation.profile_asset_bytes),
            pending_readbacks: presentation
                .map_or(0, |presentation| presentation.readbacks.pending_count()),
            encoder: presentation.map_or(
                crate::presentation::FrameEncoderOwnerSnapshot::default(),
                |presentation| presentation.encoder.owner_snapshot(),
            ),
            event_executor_closed: self.event_executor_closed,
            owned_render_closed: self.owned_render_closed,
            output: self.queue_metrics(),
        }
    }

    #[cfg(feature = "inspection")]
    pub fn install_fault_rule(&mut self, rule: FaultRule) -> Result<(), FaultInjectionError> {
        self.faults.replace(rule)
    }

    #[cfg(feature = "inspection")]
    pub fn remove_fault_rule(
        &mut self,
        point: FaultPoint,
    ) -> Result<FaultRule, FaultInjectionError> {
        self.faults.remove(point)
    }

    #[cfg(feature = "inspection")]
    pub const fn fault_metrics(&self) -> FaultInjectionMetrics {
        self.faults.metrics()
    }

    #[cfg(feature = "inspection")]
    pub fn frame_capture_lease_metrics(&self) -> crate::buffer_lease::BufferLeaseMetrics {
        self.presentation
            .as_ref()
            .map(|presentation| presentation.capture_leases.metrics())
            .unwrap_or_default()
    }

    pub fn register_render_feature(&mut self, bytes: &[u8]) -> Result<(), EndpointDriverError> {
        if self.state != RenderEndpointState::Created || bytes.is_empty() {
            return Err(EndpointDriverError::Rejected);
        }
        self.presentation
            .as_mut()
            .ok_or(EndpointDriverError::Rejected)?
            .encoder
            .register_render_feature(bytes)
            .map_err(|_| EndpointDriverError::Rejected)
    }

    fn push_empty(
        &mut self,
        kind: MessageKind,
        source: PacketHeader,
        commit_id: u64,
        revision: u64,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id,
                base_revision: 0,
                new_revision: revision,
                required_control_revision: 0,
                source_simulation_revision: 0,
                sequence: source.sequence,
                payload_len: 0,
            },
            payload: Vec::new(),
        })
    }

    fn push(&mut self, packet: OwnedEndpointPacket) -> Result<(), EndpointDriverError> {
        packet.validate().map_err(|_| EndpointDriverError::Failed)?;
        if self.output.len() == self.max_output_packets {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Failed);
        }
        let Some(output_bytes) = self
            .output_bytes
            .checked_add(packet.payload.len())
            .filter(|bytes| *bytes <= self.max_output_bytes)
        else {
            self.output_capacity_rejections = self.output_capacity_rejections.saturating_add(1);
            return Err(EndpointDriverError::Failed);
        };
        self.output.push_back(packet);
        self.output_bytes = output_bytes;
        self.output_peak_packets = self.output_peak_packets.max(self.output.len());
        self.output_peak_bytes = self.output_peak_bytes.max(self.output_bytes);
        Ok(())
    }

    fn fail<T>(&mut self) -> Result<T, EndpointDriverError> {
        self.state = RenderEndpointState::Failed;
        Err(EndpointDriverError::Failed)
    }

    fn push_event_result(
        &mut self,
        source: PacketHeader,
        result: RenderEventResult,
    ) -> Result<(), EndpointDriverError> {
        let sequence = result.sequence;
        let payload = vec![render_event_outcome_tag(result.outcome)];
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderEventResult,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id: result.event_id,
                base_revision: self.engine.render_revision(),
                new_revision: self.engine.render_revision(),
                required_control_revision: self.engine.render_control_revision(),
                source_simulation_revision: source.source_simulation_revision,
                sequence: result.sequence,
                payload_len: 1,
            },
            payload,
        })?;
        self.observed_event_sequence = self.observed_event_sequence.max(sequence);
        Ok(())
    }

    fn publish_control_observation(&mut self) -> Result<(), EndpointDriverError> {
        let Some(source) = self.last_control_source else {
            return Ok(());
        };
        let progress = RetirementProgress {
            render_revision: self.engine.render_revision(),
            domain_frame: self.observed_domain_frame,
            event_sequence: self.observed_event_sequence,
            audio_event_sequence: 0,
        };
        if self.last_observed_progress == Some((source.commit_id, source.new_revision, progress)) {
            return Ok(());
        }
        self.push(
            encode_control_observed(source, ControlDomain::Render, progress)
                .map_err(|_| EndpointDriverError::Failed)?,
        )?;
        self.last_observed_progress = Some((source.commit_id, source.new_revision, progress));
        Ok(())
    }

    fn realized_render_targets(
        control: &RenderControlState,
        retiring: &BTreeMap<StableControlRef, RetirementFence>,
    ) -> Vec<StableControlRef> {
        let mut targets = control
            .targets
            .iter()
            .map(|target| target.target)
            .collect::<BTreeSet<_>>();
        targets.extend(retiring.keys().copied());
        targets.into_iter().collect()
    }

    fn release_ready_render_targets(&mut self) -> Result<(), EndpointDriverError> {
        let progress = RetirementProgress {
            render_revision: self.engine.render_revision(),
            domain_frame: self.observed_domain_frame,
            event_sequence: self.observed_event_sequence,
            audio_event_sequence: 0,
        };
        let before = self.retiring_render_targets.len();
        self.retiring_render_targets
            .retain(|_, fence| !retirement_fence_reached(*fence, progress));
        if self.retiring_render_targets.len() == before {
            return Ok(());
        }
        let control = self.control.as_ref().ok_or(EndpointDriverError::Failed)?;
        let targets = Self::realized_render_targets(control, &self.retiring_render_targets);
        if let Some(presentation) = self.presentation.as_mut() {
            presentation
                .synchronize_render_targets(&targets)
                .map_err(|_| EndpointDriverError::Rejected)?;
        }
        Ok(())
    }

    fn retry_render_control_realization(&mut self) -> Result<(), EndpointDriverError> {
        let (Some(source), Some(control), Some(presentation)) = (
            self.last_control_source,
            self.control.as_ref(),
            self.presentation.as_mut(),
        ) else {
            return Ok(());
        };
        let targets = Self::realized_render_targets(control, &self.retiring_render_targets);
        let target_outcome = match presentation.synchronize_render_targets(&targets) {
            Ok(()) => RealizationOutcome::Live,
            Err(RenderBackendFailure::RejectedBeforeSubmit) => RealizationOutcome::PermanentFailure,
            Err(
                RenderBackendFailure::OutcomeUnknown
                | RenderBackendFailure::SurfaceLost
                | RenderBackendFailure::DeviceLost,
            ) => RealizationOutcome::TransientFailure,
        };
        let endpoint_generation = presentation.endpoint_generation();
        let results = control
            .targets
            .iter()
            .map(|target| target.target)
            .chain(control.views.iter().map(|view| view.view))
            .chain(control.graphs.iter().map(|graph| graph.graph))
            .map(|stable| {
                let outcome = match stable.kind {
                    ControlKind::RenderTarget | ControlKind::RenderView => target_outcome,
                    ControlKind::RenderGraph => RealizationOutcome::Live,
                    _ => RealizationOutcome::PermanentFailure,
                };
                (stable, outcome)
            })
            .collect::<Vec<_>>();
        self.push(
            encode_control_realization_results(
                source,
                ControlDomain::Render,
                endpoint_generation,
                &results,
            )
            .map_err(|_| EndpointDriverError::Failed)?,
        )
    }

    fn push_render_asset_ack(
        &mut self,
        source: PacketHeader,
        asset_kind: u32,
    ) -> Result<(), EndpointDriverError> {
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderAssetAck,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id: source.commit_id,
                base_revision: 0,
                new_revision: source.new_revision,
                required_control_revision: self.engine.render_control_revision(),
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: 4,
            },
            payload: asset_kind.to_le_bytes().to_vec(),
        })
    }

    fn push_readback_result(
        &mut self,
        source: PacketHeader,
        result: ReadbackResult,
    ) -> Result<(), EndpointDriverError> {
        let payload = encode_readback_result(&result).map_err(|_| EndpointDriverError::Failed)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::RenderReadbackResult,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id: result.id.0,
                base_revision: result.target_revision,
                new_revision: result.target_revision,
                required_control_revision: source.required_control_revision,
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    fn drain_readbacks(&mut self) -> Result<(), EndpointDriverError> {
        let Some(presentation) = &mut self.presentation else {
            return Ok(());
        };
        let results = presentation.pump_readbacks()?;
        for (source, result) in results {
            self.push_readback_result(source, result)?;
        }
        Ok(())
    }

    #[cfg(feature = "inspection")]
    fn push_frame_capture_result(
        &mut self,
        source: PacketHeader,
        result: FrameCaptureResult,
    ) -> Result<(), EndpointDriverError> {
        let payload =
            encode_frame_capture_result(&result).map_err(|_| EndpointDriverError::Failed)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::InspectionResponse,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id: result.request.id.0,
                base_revision: result.graph_signature,
                new_revision: self.engine.render_revision(),
                required_control_revision: self.engine.render_control_revision(),
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    #[cfg(feature = "inspection")]
    fn push_frame_capture_attachment_response(
        &mut self,
        source: PacketHeader,
        response: FrameCaptureAttachmentResponse,
    ) -> Result<(), EndpointDriverError> {
        let request = match &response {
            FrameCaptureAttachmentResponse::Chunk { request, .. }
            | FrameCaptureAttachmentResponse::Released { request, .. } => *request,
        };
        let payload = encode_frame_capture_attachment_response(&response)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.push(OwnedEndpointPacket {
            header: PacketHeader {
                kind: MessageKind::InspectionResponse,
                engine: self.engine.id(),
                channel_epoch: source.channel_epoch,
                commit_id: request,
                base_revision: self.engine.render_revision(),
                new_revision: self.engine.render_revision(),
                required_control_revision: self.engine.render_control_revision(),
                source_simulation_revision: source.source_simulation_revision,
                sequence: source.sequence,
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    #[cfg(feature = "inspection")]
    fn drain_frame_captures(&mut self) -> Result<(), EndpointDriverError> {
        let Some(presentation) = &mut self.presentation else {
            return Ok(());
        };
        presentation.pump_native_frame_traces()?;
        let results = presentation.take_frame_capture_results()?;
        for (source, result) in results {
            self.push_frame_capture_result(source, result)?;
        }
        Ok(())
    }

    fn drain_events(&mut self, source: PacketHeader) -> Result<(), EndpointDriverError> {
        loop {
            match self.engine.poll_event() {
                Some(RenderEventPoll::Dispatch(event)) => {
                    let outcome = self.event_executor.execute(&event, &self.engine);
                    self.engine
                        .complete_event(event.sequence, outcome)
                        .map_err(|_| EndpointDriverError::Failed)?;
                }
                Some(RenderEventPoll::Terminal(_)) => {}
                None => break,
            }
        }
        for result in self.engine.drain_contiguous_event_results() {
            self.push_event_result(source, result)?;
        }
        Ok(())
    }

    fn shutdown_owned_render(
        &mut self,
        deadline_micros: u64,
    ) -> Result<Option<RenderEndpointDriverShutdownReport>, EndpointDriverError> {
        if self.owned_render_closed {
            self.close_event_executor(deadline_micros)?;
            return Ok(None);
        }
        self.engine
            .preflight_shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        self.close_event_executor(deadline_micros)?;
        let encoder_before = self.presentation.as_ref().map_or(
            crate::presentation::FrameEncoderOwnerSnapshot::default(),
            |presentation| presentation.encoder.owner_snapshot(),
        );
        let readbacks = if let Some(presentation) = &mut self.presentation {
            presentation.close_all()?
        } else {
            Vec::new()
        };
        let encoder_after = self.presentation.as_ref().map_or(
            crate::presentation::FrameEncoderOwnerSnapshot::default(),
            |presentation| presentation.encoder.owner_snapshot(),
        );
        let engine = self
            .engine
            .shutdown()
            .map_err(|_| EndpointDriverError::Failed)?;
        let released_control_entries = self
            .control_snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.entries.len());
        let released_retiring_render_targets = self.retiring_render_targets.len();
        self.control = None;
        self.control_snapshot = None;
        self.last_control_source = None;
        self.last_observed_progress = None;
        self.observed_domain_frame = 0;
        self.observed_event_sequence = 0;
        self.retiring_render_targets.clear();
        #[cfg(feature = "inspection")]
        self.faults.shutdown();
        self.owned_render_closed = true;
        Ok(Some(RenderEndpointDriverShutdownReport {
            engine,
            readbacks,
            released_control_entries,
            released_retiring_render_targets,
            encoder_before,
            encoder_after,
        }))
    }

    fn close_event_executor(&mut self, deadline_micros: u64) -> Result<(), EndpointDriverError> {
        if self.event_executor_closed {
            return Ok(());
        }
        self.event_executor
            .close(deadline_micros)
            .map_err(|_| EndpointDriverError::Failed)?;
        self.event_executor_closed = true;
        Ok(())
    }
}

impl EndpointDriver for RenderEndpointDriver {
    fn dispatch(
        &mut self,
        header: PacketHeader,
        payload: &[u8],
    ) -> Result<DispatchOutcome, EndpointDriverError> {
        if header.engine != self.engine.id() {
            return self.fail();
        }
        #[cfg(feature = "inspection")]
        if let Some(fault) = self.faults.trigger(render_fault_point(header.kind)) {
            match fault {
                InjectedFault::RejectBeforeDispatch => {
                    return Err(EndpointDriverError::Rejected);
                }
                InjectedFault::DropLatestOnly
                    if matches!(
                        header.kind,
                        MessageKind::RenderTransient | MessageKind::FramePulse
                    ) =>
                {
                    return Ok(DispatchOutcome::Idle);
                }
                InjectedFault::DropLatestOnly => {
                    return Err(EndpointDriverError::Rejected);
                }
                InjectedFault::FailOwner
                | InjectedFault::OutcomeUnknown
                | InjectedFault::SurfaceLost
                | InjectedFault::DeviceLost
                | InjectedFault::AudioDeviceLost => return self.fail(),
            }
        }
        match header.kind {
            MessageKind::EngineStart if self.state == RenderEndpointState::Created => {
                if !payload.is_empty() {
                    return self.fail();
                }
                self.state = RenderEndpointState::Running;
                self.push_empty(MessageKind::EngineReady, header, 0, 0)?;
            }
            MessageKind::EngineSuspend if self.state == RenderEndpointState::Running => {
                if !payload.is_empty() {
                    return self.fail();
                }
                if let Some(presentation) = &mut self.presentation {
                    presentation.suspend_all()?;
                }
                self.state = RenderEndpointState::Suspended;
            }
            MessageKind::EngineResume if self.state == RenderEndpointState::Suspended => {
                if !payload.is_empty() {
                    return self.fail();
                }
                if let Some(presentation) = &mut self.presentation {
                    presentation.resume_all()?;
                }
                self.state = RenderEndpointState::Running;
            }
            MessageKind::EngineClose
                if matches!(
                    self.state,
                    RenderEndpointState::Created
                        | RenderEndpointState::Running
                        | RenderEndpointState::Suspended
                        | RenderEndpointState::Failed
                ) =>
            {
                if !payload.is_empty() {
                    return self.fail();
                }
                let shutdown = self
                    .shutdown_owned_render(0)?
                    .ok_or(EndpointDriverError::Failed)?;
                for (source, result) in shutdown.readbacks {
                    self.push_readback_result(source, result)?;
                }
                #[cfg(feature = "inspection")]
                {
                    self.drain_frame_captures()?;
                    if let Some(presentation) = &mut self.presentation {
                        presentation.finalize_capture_shutdown()?;
                    }
                }
                for result in shutdown.engine.terminal_events {
                    self.push_event_result(header, result)?;
                }
                self.state = RenderEndpointState::Closed;
                self.push_empty(MessageKind::EngineClosed, header, 0, 0)?;
            }
            MessageKind::RenderStateTransaction if self.state == RenderEndpointState::Running => {
                let transaction = decode_render_transaction_parts(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let ack = self
                    .engine
                    .apply_render_state(transaction)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_empty(
                    MessageKind::RenderStateAck,
                    header,
                    ack.commit_id,
                    ack.revision,
                )?;
                self.drain_events(header)?;
            }
            MessageKind::RenderStateSnapshot
                if matches!(
                    self.state,
                    RenderEndpointState::Created
                        | RenderEndpointState::Running
                        | RenderEndpointState::Suspended
                ) =>
            {
                let snapshot = decode_render_snapshot_parts(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                let ack = self
                    .engine
                    .apply_render_snapshot(snapshot)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.push_empty(
                    MessageKind::RenderStateAck,
                    header,
                    ack.commit_id,
                    ack.revision,
                )?;
                self.drain_events(header)?;
            }
            MessageKind::RenderControlTransaction
                if matches!(
                    self.state,
                    RenderEndpointState::Created
                        | RenderEndpointState::Running
                        | RenderEndpointState::Suspended
                ) =>
            {
                if header.new_revision < self.engine.render_control_revision() {
                    return Err(EndpointDriverError::Rejected);
                }
                let snapshot = decode_render_control_snapshot_parts(
                    header,
                    payload,
                    self.control_decode.max_entries,
                    self.control_decode.max_descriptor_bytes,
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                let control = decode_render_control_snapshot(&snapshot, self.control_decode)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if header.new_revision == self.engine.render_control_revision() {
                    let current_snapshot = self
                        .control_snapshot
                        .as_ref()
                        .ok_or(EndpointDriverError::Rejected)?;
                    if current_snapshot != &snapshot
                        && !is_tombstone_update(current_snapshot, &snapshot)
                    {
                        return Err(EndpointDriverError::Rejected);
                    }
                }
                let realized = control
                    .targets
                    .iter()
                    .map(|target| target.target)
                    .chain(control.views.iter().map(|view| view.view))
                    .chain(control.graphs.iter().map(|graph| graph.graph))
                    .collect::<Vec<StableControlRef>>();
                let retiring_render_targets = snapshot
                    .entries
                    .iter()
                    .filter_map(|entry| match &entry.state {
                        ControlSnapshotState::Tombstone { fence }
                            if entry.stable.kind == ControlKind::RenderTarget =>
                        {
                            Some((entry.stable, *fence))
                        }
                        _ => None,
                    })
                    .collect::<BTreeMap<_, _>>();
                let realization_outcome = if let Some(presentation) = self.presentation.as_mut() {
                    let targets = Self::realized_render_targets(&control, &retiring_render_targets);
                    match presentation.synchronize_render_targets(&targets) {
                        Ok(()) => RealizationOutcome::Live,
                        Err(RenderBackendFailure::RejectedBeforeSubmit) => {
                            RealizationOutcome::PermanentFailure
                        }
                        Err(
                            RenderBackendFailure::OutcomeUnknown
                            | RenderBackendFailure::SurfaceLost
                            | RenderBackendFailure::DeviceLost,
                        ) => RealizationOutcome::TransientFailure,
                    }
                } else {
                    RealizationOutcome::Live
                };
                self.engine.set_render_control_revision(control.revision);
                self.control = Some(control);
                self.control_snapshot = Some(snapshot);
                self.retiring_render_targets = retiring_render_targets;
                self.push_empty(
                    MessageKind::RenderControlAck,
                    header,
                    header.commit_id,
                    header.new_revision,
                )?;
                if let Some(endpoint_generation) = self
                    .presentation
                    .as_ref()
                    .map(RenderEndpointPresentation::endpoint_generation)
                {
                    self.push(
                        encode_control_realization_results(
                            header,
                            ControlDomain::Render,
                            endpoint_generation,
                            &realized
                                .iter()
                                .copied()
                                .map(|stable| {
                                    let outcome = match stable.kind {
                                        ControlKind::RenderTarget | ControlKind::RenderView => {
                                            realization_outcome
                                        }
                                        ControlKind::RenderGraph => RealizationOutcome::Live,
                                        _ => RealizationOutcome::PermanentFailure,
                                    };
                                    (stable, outcome)
                                })
                                .collect::<Vec<_>>(),
                        )
                        .map_err(|_| EndpointDriverError::Failed)?,
                    )?;
                }
                self.drain_events(header)?;
                self.last_control_source = Some(header);
            }
            MessageKind::RenderEvent if self.state == RenderEndpointState::Running => {
                if header.sequence == 0
                    || header.commit_id == 0
                    || header.new_revision == 0
                    || header.payload_len as usize != payload.len()
                {
                    return Err(EndpointDriverError::Rejected);
                }
                self.engine
                    .stage_event(RenderEvent {
                        engine: self.engine.id(),
                        sequence: header.sequence,
                        event_id: header.commit_id,
                        min_render_revision: header.base_revision,
                        required_control_revision: header.required_control_revision,
                        deadline_millis: header.new_revision,
                        payload: payload.to_vec(),
                    })
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.drain_events(header)?;
            }
            MessageKind::ControlObservedAck
                if matches!(
                    self.state,
                    RenderEndpointState::Created
                        | RenderEndpointState::Running
                        | RenderEndpointState::Suspended
                ) =>
            {
                let observed = decode_control_observed_ack(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                if observed.domain != ControlDomain::Render
                    || self.last_control_source.is_none_or(|source| {
                        observed.revision > source.new_revision
                            || observed.revision == source.new_revision
                                && observed.transaction_id != source.commit_id
                    })
                {
                    return Err(EndpointDriverError::Rejected);
                }
            }
            MessageKind::RenderTransient if self.state == RenderEndpointState::Running => {
                if header.channel_epoch != self.engine.channel_epoch() {
                    return Err(EndpointDriverError::Rejected);
                }
                let transient = decode_render_transient_parts(header, payload)
                    .map_err(|_| EndpointDriverError::Rejected)?;
                self.presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?
                    .stage_transient(transient)?;
            }
            MessageKind::RenderAssetData
                if matches!(
                    self.state,
                    RenderEndpointState::Running | RenderEndpointState::Suspended
                ) =>
            {
                let presentation = self
                    .presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?;
                let asset_kind = match payload.get(..4) {
                    Some(b"VRT1") => presentation.apply_texture_asset(header, payload)?,
                    Some(b"VRA1") => presentation.apply_profile_asset(header, payload)?,
                    _ => return Err(EndpointDriverError::Rejected),
                };
                self.push_render_asset_ack(header, asset_kind)?;
            }
            MessageKind::RenderReadbackRequest
                if matches!(
                    self.state,
                    RenderEndpointState::Running | RenderEndpointState::Suspended
                ) =>
            {
                let presentation = self
                    .presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?;
                let command =
                    decode_readback_request(header, payload, presentation.endpoint_generation())
                        .map_err(|_| EndpointDriverError::Rejected)?;
                presentation.submit_readback(header, command, self.control.as_ref())?;
            }
            #[cfg(feature = "inspection")]
            MessageKind::InspectionRequest
                if matches!(
                    self.state,
                    RenderEndpointState::Running | RenderEndpointState::Suspended
                ) =>
            {
                if is_fault_wire_command(payload) {
                    let response = apply_fault_wire_command(&mut self.faults, payload)
                        .map_err(|_| EndpointDriverError::Rejected)?;
                    self.push(fault_wire_response_packet(
                        self.engine.id(),
                        header,
                        response,
                    ))?;
                    return Ok(DispatchOutcome::Wake);
                }
                let presentation = self
                    .presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?;
                let command = decode_frame_capture_command(
                    header,
                    payload,
                    presentation.endpoint_generation(),
                )
                .map_err(|_| EndpointDriverError::Rejected)?;
                if let Some(response) = presentation.submit_frame_capture(header, command)? {
                    self.push_frame_capture_attachment_response(header, response)?;
                }
                self.drain_frame_captures()?;
            }
            MessageKind::SurfaceControl
                if matches!(
                    self.state,
                    RenderEndpointState::Running | RenderEndpointState::Suspended
                ) =>
            {
                let command = decode_surface_command(payload)?;
                self.presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?
                    .apply_surface_command(self.engine.id(), command)?;
            }
            MessageKind::FramePulse if self.state == RenderEndpointState::Running => {
                let request = decode_frame_pulse(header, payload)?;
                let pulse_id = request.pulse_id;
                self.presentation
                    .as_mut()
                    .ok_or(EndpointDriverError::Rejected)?
                    .present(&mut self.engine, request, self.control.as_ref())?;
                self.observed_domain_frame = self.observed_domain_frame.max(pulse_id);
                self.drain_readbacks()?;
                #[cfg(feature = "inspection")]
                self.drain_frame_captures()?;
            }
            MessageKind::WorkerWake if self.state == RenderEndpointState::Running => {
                if !payload.is_empty() {
                    return Err(EndpointDriverError::Rejected);
                }
                self.drain_events(header)?;
                self.drain_readbacks()?;
                #[cfg(feature = "inspection")]
                self.drain_frame_captures()?;
            }
            MessageKind::DeviceEvent
                if matches!(
                    self.state,
                    RenderEndpointState::Running | RenderEndpointState::Suspended
                ) =>
            {
                match payload {
                    [1, time @ ..] if time.len() == 8 => {
                        let now = u64::from_le_bytes(time.try_into().unwrap());
                        self.engine
                            .advance_time(now)
                            .map_err(|_| EndpointDriverError::Rejected)?;
                        if let Some(presentation) = &mut self.presentation {
                            presentation.advance_readback_clock(now)?;
                            #[cfg(feature = "inspection")]
                            {
                                let captures = presentation.expire_frame_captures(now)?;
                                for (source, result) in captures {
                                    self.push_frame_capture_result(source, result)?;
                                }
                            }
                        }
                    }
                    [2] => {
                        let readbacks = if let Some(presentation) = &mut self.presentation {
                            presentation
                                .abort_readbacks(b"render device restarted during readback")?
                        } else {
                            Vec::new()
                        };
                        for (source, result) in readbacks {
                            self.push_readback_result(source, result)?;
                        }
                        if let Some(presentation) = &mut self.presentation {
                            #[cfg(feature = "inspection")]
                            presentation
                                .captures
                                .device_restarted()
                                .map_err(|_| EndpointDriverError::Failed)?;
                            #[cfg(feature = "inspection")]
                            presentation
                                .capture_leases
                                .restart_provider()
                                .map_err(|_| EndpointDriverError::Failed)?;
                            presentation.recover_backend_surfaces()?;
                            presentation.recover_texture_assets()?;
                            presentation.recover_profile_assets()?;
                        }
                        self.retry_render_control_realization()?;
                        let results = self
                            .engine
                            .restart_renderer()
                            .map_err(|_| EndpointDriverError::Failed)?;
                        for result in results {
                            self.push_event_result(header, result)?;
                        }
                    }
                    _ => return Err(EndpointDriverError::Rejected),
                }
                self.drain_events(header)?;
                self.drain_readbacks()?;
                #[cfg(feature = "inspection")]
                self.drain_frame_captures()?;
            }
            _ => return Err(EndpointDriverError::Rejected),
        }
        self.release_ready_render_targets()?;
        self.publish_control_observation()?;
        Ok(if self.output.is_empty() {
            DispatchOutcome::Idle
        } else {
            DispatchOutcome::Wake
        })
    }

    fn poll(&mut self, budget: usize) -> Result<Vec<OwnedEndpointPacket>, EndpointDriverError> {
        let packets = (0..budget)
            .map_while(|_| self.output.pop_front())
            .collect::<Vec<_>>();
        self.output_bytes = self.output.iter().map(|packet| packet.payload.len()).sum();
        Ok(packets)
    }

    fn close(&mut self, _deadline_micros: u64) -> Result<(), EndpointDriverError> {
        self.shutdown_owned_render(_deadline_micros)?;
        #[cfg(feature = "inspection")]
        if let Some(presentation) = &mut self.presentation {
            presentation.finalize_capture_shutdown()?;
        }
        self.state = RenderEndpointState::Closed;
        self.output.clear();
        self.output_bytes = 0;
        Ok(())
    }
}

const fn retirement_fence_reached(fence: RetirementFence, progress: RetirementProgress) -> bool {
    progress.render_revision >= fence.last_render_revision
        && progress.domain_frame >= fence.last_domain_frame
        && progress.event_sequence >= fence.last_event_sequence
        && progress.audio_event_sequence >= fence.last_audio_event_sequence
}

#[cfg(feature = "inspection")]
const fn render_fault_point(kind: MessageKind) -> FaultPoint {
    match kind {
        MessageKind::WorkerWake => FaultPoint::WorkerDispatch,
        MessageKind::RenderReadbackRequest => FaultPoint::Readback,
        MessageKind::InspectionRequest => FaultPoint::FrameCapture,
        MessageKind::SurfaceControl => FaultPoint::ResourceAcquire,
        MessageKind::FramePulse => FaultPoint::SurfaceSubmit,
        MessageKind::DeviceEvent => FaultPoint::DeviceOperation,
        MessageKind::EngineClose => FaultPoint::Shutdown,
        _ => FaultPoint::ProtocolDecode,
    }
}

#[derive(Clone, Copy)]
enum TextureAssetCommand<'a> {
    Upsert {
        texture: u64,
        revision: u64,
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
    Remove {
        texture: u64,
        revision: u64,
    },
}

struct TextureAssetRecord {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
enum ProfileAssetCommand<'a> {
    Upsert {
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &'a [u8],
    },
    Remove {
        kind: u32,
        asset: u64,
        revision: u64,
    },
}

impl ProfileAssetCommand<'_> {
    const fn identity(&self) -> (u32, u64, u64) {
        match *self {
            Self::Upsert {
                kind,
                asset,
                revision,
                ..
            }
            | Self::Remove {
                kind,
                asset,
                revision,
            } => (kind, asset, revision),
        }
    }
}

fn decode_profile_asset(payload: &[u8]) -> Result<ProfileAssetCommand<'_>, EndpointDriverError> {
    if payload.len() < 29 || payload.get(..4) != Some(b"VRA1") {
        return Err(EndpointDriverError::Rejected);
    }
    let action = payload[4];
    let kind = read_u32(payload, 5);
    let asset = read_u64(payload, 9);
    let revision = read_u64(payload, 17);
    let byte_len = read_u32(payload, 25) as usize;
    if kind < 2 || asset == 0 || revision == 0 {
        return Err(EndpointDriverError::Rejected);
    }
    match action {
        1 => {
            let bytes = payload
                .get(29..)
                .filter(|bytes| !bytes.is_empty() && bytes.len() == byte_len)
                .ok_or(EndpointDriverError::Rejected)?;
            Ok(ProfileAssetCommand::Upsert {
                kind,
                asset,
                revision,
                bytes,
            })
        }
        2 if byte_len == 0 && payload.len() == 29 => Ok(ProfileAssetCommand::Remove {
            kind,
            asset,
            revision,
        }),
        _ => Err(EndpointDriverError::Rejected),
    }
}

impl TextureAssetCommand<'_> {
    const fn identity(&self) -> (u64, u64) {
        match *self {
            Self::Upsert {
                texture, revision, ..
            }
            | Self::Remove { texture, revision } => (texture, revision),
        }
    }
}

fn decode_texture_asset(payload: &[u8]) -> Result<TextureAssetCommand<'_>, EndpointDriverError> {
    if payload.len() < 21 || payload.get(..4) != Some(b"VRT1") {
        return Err(EndpointDriverError::Rejected);
    }
    let action = payload[4];
    let texture = read_u64(payload, 5);
    let revision = read_u64(payload, 13);
    if texture == 0 || revision == 0 {
        return Err(EndpointDriverError::Rejected);
    }
    match action {
        1 if payload.len() >= 33 => {
            let width = read_u32(payload, 21);
            let height = read_u32(payload, 25);
            let byte_len = read_u32(payload, 29) as usize;
            let expected = (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .filter(|bytes| *bytes == byte_len)
                .ok_or(EndpointDriverError::Rejected)?;
            let pixels = payload
                .get(33..)
                .filter(|pixels| pixels.len() == expected)
                .ok_or(EndpointDriverError::Rejected)?;
            Ok(TextureAssetCommand::Upsert {
                texture,
                revision,
                width,
                height,
                pixels,
            })
        }
        2 if payload.len() == 21 => Ok(TextureAssetCommand::Remove { texture, revision }),
        _ => Err(EndpointDriverError::Rejected),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceCommand {
    Attach {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
        metrics: SurfaceMetrics,
        render_endpoint: voplay_protocol::Handle,
        device_generation: u64,
    },
    Resize {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
        metrics: SurfaceMetrics,
    },
    Suspend {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
    },
    Resume {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
    },
    Close {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
    },
    ReplaceBinding {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
        render_endpoint: voplay_protocol::Handle,
        device_generation: u64,
    },
    Rebind {
        surface: SurfaceHandle,
        domain: voplay_protocol::Handle,
        replacement_surface: SurfaceHandle,
        render_endpoint: voplay_protocol::Handle,
        device_generation: u64,
        metrics: SurfaceMetrics,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FramePulseRequest {
    surface: SurfaceHandle,
    domain: voplay_protocol::Handle,
    pulse_id: u64,
    now_micros: u64,
    deadline_micros: u64,
    metrics: SurfaceMetrics,
}

fn decode_surface_command(payload: &[u8]) -> Result<SurfaceCommand, EndpointDriverError> {
    let (&tag, bytes) = payload.split_first().ok_or(EndpointDriverError::Rejected)?;
    let id =
        |bytes: &[u8]| -> Result<(SurfaceHandle, voplay_protocol::Handle), EndpointDriverError> {
            if bytes.len() < 16 {
                return Err(EndpointDriverError::Rejected);
            }
            let surface = SurfaceHandle {
                index: read_u32(bytes, 0),
                generation: read_u32(bytes, 4),
            };
            let domain = read_handle(bytes, 8);
            if !surface.is_valid() || !domain.is_valid() {
                return Err(EndpointDriverError::Rejected);
            }
            Ok((surface, domain))
        };
    match tag {
        1 if bytes.len() == 48 => {
            let (surface, domain) = id(bytes)?;
            let metrics = read_metrics(bytes, 16)?;
            let render_endpoint = read_handle(bytes, 32);
            let device_generation = read_u64(bytes, 40);
            if !render_endpoint.is_valid() || device_generation == 0 {
                return Err(EndpointDriverError::Rejected);
            }
            Ok(SurfaceCommand::Attach {
                surface,
                domain,
                metrics,
                render_endpoint,
                device_generation,
            })
        }
        2 if bytes.len() == 32 => {
            let (surface, domain) = id(bytes)?;
            Ok(SurfaceCommand::Resize {
                surface,
                domain,
                metrics: read_metrics(bytes, 16)?,
            })
        }
        3 if bytes.len() == 16 => {
            let (surface, domain) = id(bytes)?;
            Ok(SurfaceCommand::Suspend { surface, domain })
        }
        4 if bytes.len() == 16 => {
            let (surface, domain) = id(bytes)?;
            Ok(SurfaceCommand::Resume { surface, domain })
        }
        5 if bytes.len() == 16 => {
            let (surface, domain) = id(bytes)?;
            Ok(SurfaceCommand::Close { surface, domain })
        }
        6 if bytes.len() == 32 => {
            let (surface, domain) = id(bytes)?;
            let render_endpoint = read_handle(bytes, 16);
            let device_generation = read_u64(bytes, 24);
            if !render_endpoint.is_valid() || device_generation == 0 {
                return Err(EndpointDriverError::Rejected);
            }
            Ok(SurfaceCommand::ReplaceBinding {
                surface,
                domain,
                render_endpoint,
                device_generation,
            })
        }
        7 if bytes.len() == 56 => {
            let (surface, domain) = id(bytes)?;
            let replacement_surface = SurfaceHandle {
                index: read_u32(bytes, 16),
                generation: read_u32(bytes, 20),
            };
            let render_endpoint = read_handle(bytes, 24);
            let device_generation = read_u64(bytes, 32);
            let metrics = read_metrics(bytes, 40)?;
            if !replacement_surface.is_valid()
                || !render_endpoint.is_valid()
                || device_generation == 0
            {
                return Err(EndpointDriverError::Rejected);
            }
            Ok(SurfaceCommand::Rebind {
                surface,
                domain,
                replacement_surface,
                render_endpoint,
                device_generation,
                metrics,
            })
        }
        _ => Err(EndpointDriverError::Rejected),
    }
}

fn decode_frame_pulse(
    header: PacketHeader,
    payload: &[u8],
) -> Result<FramePulseRequest, EndpointDriverError> {
    if header.sequence == 0 || payload.len() != 48 {
        return Err(EndpointDriverError::Rejected);
    }
    let surface = SurfaceHandle {
        index: read_u32(payload, 0),
        generation: read_u32(payload, 4),
    };
    let domain = read_handle(payload, 8);
    let now_micros = read_u64(payload, 16);
    let deadline_micros = read_u64(payload, 24);
    let metrics = read_metrics(payload, 32)?;
    if !surface.is_valid()
        || !domain.is_valid()
        || deadline_micros < now_micros
        || header.commit_id != now_micros
        || header.new_revision != deadline_micros
    {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(FramePulseRequest {
        surface,
        domain,
        pulse_id: header.sequence,
        now_micros,
        deadline_micros,
        metrics,
    })
}

fn read_metrics(bytes: &[u8], offset: usize) -> Result<SurfaceMetrics, EndpointDriverError> {
    let metrics = SurfaceMetrics {
        width: read_u32(bytes, offset),
        height: read_u32(bytes, offset + 4),
        scale_numerator: read_u32(bytes, offset + 8),
        scale_denominator: read_u32(bytes, offset + 12),
    };
    if !metrics.is_valid() {
        return Err(EndpointDriverError::Rejected);
    }
    Ok(metrics)
}

fn read_handle(bytes: &[u8], offset: usize) -> voplay_protocol::Handle {
    voplay_protocol::Handle {
        index: read_u32(bytes, offset),
        generation: read_u32(bytes, offset + 4),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn render_event_outcome_tag(outcome: RenderEventOutcome) -> u8 {
    match outcome {
        RenderEventOutcome::Executed => 1,
        RenderEventOutcome::DroppedBeforeDispatch => 2,
        RenderEventOutcome::OutcomeUnknown => 3,
        RenderEventOutcome::Failed => 4,
        RenderEventOutcome::FailedControlUnavailable => 5,
        RenderEventOutcome::DeadlineExceeded => 6,
    }
}
