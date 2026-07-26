use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use voplay_render_core::{
    NativeRenderDevice, NativeRenderOwner, NativeRenderOwnerConfig, NativeRenderSubmission,
    NativeSurfaceBinding, WgpuRenderDevice, WgpuRenderDeviceConfig,
};
use voplay_runtime::{
    control::StableControlRef,
    host_render_backend::{
        decode_host_render_command, encode_host_render_result, HostRenderCommand, HostRenderResult,
    },
    platform_backend::{PlatformAdapterError, PlatformRenderBackend, PlatformRenderBackendConfig},
    render_graph::TextureFormat,
    render_readback::{ReadbackFormat, ReadbackRegion, ReadbackRequestId},
    surface::{
        GameSurfaceId, PresentOutcome, RenderBackend, RenderBackendFailure, RenderBackendReadback,
        RenderFrameAck, RenderFrameSubmission, SurfaceMetrics,
    },
};

use crate::{
    decode_scene_frame_3d, encode_native_frame_trace_3d, is_scene_frame_3d, ShadowCascadeRender,
    TextureUpload3d, WgpuAssetResidency3d, WgpuAssetResidency3dConfig, WgpuFrameTrace3d,
    WgpuSceneRenderer, WgpuSceneRendererConfig,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuGameRenderDevice3dConfig {
    pub surface: WgpuRenderDeviceConfig,
    pub scene: WgpuSceneRendererConfig,
    pub assets: WgpuAssetResidency3dConfig,
    pub color_format: wgpu::TextureFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostRenderCommandOutcome3d {
    Accepted,
    Rendered(RenderFrameAck),
    Presented(PresentOutcome),
    ReadbackQueued(ReadbackRequestId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuViewFrameTrace3d {
    pub target: StableControlRef,
    pub viewport: [u32; 4],
    pub trace: WgpuFrameTrace3d,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuNativeFrameTrace3d {
    pub frame_id: u64,
    pub graph_signature: u64,
    pub views: Vec<WgpuViewFrameTrace3d>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostRenderCommandProcessor3dMetrics {
    pub pending_frame_traces: usize,
    pub awaiting_frame_traces: usize,
    pub ready_frame_traces: usize,
    pub capture_readbacks: usize,
    pub peak_pending_frame_traces: usize,
    pub peak_awaiting_frame_traces: usize,
    pub peak_ready_frame_traces: usize,
    pub peak_capture_readbacks: usize,
    pub requested_frame_traces: u64,
    pub cancelled_frame_traces: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostRenderCommandProcessor3dOwnerSnapshot {
    pub engine: voplay_protocol::EngineId,
    pub closed: bool,
    pub metrics: HostRenderCommandProcessor3dMetrics,
}

pub struct HostRenderCommandProcessor3d {
    engine: voplay_protocol::EngineId,
    backend: PlatformRenderBackend<NativeRenderOwner<WgpuGameRenderDevice3d>>,
    pending_frame_traces: BTreeMap<u64, PendingHostFrameTrace3d>,
    frame_trace_results: BTreeMap<u64, Vec<u8>>,
    awaiting_frame_traces: BTreeMap<u64, AwaitingHostFrameTrace3d>,
    next_capture_readback: u64,
    trace_metrics: HostRenderCommandProcessor3dMetrics,
    closed: bool,
}

#[derive(Clone, Copy)]
struct PendingHostFrameTrace3d {
    frame_id: u64,
    graph_signature: u64,
    include_attachments: bool,
    include_shader_diagnostics: bool,
    max_bytes: usize,
}

struct AwaitingHostFrameTrace3d {
    trace: WgpuNativeFrameTrace3d,
    max_bytes: usize,
    readbacks: Vec<(ReadbackRequestId, usize)>,
}

impl HostRenderCommandProcessor3d {
    pub fn new(
        config: WgpuGameRenderDevice3dConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        engine: voplay_protocol::EngineId,
        device_generation: u64,
    ) -> Result<Self, PlatformAdapterError> {
        let surface_limit = config.surface.max_surfaces;
        let device = WgpuGameRenderDevice3d::new(config, device, queue, engine, device_generation)?;
        let owner = NativeRenderOwner::new(
            NativeRenderOwnerConfig {
                max_surfaces: surface_limit,
                max_command_bytes: 64 * 1024 * 1024,
            },
            device,
        )?;
        let backend = PlatformRenderBackend::new(
            PlatformRenderBackendConfig {
                max_surfaces: surface_limit,
            },
            owner,
        )?;
        Ok(Self {
            engine,
            backend,
            pending_frame_traces: BTreeMap::new(),
            frame_trace_results: BTreeMap::new(),
            awaiting_frame_traces: BTreeMap::new(),
            next_capture_readback: u64::MAX,
            trace_metrics: HostRenderCommandProcessor3dMetrics::default(),
            closed: false,
        })
    }

    pub fn apply(
        &mut self,
        bytes: &[u8],
    ) -> Result<HostRenderCommandOutcome3d, RenderBackendFailure> {
        if self.closed {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        match decode_host_render_command(bytes)? {
            HostRenderCommand::Attach { surface, metrics } => {
                self.backend.attach_surface(surface, metrics)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::Resize { surface, metrics } => {
                self.backend.resize_surface(surface, metrics)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::Rebind { surface, metrics } => {
                self.backend.rebind_surface(surface, metrics)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::UploadTexture {
                texture,
                width,
                height,
                pixels,
            } => {
                self.backend
                    .upload_rgba_texture(texture, width, height, &pixels)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::RemoveTexture { texture } => {
                self.backend.remove_texture(texture)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::UpsertProfileAsset {
                kind,
                asset,
                revision,
                bytes,
            } => {
                self.backend
                    .upsert_profile_asset(kind, asset, revision, &bytes)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::RemoveProfileAsset {
                kind,
                asset,
                revision,
            } => {
                self.backend.remove_profile_asset(kind, asset, revision)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::SynchronizeTargets { targets } => {
                self.backend.synchronize_render_targets(&targets)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::Render(frame) => {
                let ack = self.backend.render(&frame)?;
                self.complete_frame_traces(
                    frame.frame_id,
                    frame.graph_signature,
                    frame.required_render_revision,
                )?;
                Ok(HostRenderCommandOutcome3d::Rendered(ack))
            }
            HostRenderCommand::Present {
                ack,
                now_micros,
                deadline_micros,
            } => self
                .backend
                .present(ack, now_micros, deadline_micros)
                .map(HostRenderCommandOutcome3d::Presented),
            HostRenderCommand::RequestReadback {
                request,
                target,
                expected_target_revision,
                region,
                format,
            } => {
                self.backend.request_readback(
                    request,
                    target,
                    expected_target_revision,
                    region,
                    format,
                )?;
                Ok(HostRenderCommandOutcome3d::ReadbackQueued(request))
            }
            HostRenderCommand::CancelReadback { request } => {
                self.backend.cancel_readback(request)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::RequestFrameTrace {
                request,
                frame_id,
                graph_signature,
                include_attachments,
                include_shader_diagnostics,
                max_bytes,
            } => {
                if self
                    .pending_frame_traces
                    .insert(
                        request,
                        PendingHostFrameTrace3d {
                            frame_id,
                            graph_signature,
                            include_attachments,
                            include_shader_diagnostics,
                            max_bytes,
                        },
                    )
                    .is_some()
                {
                    return Err(RenderBackendFailure::RejectedBeforeSubmit);
                }
                self.trace_metrics.requested_frame_traces =
                    self.trace_metrics.requested_frame_traces.saturating_add(1);
                self.refresh_trace_metrics();
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::CancelFrameTrace { request } => {
                let pending = self.pending_frame_traces.remove(&request).is_some();
                let awaiting = self.awaiting_frame_traces.remove(&request);
                if !pending && awaiting.is_none() {
                    return Err(RenderBackendFailure::RejectedBeforeSubmit);
                }
                if let Some(awaiting) = awaiting {
                    for (readback, _) in awaiting.readbacks {
                        let _ = self.backend.cancel_readback(readback);
                    }
                }
                self.frame_trace_results.remove(&request);
                self.trace_metrics.cancelled_frame_traces =
                    self.trace_metrics.cancelled_frame_traces.saturating_add(1);
                self.refresh_trace_metrics();
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
            HostRenderCommand::Detach { surface } => {
                self.backend.detach_surface(surface)?;
                Ok(HostRenderCommandOutcome3d::Accepted)
            }
        }
    }

    pub fn poll_readback_result(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<Vec<u8>>, RenderBackendFailure> {
        match self.backend.poll_readback(request) {
            Ok(Some(readback)) => {
                encode_host_render_result(&HostRenderResult::Readback(readback)).map(Some)
            }
            Ok(None) => Ok(None),
            Err(failure) => {
                encode_host_render_result(&HostRenderResult::ReadbackFailed { request, failure })
                    .map(Some)
            }
        }
    }

    pub fn poll_frame_trace_result(
        &mut self,
        request: u64,
    ) -> Result<Option<Vec<u8>>, RenderBackendFailure> {
        if let Some(bytes) = self.frame_trace_results.remove(&request) {
            self.refresh_trace_metrics();
            return Ok(Some(bytes));
        }
        let Some(mut awaiting) = self.awaiting_frame_traces.remove(&request) else {
            return Ok(None);
        };
        let readback_index = 0;
        while readback_index < awaiting.readbacks.len() {
            let (readback, view_index) = awaiting.readbacks[readback_index];
            match self.backend.poll_readback(readback) {
                Ok(Some(result)) => {
                    let attachment = awaiting
                        .trace
                        .views
                        .get_mut(view_index)
                        .and_then(|view| view.trace.attachments.first_mut())
                        .ok_or(RenderBackendFailure::OutcomeUnknown)?;
                    attachment.row_bytes = result.row_bytes;
                    attachment.bytes = result.bytes;
                    awaiting.readbacks.remove(readback_index);
                }
                Ok(None) => {
                    self.awaiting_frame_traces.insert(request, awaiting);
                    self.refresh_trace_metrics();
                    return Ok(None);
                }
                Err(failure) => {
                    let encoded = encode_host_render_result(&HostRenderResult::FrameTraceFailed {
                        request,
                        failure,
                    })?;
                    return Ok(Some(encoded));
                }
            }
        }
        let trace_bytes = encode_native_frame_trace_3d(&awaiting.trace)
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
        let result = if trace_bytes.len() <= awaiting.max_bytes {
            HostRenderResult::FrameTrace {
                request,
                frame_id: awaiting.trace.frame_id,
                graph_signature: awaiting.trace.graph_signature,
                bytes: trace_bytes,
            }
        } else {
            HostRenderResult::FrameTraceFailed {
                request,
                failure: RenderBackendFailure::RejectedBeforeSubmit,
            }
        };
        let encoded = encode_host_render_result(&result).map(Some);
        self.refresh_trace_metrics();
        encoded
    }

    pub fn poll_ready_frame_trace_results(
        &mut self,
        budget: usize,
    ) -> Result<Vec<Vec<u8>>, RenderBackendFailure> {
        if budget == 0 {
            return Ok(Vec::new());
        }
        let requests = self
            .frame_trace_results
            .keys()
            .copied()
            .chain(self.awaiting_frame_traces.keys().copied())
            .take(budget)
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for request in requests {
            if let Some(bytes) = self.poll_frame_trace_result(request)? {
                results.push(bytes);
            }
        }
        Ok(results)
    }

    fn complete_frame_traces(
        &mut self,
        frame_id: u64,
        graph_signature: u64,
        target_revision: u64,
    ) -> Result<(), RenderBackendFailure> {
        let requests = self
            .pending_frame_traces
            .iter()
            .filter(|(_, pending)| pending.frame_id == frame_id)
            .map(|(request, pending)| (*request, *pending))
            .collect::<Vec<_>>();
        if requests.is_empty() {
            return Ok(());
        }
        let trace = self
            .backend
            .adapter_mut()
            .device_mut()
            .take_last_frame_trace()
            .ok_or(RenderBackendFailure::OutcomeUnknown)?;
        for (request, pending) in requests {
            self.pending_frame_traces.remove(&request);
            let result = if pending.graph_signature != graph_signature {
                HostRenderResult::FrameTraceFailed {
                    request,
                    failure: RenderBackendFailure::RejectedBeforeSubmit,
                }
            } else {
                let mut requested_trace = trace.clone();
                if !pending.include_attachments {
                    for view in &mut requested_trace.views {
                        view.trace.attachments.clear();
                    }
                }
                if !pending.include_shader_diagnostics {
                    for view in &mut requested_trace.views {
                        view.trace.shader_diagnostics.clear();
                    }
                }
                if pending.include_attachments {
                    let mut readbacks = Vec::with_capacity(requested_trace.views.len());
                    for (view_index, view) in requested_trace.views.iter().enumerate() {
                        let readback = ReadbackRequestId(self.next_capture_readback);
                        self.next_capture_readback = self
                            .next_capture_readback
                            .checked_sub(1)
                            .filter(|next| *next != 0)
                            .ok_or(RenderBackendFailure::OutcomeUnknown)?;
                        self.backend.request_readback(
                            readback,
                            view.target,
                            target_revision,
                            ReadbackRegion {
                                x: view.viewport[0],
                                y: view.viewport[1],
                                width: view.viewport[2],
                                height: view.viewport[3],
                            },
                            ReadbackFormat::Rgba8,
                        )?;
                        readbacks.push((readback, view_index));
                    }
                    self.awaiting_frame_traces.insert(
                        request,
                        AwaitingHostFrameTrace3d {
                            trace: requested_trace,
                            max_bytes: pending.max_bytes,
                            readbacks,
                        },
                    );
                    continue;
                }
                match encode_native_frame_trace_3d(&requested_trace) {
                    Ok(bytes) if bytes.len() <= pending.max_bytes => HostRenderResult::FrameTrace {
                        request,
                        frame_id,
                        graph_signature,
                        bytes,
                    },
                    _ => HostRenderResult::FrameTraceFailed {
                        request,
                        failure: RenderBackendFailure::RejectedBeforeSubmit,
                    },
                }
            };
            let encoded = encode_host_render_result(&result)?;
            self.frame_trace_results.insert(request, encoded);
        }
        self.refresh_trace_metrics();
        Ok(())
    }

    pub fn owner_snapshot(&self) -> HostRenderCommandProcessor3dOwnerSnapshot {
        let mut metrics = self.trace_metrics;
        metrics.pending_frame_traces = self.pending_frame_traces.len();
        metrics.awaiting_frame_traces = self.awaiting_frame_traces.len();
        metrics.ready_frame_traces = self.frame_trace_results.len();
        metrics.capture_readbacks = self.capture_readback_count();
        HostRenderCommandProcessor3dOwnerSnapshot {
            engine: self.engine,
            closed: self.closed,
            metrics,
        }
    }

    pub fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.shutdown_frame_traces();
        self.backend.shutdown();
        self.closed = true;
    }

    pub fn shutdown_frame_traces(&mut self) {
        let cancelled = self
            .pending_frame_traces
            .len()
            .saturating_add(self.awaiting_frame_traces.len());
        for awaiting in self.awaiting_frame_traces.values() {
            for (readback, _) in &awaiting.readbacks {
                let _ = self.backend.cancel_readback(*readback);
            }
        }
        self.pending_frame_traces.clear();
        self.awaiting_frame_traces.clear();
        self.frame_trace_results.clear();
        self.trace_metrics.cancelled_frame_traces = self
            .trace_metrics
            .cancelled_frame_traces
            .saturating_add(cancelled as u64);
        self.refresh_trace_metrics();
    }

    fn capture_readback_count(&self) -> usize {
        self.awaiting_frame_traces
            .values()
            .map(|awaiting| awaiting.readbacks.len())
            .sum()
    }

    fn refresh_trace_metrics(&mut self) {
        self.trace_metrics.pending_frame_traces = self.pending_frame_traces.len();
        self.trace_metrics.awaiting_frame_traces = self.awaiting_frame_traces.len();
        self.trace_metrics.ready_frame_traces = self.frame_trace_results.len();
        self.trace_metrics.capture_readbacks = self.capture_readback_count();
        self.trace_metrics.peak_pending_frame_traces = self
            .trace_metrics
            .peak_pending_frame_traces
            .max(self.trace_metrics.pending_frame_traces);
        self.trace_metrics.peak_awaiting_frame_traces = self
            .trace_metrics
            .peak_awaiting_frame_traces
            .max(self.trace_metrics.awaiting_frame_traces);
        self.trace_metrics.peak_ready_frame_traces = self
            .trace_metrics
            .peak_ready_frame_traces
            .max(self.trace_metrics.ready_frame_traces);
        self.trace_metrics.peak_capture_readbacks = self
            .trace_metrics
            .peak_capture_readbacks
            .max(self.trace_metrics.capture_readbacks);
    }

    pub fn take_last_frame_trace(&mut self) -> Option<WgpuNativeFrameTrace3d> {
        self.backend
            .adapter_mut()
            .device_mut()
            .take_last_frame_trace()
    }

    pub fn composition_output(&self, surface: GameSurfaceId) -> Option<NativeRenderSubmission> {
        self.backend.adapter().composition_output(surface)
    }

    pub const fn backend(
        &self,
    ) -> &PlatformRenderBackend<NativeRenderOwner<WgpuGameRenderDevice3d>> {
        &self.backend
    }

    pub fn backend_mut(
        &mut self,
    ) -> &mut PlatformRenderBackend<NativeRenderOwner<WgpuGameRenderDevice3d>> {
        &mut self.backend
    }
}

impl Default for WgpuGameRenderDevice3dConfig {
    fn default() -> Self {
        Self {
            surface: WgpuRenderDeviceConfig::default(),
            scene: WgpuSceneRendererConfig::default(),
            assets: WgpuAssetResidency3dConfig::default(),
            color_format: wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }
}

pub struct WgpuGameRenderDevice3d {
    surface: WgpuRenderDevice,
    scene: WgpuSceneRenderer,
    scene_config: WgpuSceneRendererConfig,
    scene_device: Arc<wgpu::Device>,
    scene_queue: Arc<wgpu::Queue>,
    scene_profiles: BTreeMap<(TextureFormat, u8), WgpuSceneRenderer>,
    desired_textures: BTreeMap<u64, (u32, u32, Vec<u8>)>,
    assets: WgpuAssetResidency3d,
    max_scene_views: usize,
    max_scene_draws: usize,
    max_scene_lights: usize,
    max_scene_bytes: usize,
    dynamic_meshes: BTreeSet<u64>,
    last_frame_trace: Option<WgpuNativeFrameTrace3d>,
}

impl WgpuGameRenderDevice3d {
    pub fn new(
        config: WgpuGameRenderDevice3dConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        engine: voplay_protocol::EngineId,
        device_generation: u64,
    ) -> Result<Self, PlatformAdapterError> {
        if config.color_format != wgpu::TextureFormat::Rgba8UnormSrgb {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let max_scene_views = config.surface.max_targets;
        let max_scene_bytes = config.surface.max_texture_bytes;
        let surface = WgpuRenderDevice::new(
            config.surface,
            Arc::clone(&device),
            Arc::clone(&queue),
            device_generation,
        )?;
        let scene = WgpuSceneRenderer::new(
            config.scene,
            Arc::clone(&device),
            Arc::clone(&queue),
            config.color_format,
        )
        .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        let assets = WgpuAssetResidency3d::new(engine, config.assets)
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        Ok(Self {
            surface,
            scene,
            scene_config: config.scene,
            scene_device: device,
            scene_queue: queue,
            scene_profiles: BTreeMap::new(),
            desired_textures: BTreeMap::new(),
            assets,
            max_scene_views,
            max_scene_draws: 1_000_000,
            max_scene_lights: 16_384,
            max_scene_bytes,
            dynamic_meshes: BTreeSet::new(),
            last_frame_trace: None,
        })
    }

    fn primary_scene_key(&self) -> (TextureFormat, u8) {
        (
            TextureFormat::Rgba8,
            u8::try_from(self.scene_config.sample_count).unwrap_or(1),
        )
    }

    fn ensure_scene_profile(
        &mut self,
        key: (TextureFormat, u8),
    ) -> Result<&mut WgpuSceneRenderer, PlatformAdapterError> {
        if key == self.primary_scene_key() {
            return Ok(&mut self.scene);
        }
        if !self.scene_profiles.contains_key(&key) {
            if self.scene_profiles.len() >= 7 {
                return Err(PlatformAdapterError::RejectedBeforeSubmit);
            }
            let mut config = self.scene_config;
            config.sample_count = u32::from(key.1);
            let format = match key.0 {
                TextureFormat::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
                TextureFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
                TextureFormat::Depth24 | TextureFormat::Depth32Float => {
                    return Err(PlatformAdapterError::RejectedBeforeSubmit);
                }
            };
            let mut scene = WgpuSceneRenderer::new(
                config,
                Arc::clone(&self.scene_device),
                Arc::clone(&self.scene_queue),
                format,
            )
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            self.assets
                .recover(&mut scene)
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            for (texture, (width, height, pixels)) in &self.desired_textures {
                scene
                    .upsert_texture(TextureUpload3d {
                        id: *texture,
                        width: *width,
                        height: *height,
                        rgba8: pixels,
                        srgb: true,
                    })
                    .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            }
            self.scene_profiles.insert(key, scene);
        }
        self.scene_profiles
            .get_mut(&key)
            .ok_or(PlatformAdapterError::OutcomeUnknown)
    }

    pub const fn surface_device(&self) -> &WgpuRenderDevice {
        &self.surface
    }

    pub fn surface_device_mut(&mut self) -> &mut WgpuRenderDevice {
        &mut self.surface
    }

    pub const fn scene_renderer(&self) -> &WgpuSceneRenderer {
        &self.scene
    }

    pub fn scene_renderer_mut(&mut self) -> &mut WgpuSceneRenderer {
        &mut self.scene
    }

    pub const fn asset_residency(&self) -> &WgpuAssetResidency3d {
        &self.assets
    }

    pub fn take_last_frame_trace(&mut self) -> Option<WgpuNativeFrameTrace3d> {
        self.last_frame_trace.take()
    }
}

impl NativeRenderDevice for WgpuGameRenderDevice3d {
    fn shutdown(&mut self) -> Result<(), PlatformAdapterError> {
        self.last_frame_trace = None;
        self.dynamic_meshes.clear();
        self.desired_textures.clear();
        self.assets.shutdown();
        self.scene.shutdown();
        for scene in self.scene_profiles.values_mut() {
            scene.shutdown();
        }
        self.scene_profiles.clear();
        <WgpuRenderDevice as NativeRenderDevice>::shutdown(&mut self.surface)
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), PlatformAdapterError> {
        self.surface.synchronize_render_targets(targets)
    }

    fn upload_rgba_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        let upload = TextureUpload3d {
            id: texture,
            width,
            height,
            rgba8: pixels,
            srgb: true,
        };
        self.scene
            .upsert_texture(upload)
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        for scene in self.scene_profiles.values_mut() {
            scene
                .upsert_texture(upload)
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        }
        if let Err(error) = self
            .surface
            .upload_rgba_texture(texture, width, height, pixels)
        {
            self.scene.remove_texture(texture);
            for scene in self.scene_profiles.values_mut() {
                scene.remove_texture(texture);
            }
            return Err(error);
        }
        self.desired_textures
            .insert(texture, (width, height, pixels.to_vec()));
        Ok(())
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), PlatformAdapterError> {
        self.scene.remove_texture(texture);
        for scene in self.scene_profiles.values_mut() {
            scene.remove_texture(texture);
        }
        self.desired_textures.remove(&texture);
        self.surface.remove_texture(texture);
        Ok(())
    }

    fn upsert_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        self.assets
            .upsert(&mut self.scene, kind, asset, revision, bytes)
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        for scene in self.scene_profiles.values_mut() {
            self.assets
                .recover(scene)
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        }
        Ok(())
    }

    fn remove_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), PlatformAdapterError> {
        self.assets
            .remove(&mut self.scene, kind, asset, revision)
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        for scene in self.scene_profiles.values_mut() {
            self.assets
                .realize_remove(scene, kind, asset)
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        }
        Ok(())
    }

    fn request_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        self.surface
            .request_readback(request, target, expected_target_revision, region, format)
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        self.surface.poll_readback(request)
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), PlatformAdapterError> {
        self.surface.cancel_readback(request)
    }

    fn attach(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<NativeSurfaceBinding, PlatformAdapterError> {
        self.surface.attach(surface, metrics)
    }

    fn resize(
        &mut self,
        binding: NativeSurfaceBinding,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError> {
        self.surface.resize(binding, metrics)
    }

    fn submit(
        &mut self,
        binding: NativeSurfaceBinding,
        frame: &RenderFrameSubmission,
    ) -> Result<NativeRenderSubmission, PlatformAdapterError> {
        self.last_frame_trace = None;
        if !is_scene_frame_3d(&frame.commands) {
            return self.surface.submit(binding, frame);
        }
        let scene_frame = decode_scene_frame_3d(
            &frame.commands,
            frame.surface.engine,
            self.max_scene_views,
            self.max_scene_draws,
            self.max_scene_lights,
            self.max_scene_bytes,
        )
        .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        let next_dynamic_meshes = scene_frame
            .dynamic_meshes
            .iter()
            .map(|mesh| mesh.descriptor.id)
            .collect::<BTreeSet<_>>();
        if scene_frame
            .views
            .iter()
            .filter(|view| view.external_surface)
            .count()
            != 1
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let mut targets = BTreeSet::new();
        let mut prepared_profiles = BTreeSet::new();
        let mut fence = 0;
        let mut view_traces = Vec::with_capacity(scene_frame.views.len());
        for view in &scene_frame.views {
            if !matches!(
                view.color_format,
                TextureFormat::Rgba8 | TextureFormat::Rgba16Float
            ) || view
                .depth_format
                .is_some_and(|format| format != TextureFormat::Depth32Float)
                || !matches!(view.sample_count, 1 | 2 | 4 | 8)
                || view.depth_format.is_some() && view.sample_count != 1
                || view.external_surface
                    && (view.color_format != TextureFormat::Rgba8 || view.sample_count != 1)
            {
                return Err(PlatformAdapterError::RejectedBeforeSubmit);
            }
            let first_for_target = targets.insert(view.target);
            if first_for_target {
                self.surface.prepare_scene_target(
                    binding,
                    view.target,
                    view.width,
                    view.height,
                    view.external_surface,
                    frame.required_render_revision,
                    view.color_format,
                    view.depth_format,
                    view.sample_count,
                    view.usage,
                )?;
            }
            let target = self
                .surface
                .scene_target_texture(binding, view.target, view.external_surface)?
                .clone();
            let depth_target = self
                .surface
                .scene_target_depth_texture(binding, view.target)?
                .cloned();
            let profile = (view.color_format, view.sample_count);
            let first_for_profile = prepared_profiles.insert(profile);
            let dynamic_meshes = &scene_frame.dynamic_meshes;
            let scene = self.ensure_scene_profile(profile)?;
            if first_for_profile {
                for mesh in dynamic_meshes {
                    scene
                        .upsert_mesh_artifact(mesh)
                        .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
                }
            }
            let device = scene.device();
            let queue = scene.queue();
            if first_for_target {
                let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
                let depth_view = depth_target
                    .as_ref()
                    .map(|depth| depth.create_view(&wgpu::TextureViewDescriptor::default()));
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("voplay-3d-target-clear"),
                });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("voplay-3d-target-clear-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: view.clear_color[0] as f64,
                                    g: view.clear_color[1] as f64,
                                    b: view.clear_color[2] as f64,
                                    a: view.clear_color[3] as f64,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: depth_view.as_ref().map(|view| {
                            wgpu::RenderPassDepthStencilAttachment {
                                view,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: wgpu::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            }
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                queue.submit([encoder.finish()]);
            }
            let viewport_format = match view.color_format {
                TextureFormat::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
                TextureFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
                TextureFormat::Depth24 | TextureFormat::Depth32Float => {
                    return Err(PlatformAdapterError::RejectedBeforeSubmit);
                }
            };
            let viewport_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voplay-3d-view-output"),
                size: wgpu::Extent3d {
                    width: view.viewport[2],
                    height: view.viewport[3],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: viewport_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let viewport_target =
                viewport_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let shadow_cascades = view
                .shadow_cascades
                .iter()
                .map(|cascade| ShadowCascadeRender {
                    descriptor: cascade.descriptor,
                    light_view_projection: cascade.light_view_projection,
                    draws: &view.plan.shadow_casters,
                })
                .collect::<Vec<_>>();
            fence = scene
                .render_composed_with_shadows(
                    &view.plan,
                    &view.features,
                    &viewport_target,
                    view.viewport[2],
                    view.viewport[3],
                    view.view_projection,
                    view.clear_color,
                    view.camera_position_milli,
                    &shadow_cascades,
                    &view.render_features,
                    depth_target.as_ref().map(|depth| {
                        (
                            depth,
                            wgpu::Origin3d {
                                x: view.viewport[0],
                                y: view.viewport[1],
                                z: 0,
                            },
                        )
                    }),
                )
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            let trace = scene
                .take_last_frame_trace()
                .ok_or(PlatformAdapterError::OutcomeUnknown)?;
            view_traces.push(WgpuViewFrameTrace3d {
                target: view.target,
                viewport: view.viewport,
                trace,
            });
            let mut copy_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay-3d-view-composite"),
            });
            copy_encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &viewport_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &target,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: view.viewport[0],
                        y: view.viewport[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: view.viewport[2],
                    height: view.viewport[3],
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([copy_encoder.finish()]);
        }
        self.last_frame_trace = Some(WgpuNativeFrameTrace3d {
            frame_id: frame.frame_id,
            graph_signature: frame.graph_signature,
            views: view_traces,
        });
        for mesh in self.dynamic_meshes.difference(&next_dynamic_meshes) {
            self.scene.remove_mesh(*mesh);
            for scene in self.scene_profiles.values_mut() {
                scene.remove_mesh(*mesh);
            }
        }
        self.dynamic_meshes = next_dynamic_meshes;
        if scene_frame.native_overlay_commands == b"VFC1\0\0\0\0" {
            self.surface.finish_scene_submission(binding, frame, fence)
        } else {
            let mut overlay_frame = frame.clone();
            overlay_frame.commands = scene_frame.native_overlay_commands;
            self.surface.submit(binding, &overlay_frame)
        }
    }

    fn present(
        &mut self,
        binding: NativeSurfaceBinding,
        fence_value: u64,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, PlatformAdapterError> {
        self.surface
            .present(binding, fence_value, now_micros, deadline_micros)
    }

    fn detach(&mut self, binding: NativeSurfaceBinding) -> Result<(), PlatformAdapterError> {
        self.surface.detach(binding)
    }
}
