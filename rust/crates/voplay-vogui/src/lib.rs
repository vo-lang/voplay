use vo_app_protocol::{SurfaceHandle, ViewHandle};
use vo_app_runtime::{
    ArbitrationEvent, ArbitrationResult, BridgeFrame, BridgeRestartReport, BridgeState,
    BridgeTransport, BridgeTransportConfig, BridgeTransportError, CompositionError,
    CompositionPointerId, CompositionRegistry, NativeCompositionFrame, NativeLayerSubmission,
    SurfaceGeometry, SurfaceInputPolicy, SurfaceKind,
};
use vogui_native_renderer::NativePaintSubmission;
use voplay_render_core::NativeRenderSubmission;

#[cfg(all(target_os = "macos", feature = "macos-gpu-host"))]
mod macos_gpu_host;
#[cfg(all(target_os = "macos", feature = "macos-gpu-host"))]
pub use macos_gpu_host::{
    MacOsGpuRecoveryReport, MacOsGpuTopology, MacOsGpuTopologyConfig, MacOsGpuTopologyError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompositionLayerPolicy {
    pub z_order: i32,
    pub input: SurfaceInputPolicy,
    pub geometry: SurfaceGeometry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationError {
    InvalidConfig,
    LayerCapacity,
    DuplicateSurface,
    DeviceGenerationMismatch,
    EmptyFrame,
    WrongView,
    WrongSurfaceKind,
    LayerUnavailable,
    StaleLayerRevision,
    PulseRegression,
    InvalidRecoverySnapshot,
    Composition(CompositionError),
    Bridge(BridgeTransportError),
    Closed,
}

pub struct SharedNativeViewComposition {
    view: ViewHandle,
    game: SurfaceHandle,
    ui: SurfaceHandle,
    device_generation: u64,
    last_pulse_id: u64,
    game_submission: Option<NativeRenderSubmission>,
    ui_submission: Option<NativePaintSubmission>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedNativeViewCompositionOwnerSnapshot {
    pub view: ViewHandle,
    pub game: SurfaceHandle,
    pub ui: SurfaceHandle,
    pub device_generation: u64,
    pub last_pulse_id: u64,
    pub game_revision: Option<u64>,
    pub ui_revision: Option<u64>,
    pub closed: bool,
}

impl SharedNativeViewComposition {
    pub fn new(
        composition: &CompositionRegistry,
        view: ViewHandle,
        game: SurfaceHandle,
        ui: SurfaceHandle,
        device_generation: u64,
    ) -> Result<Self, IntegrationError> {
        validate_shared_surfaces(composition, view, game, ui)?;
        if device_generation == 0 {
            return Err(IntegrationError::InvalidConfig);
        }
        Ok(Self {
            view,
            game,
            ui,
            device_generation,
            last_pulse_id: 0,
            game_submission: None,
            ui_submission: None,
            closed: false,
        })
    }

    pub const fn view(&self) -> ViewHandle {
        self.view
    }

    pub const fn game_surface(&self) -> SurfaceHandle {
        self.game
    }

    pub const fn ui_surface(&self) -> SurfaceHandle {
        self.ui
    }

    pub const fn device_generation(&self) -> u64 {
        self.device_generation
    }

    pub fn owner_snapshot(&self) -> SharedNativeViewCompositionOwnerSnapshot {
        SharedNativeViewCompositionOwnerSnapshot {
            view: self.view,
            game: self.game,
            ui: self.ui,
            device_generation: self.device_generation,
            last_pulse_id: self.last_pulse_id,
            game_revision: self
                .game_submission
                .map(|submission| submission.content_revision),
            ui_revision: self
                .ui_submission
                .map(|submission| submission.content_revision),
            closed: self.closed,
        }
    }

    pub fn publish_voplay(
        &mut self,
        submission: NativeRenderSubmission,
    ) -> Result<(), IntegrationError> {
        self.ensure_open()?;
        if submission.device_generation != self.device_generation
            || submission.texture_token == 0
            || submission.fence_value == 0
            || submission.content_revision == 0
        {
            return Err(IntegrationError::DeviceGenerationMismatch);
        }
        if self.game_submission.is_some_and(|current| {
            current.content_revision > submission.content_revision
                || (current.content_revision == submission.content_revision
                    && current.fence_value >= submission.fence_value)
        }) {
            return Err(IntegrationError::StaleLayerRevision);
        }
        self.game_submission = Some(submission);
        Ok(())
    }

    pub fn publish_vogui(
        &mut self,
        submission: NativePaintSubmission,
    ) -> Result<(), IntegrationError> {
        self.ensure_open()?;
        if submission.device_generation != self.device_generation
            || submission.texture_token == 0
            || submission.fence_value == 0
            || submission.content_revision == 0
        {
            return Err(IntegrationError::DeviceGenerationMismatch);
        }
        if self.ui_submission.is_some_and(|current| {
            current.content_revision > submission.content_revision
                || (current.content_revision == submission.content_revision
                    && current.fence_value >= submission.fence_value)
        }) {
            return Err(IntegrationError::StaleLayerRevision);
        }
        self.ui_submission = Some(submission);
        Ok(())
    }

    pub fn build_frame(
        &mut self,
        composition: &CompositionRegistry,
        pulse_id: u64,
        viewport_width_milli: u32,
        viewport_height_milli: u32,
    ) -> Result<NativeCompositionFrame, IntegrationError> {
        self.ensure_open()?;
        if pulse_id <= self.last_pulse_id {
            return Err(IntegrationError::PulseRegression);
        }
        validate_shared_surfaces(composition, self.view, self.game, self.ui)?;
        let game = self
            .game_submission
            .ok_or(IntegrationError::LayerUnavailable)?;
        let ui = self
            .ui_submission
            .ok_or(IntegrationError::LayerUnavailable)?;
        let mut builder = NativeCompositionBuilder::new(
            self.view,
            pulse_id,
            self.device_generation,
            viewport_width_milli,
            viewport_height_milli,
            2,
        )?;
        builder.push_registered_voplay(composition, self.game, game)?;
        builder.push_registered_vogui(composition, self.ui, ui)?;
        let frame = builder.finish()?;
        self.last_pulse_id = pulse_id;
        Ok(frame)
    }

    pub fn rebind_surfaces(
        &mut self,
        composition: &CompositionRegistry,
        game: SurfaceHandle,
        ui: SurfaceHandle,
    ) -> Result<(), IntegrationError> {
        self.ensure_open()?;
        validate_shared_surfaces(composition, self.view, game, ui)?;
        self.game = game;
        self.ui = ui;
        self.game_submission = None;
        self.ui_submission = None;
        self.last_pulse_id = 0;
        Ok(())
    }

    pub fn rebind_device(&mut self, new_generation: u64) -> Result<(), IntegrationError> {
        self.ensure_open()?;
        if new_generation <= self.device_generation {
            return Err(IntegrationError::DeviceGenerationMismatch);
        }
        self.device_generation = new_generation;
        self.game_submission = None;
        self.ui_submission = None;
        self.last_pulse_id = 0;
        Ok(())
    }

    pub fn shutdown(&mut self) -> SharedNativeViewCompositionOwnerSnapshot {
        self.game_submission = None;
        self.ui_submission = None;
        self.closed = true;
        self.owner_snapshot()
    }

    fn ensure_open(&self) -> Result<(), IntegrationError> {
        if self.closed {
            Err(IntegrationError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_shared_surfaces(
    composition: &CompositionRegistry,
    view: ViewHandle,
    game: SurfaceHandle,
    ui: SurfaceHandle,
) -> Result<(), IntegrationError> {
    let game_descriptor = composition.surface_descriptor(game)?;
    let ui_descriptor = composition.surface_descriptor(ui)?;
    if game_descriptor.view != view || ui_descriptor.view != view {
        return Err(IntegrationError::WrongView);
    }
    if game_descriptor.kind != SurfaceKind::Game || ui_descriptor.kind != SurfaceKind::Ui {
        return Err(IntegrationError::WrongSurfaceKind);
    }
    Ok(())
}

impl From<CompositionError> for IntegrationError {
    fn from(error: CompositionError) -> Self {
        Self::Composition(error)
    }
}

impl From<BridgeTransportError> for IntegrationError {
    fn from(error: BridgeTransportError) -> Self {
        Self::Bridge(error)
    }
}

pub const WEBVIEW_RECOVERY_SNAPSHOT_KEY: u64 = 0x5650_5742_5245_4331;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebViewRecoverySnapshot<'a> {
    pub ui_model: &'a [u8],
    pub simulation_world: &'a [u8],
}

pub fn decode_webview_recovery_snapshot(
    bytes: &[u8],
) -> Result<WebViewRecoverySnapshot<'_>, IntegrationError> {
    if bytes.len() < 16 || &bytes[..5] != b"VWRC1" || bytes[5..8] != [0_u8; 3] {
        return Err(IntegrationError::InvalidRecoverySnapshot);
    }
    let ui_bytes = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| IntegrationError::InvalidRecoverySnapshot)?,
    ) as usize;
    let world_bytes = u32::from_le_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| IntegrationError::InvalidRecoverySnapshot)?,
    ) as usize;
    let ui_end = 16_usize
        .checked_add(ui_bytes)
        .ok_or(IntegrationError::InvalidRecoverySnapshot)?;
    let world_end = ui_end
        .checked_add(world_bytes)
        .filter(|end| *end == bytes.len())
        .ok_or(IntegrationError::InvalidRecoverySnapshot)?;
    Ok(WebViewRecoverySnapshot {
        ui_model: &bytes[16..ui_end],
        simulation_world: &bytes[ui_end..world_end],
    })
}

pub struct WebViewCompositionBridge {
    transport: BridgeTransport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebViewCompositionBridgeOwnerSnapshot {
    pub state: BridgeState,
    pub bridge_epoch: u64,
    pub pending_to_process_messages: usize,
    pub pending_to_process_bytes: usize,
    pub pending_from_process_messages: usize,
    pub pending_from_process_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebViewCompositionBridgeShutdownReport {
    pub before: WebViewCompositionBridgeOwnerSnapshot,
    pub discarded_to_process: usize,
    pub discarded_from_process: usize,
    pub after: WebViewCompositionBridgeOwnerSnapshot,
}

impl WebViewCompositionBridge {
    pub fn new(
        session: vo_app_protocol::SessionHandle,
        session_epoch: u64,
        config: BridgeTransportConfig,
    ) -> Result<Self, IntegrationError> {
        Ok(Self {
            transport: BridgeTransport::new(session, session_epoch, config)?,
        })
    }

    pub const fn bridge_epoch(&self) -> u64 {
        self.transport.bridge_epoch()
    }

    pub fn owner_snapshot(&self) -> WebViewCompositionBridgeOwnerSnapshot {
        let (pending_to_process_messages, pending_to_process_bytes) =
            self.transport.pending_to_webview();
        let (pending_from_process_messages, pending_from_process_bytes) =
            self.transport.pending_from_webview();
        WebViewCompositionBridgeOwnerSnapshot {
            state: self.transport.state(),
            bridge_epoch: self.transport.bridge_epoch(),
            pending_to_process_messages,
            pending_to_process_bytes,
            pending_from_process_messages,
            pending_from_process_bytes,
        }
    }

    pub fn attach_process(&mut self, bridge_epoch: u64) -> Result<(), IntegrationError> {
        self.transport.attach_webview(bridge_epoch)?;
        Ok(())
    }

    pub fn begin_process_restart(
        &mut self,
        ui_model_snapshot: &[u8],
        simulation_world_snapshot: &[u8],
    ) -> Result<(BridgeRestartReport, BridgeFrame), IntegrationError> {
        let ui_bytes =
            u32::try_from(ui_model_snapshot.len()).map_err(|_| IntegrationError::InvalidConfig)?;
        let world_bytes = u32::try_from(simulation_world_snapshot.len())
            .map_err(|_| IntegrationError::InvalidConfig)?;
        let capacity = 16_usize
            .checked_add(ui_model_snapshot.len())
            .and_then(|bytes| bytes.checked_add(simulation_world_snapshot.len()))
            .ok_or(IntegrationError::InvalidConfig)?;
        let mut recovery = Vec::with_capacity(capacity);
        recovery.extend_from_slice(b"VWRC1");
        recovery.extend_from_slice(&[0_u8; 3]);
        recovery.extend_from_slice(&ui_bytes.to_le_bytes());
        recovery.extend_from_slice(&world_bytes.to_le_bytes());
        recovery.extend_from_slice(ui_model_snapshot);
        recovery.extend_from_slice(simulation_world_snapshot);
        let mut snapshots = vec![(WEBVIEW_RECOVERY_SNAPSHOT_KEY, recovery)];
        self.transport
            .preflight_webview_restart_with_snapshots(&snapshots)?;
        let report = self.transport.begin_webview_restart()?;
        let (_, recovery) = snapshots.pop().unwrap();
        let frame = self
            .transport
            .enqueue_restart_snapshot(WEBVIEW_RECOVERY_SNAPSHOT_KEY, recovery)?;
        Ok((report, frame))
    }

    pub fn take_to_process(&mut self) -> Option<BridgeFrame> {
        self.transport.take_to_webview()
    }

    pub fn submit_from_process(&mut self, frame: BridgeFrame) -> Result<(), IntegrationError> {
        self.transport.submit_from_webview(frame)?;
        Ok(())
    }

    pub fn take_from_process(&mut self) -> Option<BridgeFrame> {
        self.transport.take_from_webview()
    }

    pub fn shutdown(&mut self) -> Result<WebViewCompositionBridgeShutdownReport, IntegrationError> {
        let before = self.owner_snapshot();
        let (discarded_to_process, discarded_from_process) = match self.transport.state() {
            BridgeState::Closed => (0, 0),
            BridgeState::Closing => self.transport.finish_close()?,
            _ => {
                self.transport.begin_close()?;
                self.transport.finish_close()?
            }
        };
        Ok(WebViewCompositionBridgeShutdownReport {
            before,
            discarded_to_process,
            discarded_from_process,
            after: self.owner_snapshot(),
        })
    }
}

pub struct NativeCompositionBuilder {
    view: ViewHandle,
    pulse_id: u64,
    device_generation: u64,
    viewport_width_milli: u32,
    viewport_height_milli: u32,
    max_layers: usize,
    layers: Vec<NativeLayerSubmission>,
}

impl NativeCompositionBuilder {
    pub fn new(
        view: ViewHandle,
        pulse_id: u64,
        device_generation: u64,
        viewport_width_milli: u32,
        viewport_height_milli: u32,
        max_layers: usize,
    ) -> Result<Self, IntegrationError> {
        if !view.is_valid()
            || pulse_id == 0
            || device_generation == 0
            || viewport_width_milli == 0
            || viewport_height_milli == 0
            || max_layers == 0
        {
            return Err(IntegrationError::InvalidConfig);
        }
        Ok(Self {
            view,
            pulse_id,
            device_generation,
            viewport_width_milli,
            viewport_height_milli,
            max_layers,
            layers: Vec::new(),
        })
    }

    pub fn push_voplay(
        &mut self,
        surface: SurfaceHandle,
        submission: NativeRenderSubmission,
        policy: CompositionLayerPolicy,
    ) -> Result<(), IntegrationError> {
        self.push(
            surface,
            SurfaceKind::Game,
            submission.texture_token,
            submission.content_revision,
            submission.device_generation,
            policy,
        )
    }

    pub fn push_registered_voplay(
        &mut self,
        composition: &CompositionRegistry,
        surface: SurfaceHandle,
        submission: NativeRenderSubmission,
    ) -> Result<(), IntegrationError> {
        let policy = self.registered_policy(composition, surface, SurfaceKind::Game)?;
        self.push_voplay(surface, submission, policy)
    }

    pub fn push_vogui(
        &mut self,
        surface: SurfaceHandle,
        submission: NativePaintSubmission,
        policy: CompositionLayerPolicy,
    ) -> Result<(), IntegrationError> {
        self.push(
            surface,
            SurfaceKind::Ui,
            submission.texture_token,
            submission.content_revision,
            submission.device_generation,
            policy,
        )
    }

    pub fn push_registered_vogui(
        &mut self,
        composition: &CompositionRegistry,
        surface: SurfaceHandle,
        submission: NativePaintSubmission,
    ) -> Result<(), IntegrationError> {
        let policy = self.registered_policy(composition, surface, SurfaceKind::Ui)?;
        self.push_vogui(surface, submission, policy)
    }

    pub fn finish(mut self) -> Result<NativeCompositionFrame, IntegrationError> {
        if self.layers.is_empty() {
            return Err(IntegrationError::EmptyFrame);
        }
        self.layers.sort_by(|left, right| {
            left.z_order
                .cmp(&right.z_order)
                .then(left.surface.index.cmp(&right.surface.index))
                .then(left.surface.generation.cmp(&right.surface.generation))
        });
        Ok(NativeCompositionFrame {
            view: self.view,
            pulse_id: self.pulse_id,
            device_generation: self.device_generation,
            viewport_width_milli: self.viewport_width_milli,
            viewport_height_milli: self.viewport_height_milli,
            layers: self.layers,
        })
    }

    fn push(
        &mut self,
        surface: SurfaceHandle,
        kind: SurfaceKind,
        texture_token: u64,
        content_revision: u64,
        device_generation: u64,
        policy: CompositionLayerPolicy,
    ) -> Result<(), IntegrationError> {
        if self.layers.len() == self.max_layers {
            return Err(IntegrationError::LayerCapacity);
        }
        if self.layers.iter().any(|layer| layer.surface == surface) {
            return Err(IntegrationError::DuplicateSurface);
        }
        if device_generation != self.device_generation {
            return Err(IntegrationError::DeviceGenerationMismatch);
        }
        if !surface.is_valid() || texture_token == 0 || content_revision == 0 {
            return Err(IntegrationError::InvalidConfig);
        }
        self.layers.push(NativeLayerSubmission {
            surface,
            kind,
            z_order: policy.z_order,
            input: policy.input,
            content_revision,
            texture_token,
            device_generation,
            geometry: policy.geometry,
        });
        Ok(())
    }

    fn registered_policy(
        &self,
        composition: &CompositionRegistry,
        surface: SurfaceHandle,
        expected_kind: SurfaceKind,
    ) -> Result<CompositionLayerPolicy, IntegrationError> {
        let descriptor = composition.surface_descriptor(surface)?;
        if descriptor.view != self.view {
            return Err(IntegrationError::WrongView);
        }
        if descriptor.kind != expected_kind {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        Ok(CompositionLayerPolicy {
            z_order: descriptor.z_order,
            input: descriptor.input,
            geometry: descriptor.geometry,
        })
    }
}

pub fn pointer_hit_stack(
    ordered_top_to_bottom: impl IntoIterator<Item = (SurfaceHandle, bool)>,
) -> ArbitrationEvent {
    ArbitrationEvent::PointerStack {
        hits: ordered_top_to_bottom
            .into_iter()
            .filter_map(|(surface, hit)| hit.then_some(surface))
            .collect(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayInteractionMode {
    Passthrough,
    InteractiveHud,
    Modal,
}

pub struct SharedViewInputController {
    view: ViewHandle,
    game: SurfaceHandle,
    ui: SurfaceHandle,
    mode: OverlayInteractionMode,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedViewInputOwnerSnapshot {
    pub view: ViewHandle,
    pub game: SurfaceHandle,
    pub ui: SurfaceHandle,
    pub mode: OverlayInteractionMode,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedViewInputShutdownReport {
    pub before: SharedViewInputOwnerSnapshot,
    pub released_pointer_captures: usize,
    pub final_composition_revision: u64,
    pub after: SharedViewInputOwnerSnapshot,
}

impl SharedViewInputController {
    pub fn new(
        composition: &CompositionRegistry,
        view: ViewHandle,
        game: SurfaceHandle,
        ui: SurfaceHandle,
    ) -> Result<Self, IntegrationError> {
        let game_descriptor = composition.surface_descriptor(game)?;
        let ui_descriptor = composition.surface_descriptor(ui)?;
        if game_descriptor.view != view || ui_descriptor.view != view {
            return Err(IntegrationError::WrongView);
        }
        if game_descriptor.kind != SurfaceKind::Game || ui_descriptor.kind != SurfaceKind::Ui {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        Ok(Self {
            view,
            game,
            ui,
            mode: match ui_descriptor.input {
                SurfaceInputPolicy::Passthrough | SurfaceInputPolicy::Observe => {
                    OverlayInteractionMode::Passthrough
                }
                SurfaceInputPolicy::Interactive => OverlayInteractionMode::InteractiveHud,
                SurfaceInputPolicy::Exclusive => OverlayInteractionMode::Modal,
            },
            closed: false,
        })
    }

    pub const fn mode(&self) -> OverlayInteractionMode {
        self.mode
    }

    pub const fn view(&self) -> ViewHandle {
        self.view
    }

    pub const fn game_surface(&self) -> SurfaceHandle {
        self.game
    }

    pub const fn ui_surface(&self) -> SurfaceHandle {
        self.ui
    }

    pub const fn owner_snapshot(&self) -> SharedViewInputOwnerSnapshot {
        SharedViewInputOwnerSnapshot {
            view: self.view,
            game: self.game,
            ui: self.ui,
            mode: self.mode,
            closed: self.closed,
        }
    }

    pub fn rebind_after_surface_restart(
        &mut self,
        composition: &mut CompositionRegistry,
        game: SurfaceHandle,
        ui: SurfaceHandle,
        restore_ui_text_focus: bool,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        let game_descriptor = composition.surface_descriptor(game)?;
        let ui_descriptor = composition.surface_descriptor(ui)?;
        if game_descriptor.view != self.view || ui_descriptor.view != self.view {
            return Err(IntegrationError::WrongView);
        }
        if game_descriptor.kind != SurfaceKind::Game || ui_descriptor.kind != SurfaceKind::Ui {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        let mode = self.mode;
        if restore_ui_text_focus {
            if mode == OverlayInteractionMode::Passthrough {
                return Err(IntegrationError::Composition(
                    CompositionError::NotInteractive,
                ));
            }
            if !ui_descriptor.accepts_text {
                return Err(IntegrationError::Composition(
                    CompositionError::TextUnsupported,
                ));
            }
            let state = composition.view_input_state(self.view)?;
            let captures = composition.view_pointer_captures(self.view)?;
            let releases = captures
                .iter()
                .filter(|(_, owner)| mode == OverlayInteractionMode::Modal || *owner == ui)
                .count();
            let mode_tail = match mode {
                OverlayInteractionMode::Passthrough => {
                    (state.ime == Some(ui)) as usize + (state.focused == Some(ui)) as usize
                }
                OverlayInteractionMode::InteractiveHud => 0,
                OverlayInteractionMode::Modal => 1,
            };
            let mutations = releases
                .checked_add(3)
                .and_then(|count| count.checked_add(mode_tail))
                .ok_or(IntegrationError::Composition(
                    CompositionError::RevisionExhausted,
                ))?;
            if composition
                .revision()
                .checked_add(u64::try_from(mutations).map_err(|_| {
                    IntegrationError::Composition(CompositionError::RevisionExhausted)
                })?)
                .is_none()
            {
                return Err(IntegrationError::Composition(
                    CompositionError::RevisionExhausted,
                ));
            }
        }
        let previous_game = self.game;
        let previous_ui = self.ui;
        self.game = game;
        self.ui = ui;
        let mut revision = match self.set_mode(composition, mode) {
            Ok(revision) => revision,
            Err(error) => {
                self.game = previous_game;
                self.ui = previous_ui;
                return Err(error);
            }
        };
        if restore_ui_text_focus {
            revision = match self.focus_ui_text(composition) {
                Ok(revision) => revision,
                Err(error) => {
                    self.game = previous_game;
                    self.ui = previous_ui;
                    return Err(error);
                }
            };
        }
        Ok(revision)
    }

    pub fn set_mode(
        &mut self,
        composition: &mut CompositionRegistry,
        mode: OverlayInteractionMode,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        let game_descriptor = composition.surface_descriptor(self.game)?;
        let ui_descriptor = composition.surface_descriptor(self.ui)?;
        if game_descriptor.view != self.view || ui_descriptor.view != self.view {
            return Err(IntegrationError::WrongView);
        }
        if game_descriptor.kind != SurfaceKind::Game || ui_descriptor.kind != SurfaceKind::Ui {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        if mode == OverlayInteractionMode::Passthrough
            && !matches!(
                game_descriptor.input,
                SurfaceInputPolicy::Interactive | SurfaceInputPolicy::Exclusive
            )
        {
            return Err(IntegrationError::Composition(
                CompositionError::NotInteractive,
            ));
        }
        let input_state = composition.view_input_state(self.view)?;
        let captures = composition.view_pointer_captures(self.view)?;
        let released_captures = captures
            .iter()
            .filter(|(_, owner)| mode == OverlayInteractionMode::Modal || *owner == self.ui)
            .count();
        let trailing_mutations = match mode {
            OverlayInteractionMode::Passthrough => {
                (input_state.ime == Some(self.ui)) as usize
                    + (input_state.focused == Some(self.ui)) as usize
            }
            OverlayInteractionMode::InteractiveHud => 0,
            OverlayInteractionMode::Modal => 1,
        };
        let mutation_count = released_captures
            .checked_add(1)
            .and_then(|count| count.checked_add(trailing_mutations))
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        composition
            .revision()
            .checked_add(
                u64::try_from(mutation_count).map_err(|_| {
                    IntegrationError::Composition(CompositionError::RevisionExhausted)
                })?,
            )
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        let mut revision = composition.revision();
        if matches!(
            mode,
            OverlayInteractionMode::Passthrough | OverlayInteractionMode::Modal
        ) {
            for (pointer, owner) in captures {
                if mode == OverlayInteractionMode::Modal || owner == self.ui {
                    revision = composition.release_pointer_for(pointer, owner, revision)?;
                }
            }
        }
        let input = match mode {
            OverlayInteractionMode::Passthrough => SurfaceInputPolicy::Passthrough,
            OverlayInteractionMode::InteractiveHud => SurfaceInputPolicy::Interactive,
            OverlayInteractionMode::Modal => SurfaceInputPolicy::Exclusive,
        };
        revision = composition.update_surface_input_policy(self.ui, input, revision)?;
        match mode {
            OverlayInteractionMode::Passthrough => {
                let state = composition.view_input_state(self.view)?;
                if state.ime == Some(self.ui) {
                    revision = composition.end_ime(self.ui, revision)?;
                }
                if state.focused == Some(self.ui) {
                    revision = composition.set_focus(self.view, Some(self.game), revision)?;
                }
            }
            OverlayInteractionMode::InteractiveHud => {}
            OverlayInteractionMode::Modal => {
                revision = composition.set_focus(self.view, Some(self.ui), revision)?;
            }
        }
        self.mode = mode;
        Ok(revision)
    }

    pub fn focus_ui_text(
        &self,
        composition: &mut CompositionRegistry,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        let descriptor = composition.surface_descriptor(self.ui)?;
        if descriptor.view != self.view {
            return Err(IntegrationError::WrongView);
        }
        if descriptor.kind != SurfaceKind::Ui {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        if !descriptor.accepts_text {
            return Err(IntegrationError::Composition(
                CompositionError::TextUnsupported,
            ));
        }
        if !matches!(
            descriptor.input,
            SurfaceInputPolicy::Interactive | SurfaceInputPolicy::Exclusive
        ) {
            return Err(IntegrationError::Composition(
                CompositionError::NotInteractive,
            ));
        }
        composition
            .revision()
            .checked_add(2)
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        let revision = composition.set_focus(self.view, Some(self.ui), composition.revision())?;
        Ok(composition.begin_ime(self.ui, revision)?)
    }

    pub fn release_ui_text(
        &self,
        composition: &mut CompositionRegistry,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        let state = composition.view_input_state(self.view)?;
        let game = composition.surface_descriptor(self.game)?;
        let ui = composition.surface_descriptor(self.ui)?;
        if game.view != self.view || ui.view != self.view {
            return Err(IntegrationError::WrongView);
        }
        if game.kind != SurfaceKind::Game || ui.kind != SurfaceKind::Ui {
            return Err(IntegrationError::WrongSurfaceKind);
        }
        if state.focused == Some(self.ui)
            && !matches!(
                game.input,
                SurfaceInputPolicy::Interactive | SurfaceInputPolicy::Exclusive
            )
        {
            return Err(IntegrationError::Composition(
                CompositionError::NotInteractive,
            ));
        }
        let mutation_count =
            (state.ime == Some(self.ui)) as u64 + (state.focused == Some(self.ui)) as u64;
        composition
            .revision()
            .checked_add(mutation_count)
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        let mut revision = composition.revision();
        if state.ime == Some(self.ui) {
            revision = composition.end_ime(self.ui, revision)?;
        }
        if state.focused == Some(self.ui) {
            revision = composition.set_focus(self.view, Some(self.game), revision)?;
        }
        Ok(revision)
    }

    pub fn pointer(
        &self,
        composition: &mut CompositionRegistry,
        x_milli: i32,
        y_milli: i32,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        let hits = composition.hit_test_stack(self.view, x_milli, y_milli)?;
        Ok(composition.arbitrate(self.view, ArbitrationEvent::PointerStack { hits })?)
    }

    pub fn pointer_for(
        &self,
        composition: &mut CompositionRegistry,
        pointer: CompositionPointerId,
        x_milli: i32,
        y_milli: i32,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        let hits = composition.hit_test_stack(self.view, x_milli, y_milli)?;
        Ok(composition.arbitrate(
            self.view,
            ArbitrationEvent::PointerStackFor { pointer, hits },
        )?)
    }

    pub fn pointer_with_vogui_hit(
        &self,
        composition: &mut CompositionRegistry,
        pointer: CompositionPointerId,
        x_milli: i32,
        y_milli: i32,
        vogui_hit: bool,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        let mut hits = composition.hit_test_stack(self.view, x_milli, y_milli)?;
        if !vogui_hit && self.mode != OverlayInteractionMode::Modal {
            hits.retain(|surface| *surface != self.ui);
        }
        Ok(composition.arbitrate(
            self.view,
            ArbitrationEvent::PointerStackFor { pointer, hits },
        )?)
    }

    pub fn capture_ui_pointer(
        &self,
        composition: &mut CompositionRegistry,
        pointer: CompositionPointerId,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        Ok(composition.capture_pointer_for(pointer, self.ui, composition.revision())?)
    }

    pub fn release_ui_pointer(
        &self,
        composition: &mut CompositionRegistry,
        pointer: CompositionPointerId,
    ) -> Result<u64, IntegrationError> {
        self.ensure_open()?;
        Ok(composition.release_pointer_for(pointer, self.ui, composition.revision())?)
    }

    pub fn keyboard(
        &self,
        composition: &mut CompositionRegistry,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        Ok(composition.arbitrate(self.view, ArbitrationEvent::Keyboard)?)
    }

    pub fn text(
        &self,
        composition: &mut CompositionRegistry,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        Ok(composition.arbitrate(self.view, ArbitrationEvent::Text)?)
    }

    pub fn shortcut(
        &self,
        composition: &mut CompositionRegistry,
        system_class_mask: Option<u64>,
    ) -> Result<ArbitrationResult, IntegrationError> {
        self.ensure_open()?;
        let event = system_class_mask.map_or(ArbitrationEvent::Shortcut, |class_mask| {
            ArbitrationEvent::SystemShortcut { class_mask }
        });
        Ok(composition.arbitrate(self.view, event)?)
    }

    pub fn shutdown(
        &mut self,
        composition: &mut CompositionRegistry,
    ) -> Result<SharedViewInputShutdownReport, IntegrationError> {
        let before = self.owner_snapshot();
        if self.closed {
            return Ok(SharedViewInputShutdownReport {
                before,
                released_pointer_captures: 0,
                final_composition_revision: composition.revision(),
                after: before,
            });
        }
        let state = composition.view_input_state(self.view)?;
        let captures = composition.view_pointer_captures(self.view)?;
        let ui_captures = captures
            .iter()
            .filter(|(_, owner)| *owner == self.ui)
            .map(|(pointer, _)| *pointer)
            .collect::<Vec<_>>();
        let mutation_count = ui_captures
            .len()
            .checked_add((state.ime == Some(self.ui)) as usize)
            .and_then(|count| count.checked_add((state.focused == Some(self.ui)) as usize))
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        composition
            .revision()
            .checked_add(
                u64::try_from(mutation_count).map_err(|_| {
                    IntegrationError::Composition(CompositionError::RevisionExhausted)
                })?,
            )
            .ok_or(IntegrationError::Composition(
                CompositionError::RevisionExhausted,
            ))?;
        let mut revision = composition.revision();
        for pointer in &ui_captures {
            revision = composition.release_pointer_for(*pointer, self.ui, revision)?;
        }
        if state.ime == Some(self.ui) {
            revision = composition.end_ime(self.ui, revision)?;
        }
        if state.focused == Some(self.ui) {
            revision = composition.set_focus(self.view, Some(self.game), revision)?;
        }
        self.closed = true;
        Ok(SharedViewInputShutdownReport {
            before,
            released_pointer_captures: ui_captures.len(),
            final_composition_revision: revision,
            after: self.owner_snapshot(),
        })
    }

    fn ensure_open(&self) -> Result<(), IntegrationError> {
        if self.closed {
            Err(IntegrationError::Closed)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vo_app_protocol::GenerationalHandle;
    use vo_app_runtime::{CompositionLimits, SurfaceDescriptor};

    fn handle(index: u32) -> GenerationalHandle {
        GenerationalHandle {
            index,
            generation: 1,
        }
    }

    fn policy(z_order: i32, input: SurfaceInputPolicy) -> CompositionLayerPolicy {
        CompositionLayerPolicy {
            z_order,
            input,
            geometry: SurfaceGeometry::default(),
        }
    }

    #[test]
    fn native_frame_orders_independent_ui_and_game_layers_and_rejects_mixed_devices() {
        let view = handle(1);
        let game = handle(2);
        let ui = handle(3);
        let mut builder = NativeCompositionBuilder::new(view, 7, 4, 800_000, 600_000, 2).unwrap();
        builder
            .push_vogui(
                ui,
                NativePaintSubmission {
                    texture_token: 20,
                    fence_value: 21,
                    device_generation: 4,
                    content_revision: 22,
                },
                policy(10, SurfaceInputPolicy::Passthrough),
            )
            .unwrap();
        assert_eq!(
            builder.push_voplay(
                game,
                NativeRenderSubmission {
                    fence_value: 30,
                    texture_token: 31,
                    device_generation: 5,
                    content_revision: 32,
                },
                policy(0, SurfaceInputPolicy::Interactive),
            ),
            Err(IntegrationError::DeviceGenerationMismatch)
        );
        builder
            .push_voplay(
                game,
                NativeRenderSubmission {
                    fence_value: 30,
                    texture_token: 31,
                    device_generation: 4,
                    content_revision: 32,
                },
                policy(0, SurfaceInputPolicy::Interactive),
            )
            .unwrap();

        let frame = builder.finish().unwrap();
        assert_eq!(
            frame
                .layers
                .iter()
                .map(|layer| (layer.surface, layer.kind))
                .collect::<Vec<_>>(),
            vec![(game, SurfaceKind::Game), (ui, SurfaceKind::Ui)]
        );
    }

    fn composition() -> (
        CompositionRegistry,
        ViewHandle,
        SurfaceHandle,
        SurfaceHandle,
    ) {
        let mut composition =
            CompositionRegistry::new(handle(1), 1, CompositionLimits::default()).unwrap();
        let window = composition.create_window().unwrap();
        let view = composition.create_view(window).unwrap();
        let game = composition
            .attach_surface(SurfaceDescriptor {
                view,
                kind: SurfaceKind::Game,
                z_order: 0,
                input: SurfaceInputPolicy::Interactive,
                accepts_text: false,
                geometry: SurfaceGeometry::default(),
            })
            .unwrap();
        let ui = composition
            .attach_surface(SurfaceDescriptor {
                view,
                kind: SurfaceKind::Ui,
                z_order: 10,
                input: SurfaceInputPolicy::Passthrough,
                accepts_text: true,
                geometry: SurfaceGeometry::default(),
            })
            .unwrap();
        (composition, view, game, ui)
    }

    #[test]
    fn overlay_mode_arbitrates_passthrough_modal_focus_and_ime() {
        let (mut composition, view, game, ui) = composition();
        let mut controller = SharedViewInputController::new(&composition, view, game, ui).unwrap();

        let passthrough = controller.pointer(&mut composition, 10, 10).unwrap();
        assert_eq!(passthrough.consumed_by, Some(game));
        assert_eq!(
            passthrough
                .deliveries
                .iter()
                .map(|delivery| delivery.surface)
                .collect::<Vec<_>>(),
            vec![ui, game]
        );

        controller
            .set_mode(&mut composition, OverlayInteractionMode::Modal)
            .unwrap();
        let modal = controller.pointer(&mut composition, 10, 10).unwrap();
        assert_eq!(modal.consumed_by, Some(ui));
        assert_eq!(
            composition.view_input_state(view).unwrap().focused,
            Some(ui)
        );

        controller.focus_ui_text(&mut composition).unwrap();
        assert_eq!(composition.view_input_state(view).unwrap().ime, Some(ui));
        assert_eq!(
            controller.keyboard(&mut composition).unwrap().consumed_by,
            Some(ui)
        );
        assert_eq!(
            controller.text(&mut composition).unwrap().consumed_by,
            Some(ui)
        );

        controller.release_ui_text(&mut composition).unwrap();
        let state = composition.view_input_state(view).unwrap();
        assert_eq!(state.focused, Some(game));
        assert_eq!(state.ime, None);
    }
}
