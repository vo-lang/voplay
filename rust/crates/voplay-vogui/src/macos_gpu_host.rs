use std::{collections::BTreeMap, sync::Arc};

use vo_app_host_native::{MacOsGpuWindow, WgpuCompositorAdapter, WgpuCompositorConfig};
use vo_app_protocol::{SurfaceHandle, ViewHandle};
use vo_app_runtime::{
    CompositionRegistry, NativeCompositionFrame, NativeCompositionOutcome, NativeCompositorConfig,
    NativeCompositorError, NativeCompositorOwner,
};
use vogui_native_accessibility::{
    AppKitAccessibilityActionReceiver, AppKitAccessibilitySink, AppKitFrameTransform,
    MacOsAccessibilityPlatform, NativeAccessibilityAdapter, NativeAccessibilityError,
};
use vogui_native_renderer::{NativePaintSubmission, WgpuUiPainter, WgpuUiPainterConfig};
use vogui_runtime::platform_renderer::PlatformApplyError;
use voplay_render_3d::{
    encode_native_frame_trace_3d, HostRenderCommandOutcome3d, HostRenderCommandProcessor3d,
    WgpuGameRenderDevice3dConfig, WgpuNativeFrameTrace3d,
};
use voplay_render_core::NativeRenderSubmission;
use voplay_runtime::{
    host_render_backend::{
        decode_host_render_command, decode_routed_host_render_command,
        encode_routed_host_render_result, HostRenderCommand,
    },
    surface::RenderBackendFailure,
};

use crate::{IntegrationError, NativeCompositionBuilder};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacOsGpuTopologyConfig {
    pub compositor: NativeCompositorConfig,
    pub wgpu: WgpuCompositorConfig,
    pub max_pending_accessibility_actions: usize,
}

impl Default for MacOsGpuTopologyConfig {
    fn default() -> Self {
        Self {
            compositor: NativeCompositorConfig::default(),
            wgpu: WgpuCompositorConfig::default(),
            max_pending_accessibility_actions: 256,
        }
    }
}

#[derive(Debug)]
pub enum MacOsGpuTopologyError {
    InvalidConfig,
    Surface(wgpu::CreateSurfaceError),
    Compositor(NativeCompositorError),
    Accessibility(NativeAccessibilityError),
    Render(RenderBackendFailure),
    Ui(PlatformApplyError),
    Integration(IntegrationError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VoplayLayerOwner {
    engine: voplay_protocol::EngineId,
    texture_token: u64,
    submission: NativeRenderSubmission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VoguiLayerOwner {
    texture_token: u64,
    submission: NativePaintSubmission,
}

struct VoplayRendererOwner {
    config: WgpuGameRenderDevice3dConfig,
    processor: HostRenderCommandProcessor3d,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacOsGpuTopologyOwnerSnapshot {
    pub view: ViewHandle,
    pub device_generation: u64,
    pub engines: Vec<voplay_protocol::EngineId>,
    pub layers: Vec<(
        vo_app_protocol::SurfaceHandle,
        voplay_protocol::EngineId,
        u64,
    )>,
    pub ui_layers: Vec<(vo_app_protocol::SurfaceHandle, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacOsGpuRecoveryReport {
    pub old_device_generation: u64,
    pub new_device_generation: u64,
    pub engines_requiring_state_replay: Vec<voplay_protocol::EngineId>,
    pub game_surfaces_requiring_rebind: Vec<SurfaceHandle>,
    pub ui_surfaces_requiring_rebind: Vec<SurfaceHandle>,
}

impl From<NativeCompositorError> for MacOsGpuTopologyError {
    fn from(error: NativeCompositorError) -> Self {
        Self::Compositor(error)
    }
}

impl From<NativeAccessibilityError> for MacOsGpuTopologyError {
    fn from(error: NativeAccessibilityError) -> Self {
        Self::Accessibility(error)
    }
}

impl From<IntegrationError> for MacOsGpuTopologyError {
    fn from(error: IntegrationError) -> Self {
        Self::Integration(error)
    }
}

impl From<PlatformApplyError> for MacOsGpuTopologyError {
    fn from(error: PlatformApplyError) -> Self {
        Self::Ui(error)
    }
}

/// Production owner for the certified `gpu-native-host` topology.
///
/// The caller owns the AppKit window so its CAMetalLayer outlives the wgpu
/// Surface retained by this topology. Vogui accessibility and Vogui/Voplay
/// textures share that same Window/View lifetime and compositor owner.
pub struct MacOsGpuTopology<'window> {
    view: ViewHandle,
    device_generation: u64,
    compositor: NativeCompositorOwner<WgpuCompositorAdapter<'window>>,
    accessibility: NativeAccessibilityAdapter<MacOsAccessibilityPlatform<AppKitAccessibilitySink>>,
    accessibility_actions: AppKitAccessibilityActionReceiver,
    voplay_renderers: BTreeMap<voplay_protocol::EngineId, VoplayRendererOwner>,
    voplay_layer_tokens: BTreeMap<vo_app_protocol::SurfaceHandle, VoplayLayerOwner>,
    vogui_layer_tokens: BTreeMap<vo_app_protocol::SurfaceHandle, VoguiLayerOwner>,
}

impl<'window> MacOsGpuTopology<'window> {
    #[allow(clippy::too_many_arguments)]
    pub fn attach(
        window: &'window MacOsGpuWindow,
        instance: &wgpu::Instance,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        device_generation: u64,
        config: MacOsGpuTopologyConfig,
    ) -> Result<Self, MacOsGpuTopologyError> {
        if device_generation == 0 || config.max_pending_accessibility_actions == 0 {
            return Err(MacOsGpuTopologyError::InvalidConfig);
        }
        let metrics = window.metrics();
        let width = device_extent(metrics.width_points, metrics.scale_factor)?;
        let height = device_extent(metrics.height_points, metrics.scale_factor)?;
        let surface = window
            .create_wgpu_surface(instance)
            .map_err(MacOsGpuTopologyError::Surface)?;
        let mut adapter_owner =
            WgpuCompositorAdapter::new(config.wgpu, adapter, device, queue, device_generation)?;
        adapter_owner.register_view_surface(window.view_handle(), surface, width, height)?;
        let mut compositor = NativeCompositorOwner::new(config.compositor, adapter_owner)?;
        compositor.attach_view(window.view_handle(), device_generation)?;

        let (sink, accessibility_actions) = AppKitAccessibilitySink::new(
            window.accessibility_host(),
            config.max_pending_accessibility_actions,
            AppKitFrameTransform {
                logical_units_per_point: 1_000.0,
                host_height_points: metrics.height_points,
                source_is_top_left: true,
            },
        )?;
        Ok(Self {
            view: window.view_handle(),
            device_generation,
            compositor,
            accessibility: NativeAccessibilityAdapter::new(MacOsAccessibilityPlatform::new(sink)),
            accessibility_actions,
            voplay_renderers: BTreeMap::new(),
            voplay_layer_tokens: BTreeMap::new(),
            vogui_layer_tokens: BTreeMap::new(),
        })
    }

    pub const fn view(&self) -> ViewHandle {
        self.view
    }

    pub const fn device_generation(&self) -> u64 {
        self.device_generation
    }

    pub fn compositor(&self) -> &NativeCompositorOwner<WgpuCompositorAdapter<'window>> {
        &self.compositor
    }

    pub fn compositor_mut(&mut self) -> &mut NativeCompositorOwner<WgpuCompositorAdapter<'window>> {
        &mut self.compositor
    }

    pub fn accessibility(
        &self,
    ) -> &NativeAccessibilityAdapter<MacOsAccessibilityPlatform<AppKitAccessibilitySink>> {
        &self.accessibility
    }

    pub fn accessibility_mut(
        &mut self,
    ) -> &mut NativeAccessibilityAdapter<MacOsAccessibilityPlatform<AppKitAccessibilitySink>> {
        &mut self.accessibility
    }

    pub const fn accessibility_actions(&self) -> &AppKitAccessibilityActionReceiver {
        &self.accessibility_actions
    }

    pub fn bind_voplay_engine(
        &mut self,
        engine: voplay_protocol::EngineId,
        config: WgpuGameRenderDevice3dConfig,
    ) -> Result<(), MacOsGpuTopologyError> {
        if !engine.is_valid() || self.voplay_renderers.contains_key(&engine) {
            return Err(MacOsGpuTopologyError::InvalidConfig);
        }
        let adapter = self.compositor.adapter();
        let renderer = HostRenderCommandProcessor3d::new(
            config,
            adapter.device(),
            adapter.queue(),
            engine,
            self.device_generation,
        )
        .map_err(|_| MacOsGpuTopologyError::InvalidConfig)?;
        self.voplay_renderers.insert(
            engine,
            VoplayRendererOwner {
                config,
                processor: renderer,
            },
        );
        Ok(())
    }

    pub fn unbind_voplay_engine(
        &mut self,
        engine: voplay_protocol::EngineId,
    ) -> Result<(), MacOsGpuTopologyError> {
        self.voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .shutdown_frame_traces();
        let owned = self
            .voplay_layer_tokens
            .iter()
            .filter(|(_, owner)| owner.engine == engine)
            .map(|(surface, owner)| (*surface, owner.texture_token))
            .collect::<Vec<_>>();
        for (surface, token) in owned {
            self.compositor
                .adapter_mut()?
                .unregister_layer_texture(surface, token)?;
            self.voplay_layer_tokens.remove(&surface);
        }
        self.voplay_renderers.remove(&engine);
        Ok(())
    }

    pub fn owner_snapshot(&self) -> MacOsGpuTopologyOwnerSnapshot {
        MacOsGpuTopologyOwnerSnapshot {
            view: self.view,
            device_generation: self.device_generation,
            engines: self.voplay_renderers.keys().copied().collect(),
            layers: self
                .voplay_layer_tokens
                .iter()
                .map(|(surface, owner)| (*surface, owner.engine, owner.texture_token))
                .collect(),
            ui_layers: self
                .vogui_layer_tokens
                .iter()
                .map(|(surface, owner)| (*surface, owner.texture_token))
                .collect(),
        }
    }

    pub fn create_vogui_painter(
        &self,
        config: WgpuUiPainterConfig,
    ) -> Result<WgpuUiPainter, MacOsGpuTopologyError> {
        Ok(WgpuUiPainter::new(
            config,
            self.compositor.adapter().device(),
            self.compositor.adapter().queue(),
            self.device_generation,
        )?)
    }

    pub fn rebind_device(
        &mut self,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        new_generation: u64,
    ) -> Result<MacOsGpuRecoveryReport, MacOsGpuTopologyError> {
        if new_generation <= self.device_generation {
            return Err(MacOsGpuTopologyError::InvalidConfig);
        }
        let mut replacements = BTreeMap::new();
        for (engine, owner) in &self.voplay_renderers {
            let processor = HostRenderCommandProcessor3d::new(
                owner.config,
                Arc::clone(&device),
                Arc::clone(&queue),
                *engine,
                new_generation,
            )
            .map_err(|_| MacOsGpuTopologyError::InvalidConfig)?;
            replacements.insert(
                *engine,
                VoplayRendererOwner {
                    config: owner.config,
                    processor,
                },
            );
        }
        let old_device_generation = self.device_generation;
        let game_surfaces_requiring_rebind = self.voplay_layer_tokens.keys().copied().collect();
        let ui_surfaces_requiring_rebind = self.vogui_layer_tokens.keys().copied().collect();
        self.compositor.adapter_mut()?.stage_device(
            Arc::clone(&device),
            Arc::clone(&queue),
            new_generation,
        )?;
        self.compositor.rebind_device(new_generation)?;
        self.voplay_renderers = replacements;
        self.voplay_layer_tokens.clear();
        self.vogui_layer_tokens.clear();
        self.device_generation = new_generation;
        Ok(MacOsGpuRecoveryReport {
            old_device_generation,
            new_device_generation: new_generation,
            engines_requiring_state_replay: self.voplay_renderers.keys().copied().collect(),
            game_surfaces_requiring_rebind,
            ui_surfaces_requiring_rebind,
        })
    }

    pub fn stage_vogui_painter_device(
        &self,
        painter: &mut WgpuUiPainter,
    ) -> Result<(), MacOsGpuTopologyError> {
        painter.stage_device(
            self.compositor.adapter().device(),
            self.compositor.adapter().queue(),
            self.device_generation,
        )?;
        Ok(())
    }

    pub fn publish_vogui_painter_layer(
        &mut self,
        surface: SurfaceHandle,
        submission: NativePaintSubmission,
        painter: &WgpuUiPainter,
    ) -> Result<(), MacOsGpuTopologyError> {
        let texture = painter
            .texture()
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .clone();
        self.publish_vogui_layer(surface, submission, texture)
    }

    pub fn publish_vogui_layer(
        &mut self,
        surface: SurfaceHandle,
        submission: NativePaintSubmission,
        texture: wgpu::Texture,
    ) -> Result<(), MacOsGpuTopologyError> {
        if !surface.is_valid()
            || submission.texture_token == 0
            || submission.fence_value == 0
            || submission.content_revision == 0
            || submission.device_generation != self.device_generation
            || self.voplay_layer_tokens.contains_key(&surface)
        {
            return Err(MacOsGpuTopologyError::InvalidConfig);
        }
        self.compositor.adapter_mut()?.register_layer_texture(
            surface,
            submission.texture_token,
            submission.device_generation,
            texture,
        )?;
        self.vogui_layer_tokens.insert(
            surface,
            VoguiLayerOwner {
                texture_token: submission.texture_token,
                submission,
            },
        );
        Ok(())
    }

    pub fn unpublish_vogui_layer(
        &mut self,
        surface: SurfaceHandle,
    ) -> Result<(), MacOsGpuTopologyError> {
        let owner = *self
            .vogui_layer_tokens
            .get(&surface)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?;
        self.compositor
            .adapter_mut()?
            .unregister_layer_texture(surface, owner.texture_token)?;
        self.vogui_layer_tokens.remove(&surface);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_shared_view_composition(
        &mut self,
        composition: &CompositionRegistry,
        pulse_id: u64,
        viewport_width_milli: u32,
        viewport_height_milli: u32,
        game_surface: SurfaceHandle,
        ui_surface: SurfaceHandle,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<NativeCompositionOutcome, MacOsGpuTopologyError> {
        let game = self
            .voplay_layer_tokens
            .get(&game_surface)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .submission;
        let ui = self
            .vogui_layer_tokens
            .get(&ui_surface)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .submission;
        let mut builder = NativeCompositionBuilder::new(
            self.view,
            pulse_id,
            self.device_generation,
            viewport_width_milli,
            viewport_height_milli,
            2,
        )?;
        builder.push_registered_voplay(composition, game_surface, game)?;
        builder.push_registered_vogui(composition, ui_surface, ui)?;
        self.submit_composition(builder.finish()?, now_micros, deadline_micros)
    }

    pub fn apply_voplay_render_command(
        &mut self,
        engine: voplay_protocol::EngineId,
        bytes: &[u8],
    ) -> Result<HostRenderCommandOutcome3d, MacOsGpuTopologyError> {
        let decoded = decode_host_render_command(bytes).map_err(MacOsGpuTopologyError::Render)?;
        let command_surface = match &decoded {
            HostRenderCommand::Attach { surface, .. }
            | HostRenderCommand::Resize { surface, .. }
            | HostRenderCommand::Rebind { surface, .. }
            | HostRenderCommand::Detach { surface }
            | HostRenderCommand::Present {
                ack: voplay_runtime::surface::RenderFrameAck { surface, .. },
                ..
            } => Some(*surface),
            HostRenderCommand::Render(submission) => Some(submission.surface),
            _ => None,
        };
        if let Some(surface) = command_surface {
            if surface.engine != engine
                || self.vogui_layer_tokens.contains_key(&surface.surface)
                || self
                    .voplay_layer_tokens
                    .get(&surface.surface)
                    .is_some_and(|owner| owner.engine != engine)
            {
                return Err(MacOsGpuTopologyError::InvalidConfig);
            }
        }
        let detached = matches!(decoded, HostRenderCommand::Detach { .. })
            .then_some(command_surface.unwrap().surface);
        let outcome = self
            .voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .apply(bytes)
            .map_err(MacOsGpuTopologyError::Render)?;
        if let HostRenderCommandOutcome3d::Rendered(ack) = outcome {
            let renderer = self
                .voplay_renderers
                .get(&engine)
                .ok_or(MacOsGpuTopologyError::InvalidConfig)?;
            let submission = renderer
                .processor
                .composition_output(ack.surface)
                .ok_or(MacOsGpuTopologyError::InvalidConfig)?;
            let texture = renderer
                .processor
                .backend()
                .adapter()
                .device()
                .surface_device()
                .texture(submission.texture_token)
                .ok_or(MacOsGpuTopologyError::InvalidConfig)?
                .clone();
            self.compositor.adapter_mut()?.register_layer_texture(
                ack.surface.surface,
                submission.texture_token,
                submission.device_generation,
                texture,
            )?;
            self.voplay_layer_tokens.insert(
                ack.surface.surface,
                VoplayLayerOwner {
                    engine,
                    texture_token: submission.texture_token,
                    submission,
                },
            );
        }
        if let Some(surface) = detached {
            if let Some(owner) = self.voplay_layer_tokens.get(&surface).copied() {
                self.compositor
                    .adapter_mut()?
                    .unregister_layer_texture(surface, owner.texture_token)?;
                self.voplay_layer_tokens.remove(&surface);
            }
        }
        Ok(outcome)
    }

    pub fn apply_routed_voplay_render_command(
        &mut self,
        bytes: &[u8],
    ) -> Result<(voplay_protocol::EngineId, HostRenderCommandOutcome3d), MacOsGpuTopologyError>
    {
        let (engine, command) =
            decode_routed_host_render_command(bytes).map_err(MacOsGpuTopologyError::Render)?;
        self.apply_voplay_render_command(engine, command)
            .map(|outcome| (engine, outcome))
    }

    pub fn poll_voplay_readback_result(
        &mut self,
        engine: voplay_protocol::EngineId,
        request: voplay_runtime::render_readback::ReadbackRequestId,
    ) -> Result<Option<Vec<u8>>, MacOsGpuTopologyError> {
        self.voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .poll_readback_result(request)
            .map_err(MacOsGpuTopologyError::Render)
    }

    pub fn poll_routed_voplay_readback_result(
        &mut self,
        engine: voplay_protocol::EngineId,
        request: voplay_runtime::render_readback::ReadbackRequestId,
    ) -> Result<Option<Vec<u8>>, MacOsGpuTopologyError> {
        self.poll_voplay_readback_result(engine, request)?
            .map(|result| {
                encode_routed_host_render_result(engine, &result)
                    .map_err(MacOsGpuTopologyError::Render)
            })
            .transpose()
    }

    pub fn take_voplay_frame_trace(
        &mut self,
        engine: voplay_protocol::EngineId,
    ) -> Result<Option<WgpuNativeFrameTrace3d>, MacOsGpuTopologyError> {
        Ok(self
            .voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .take_last_frame_trace())
    }

    pub fn take_voplay_frame_trace_wire(
        &mut self,
        engine: voplay_protocol::EngineId,
    ) -> Result<Option<Vec<u8>>, MacOsGpuTopologyError> {
        self.take_voplay_frame_trace(engine)?
            .map(|trace| {
                encode_native_frame_trace_3d(&trace)
                    .map_err(|_| MacOsGpuTopologyError::InvalidConfig)
            })
            .transpose()
    }

    pub fn poll_voplay_frame_trace_result(
        &mut self,
        engine: voplay_protocol::EngineId,
        request: u64,
    ) -> Result<Option<Vec<u8>>, MacOsGpuTopologyError> {
        if request == 0 {
            return Err(MacOsGpuTopologyError::InvalidConfig);
        }
        self.voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .poll_frame_trace_result(request)
            .map_err(MacOsGpuTopologyError::Render)
    }

    pub fn poll_routed_voplay_frame_trace_result(
        &mut self,
        engine: voplay_protocol::EngineId,
        request: u64,
    ) -> Result<Option<Vec<u8>>, MacOsGpuTopologyError> {
        self.poll_voplay_frame_trace_result(engine, request)?
            .map(|result| {
                encode_routed_host_render_result(engine, &result)
                    .map_err(MacOsGpuTopologyError::Render)
            })
            .transpose()
    }

    pub fn poll_voplay_frame_trace_results(
        &mut self,
        engine: voplay_protocol::EngineId,
        budget: usize,
    ) -> Result<Vec<Vec<u8>>, MacOsGpuTopologyError> {
        if budget == 0 {
            return Ok(Vec::new());
        }
        self.voplay_renderers
            .get_mut(&engine)
            .ok_or(MacOsGpuTopologyError::InvalidConfig)?
            .processor
            .poll_ready_frame_trace_results(budget)
            .map_err(MacOsGpuTopologyError::Render)
    }

    pub fn submit_composition(
        &mut self,
        frame: NativeCompositionFrame,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<NativeCompositionOutcome, MacOsGpuTopologyError> {
        let fence = self.compositor.submit(frame)?;
        Ok(self
            .compositor
            .present(fence, now_micros, deadline_micros)?)
    }

    pub fn resize(
        &mut self,
        metrics: vo_app_host_native::MacOsViewMetrics,
    ) -> Result<(), MacOsGpuTopologyError> {
        let width = device_extent(metrics.width_points, metrics.scale_factor)?;
        let height = device_extent(metrics.height_points, metrics.scale_factor)?;
        self.compositor
            .adapter_mut()?
            .resize_view(self.view, width, height)?;
        self.accessibility
            .platform_mut()
            .sink_mut()
            .set_frame_transform(AppKitFrameTransform {
                logical_units_per_point: 1_000.0,
                host_height_points: metrics.height_points,
                source_is_top_left: true,
            })?;
        Ok(())
    }

    pub fn close(mut self) -> Result<(), MacOsGpuTopologyError> {
        use vogui_runtime::accessibility::PlatformAccessibilityAdapter;

        self.accessibility.close();
        for (surface, owner) in std::mem::take(&mut self.vogui_layer_tokens) {
            self.compositor
                .adapter_mut()?
                .unregister_layer_texture(surface, owner.texture_token)?;
        }
        for (surface, owner) in std::mem::take(&mut self.voplay_layer_tokens) {
            self.compositor
                .adapter_mut()?
                .unregister_layer_texture(surface, owner.texture_token)?;
        }
        self.compositor.detach_view(self.view)?;
        self.compositor.close()?;
        Ok(())
    }
}

fn device_extent(points: f64, scale: f64) -> Result<u32, MacOsGpuTopologyError> {
    let value = points * scale;
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return Err(MacOsGpuTopologyError::InvalidConfig);
    }
    Ok(value.round().max(1.0) as u32)
}
