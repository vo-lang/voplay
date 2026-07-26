use std::collections::{BTreeMap, VecDeque};

use vo_app_protocol::{
    AudioDeviceFormat, AudioDeviceLeaseHandle, GenerationalHandle, SurfaceHandle, ViewHandle,
};
use vo_app_runtime::{
    AppSession, DisplayTimingRequest, DynamicInstanceGroupPlan, HostedAudioDevice,
    HostedInstanceGroup, PendingHostedInstanceGroup, PresentationDomainRoute,
    PresentationVisibility, ProviderGroupCloseReport, SurfaceDescriptor, SurfaceInputPolicy,
    SurfaceKind,
};
use voplay_protocol::{encode_packet, generated::MessageKind, EngineId, Handle, PacketHeader};

use crate::{
    app_runtime_bridge::{
        AppRuntimeVoplayInbound, AppRuntimeVoplayLane, AppRuntimeVoplayLaneError, AppSurfaceRoute,
    },
    audio::AudioGestureToken,
    audio_endpoint::{
        encode_audio_device_activation, encode_audio_device_lost, encode_audio_device_recovery,
        encode_audio_resume,
    },
    device_hub::{DeviceHub, DeviceRef},
    game_engine::{Game, GameEntryDescriptor, OwnedInitData, TargetIslandFactory},
    haptics::{HapticsBridge, HapticsCompletion, HapticsConfig, HapticsError, RumbleRequest},
    input::{
        ActionBinding, InputScopeId, TickInputFrame, PLATFORM_INPUT_GAMEPAD_CONNECT,
        PLATFORM_INPUT_GAMEPAD_DISCONNECT,
    },
    outbox::PresentationDomainId,
    presentation::FrameCommandEncoder,
    runtime::{
        run, RemoteOutboxFrame, RendererRecoveryReport, VoplayRuntime, VoplayRuntimeConfig,
        VoplayRuntimeError,
    },
    surface::{
        GameSurfaceId, PresentOutcome, RemoteFrameOutcome, RemoteFramePreparation, RenderBackend,
        RenderFrameAck, RenderFrameSubmission, SurfaceMetrics,
    },
    world::SimulationTick,
};
use voplay_protocol::web_lane::{FrameOutcomeKind, SurfaceControlAction};

const DEFAULT_FRAME_BUDGET_MICROS: u64 = 16_666;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostedVoplayError {
    Runtime(VoplayRuntimeError),
    Lane(AppRuntimeVoplayLaneError),
    Haptics(HapticsError),
    AppSession(String),
}

impl From<VoplayRuntimeError> for HostedVoplayError {
    fn from(error: VoplayRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<AppRuntimeVoplayLaneError> for HostedVoplayError {
    fn from(error: AppRuntimeVoplayLaneError) -> Self {
        Self::Lane(error)
    }
}

impl From<HapticsError> for HostedVoplayError {
    fn from(error: HapticsError) -> Self {
        Self::Haptics(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedWebPulse {
    pub synchronization: crate::presentation::RenderSyncOutcome,
    pub terminal: Option<PresentOutcome>,
    pub queued_frame: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedWebService {
    pub published_controls: Vec<(GameSurfaceId, SurfaceControlAction)>,
    pub published_frames: Vec<(GameSurfaceId, u64)>,
    pub completed_frames: Vec<(GameSurfaceId, u64, PresentOutcome)>,
    pub routed_inputs: Vec<crate::input::NormalizedInputEvent>,
    pub haptics_completions: Vec<HapticsCompletion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostedVoplayOwnerSnapshot {
    pub runtime: crate::runtime::VoplayRuntimeOwnerSnapshot,
    pub group_open: bool,
    pub app_surfaces: usize,
    pub web_surfaces: usize,
    pub pending_web_controls: usize,
    pub pending_web_frames: usize,
    pub lane: Option<crate::app_runtime_bridge::AppRuntimeVoplayLaneOwnerSnapshot>,
    pub haptics: crate::haptics::HapticsOwnerSnapshot,
    pub audio_device_active: bool,
}

#[derive(Clone, Copy, Debug)]
struct HostedWebSurface {
    app_route: AppSurfaceRoute,
    presentation_route: PresentationDomainRoute,
    metrics: SurfaceMetrics,
    device_generation: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingWebControl {
    surface: GameSurfaceId,
    action: SurfaceControlAction,
    route: AppSurfaceRoute,
    metrics: SurfaceMetrics,
    render_endpoint: Handle,
    device_generation: u64,
}

#[derive(Clone, Debug)]
struct PendingWebFrame {
    route: AppSurfaceRoute,
    frame: RenderFrameSubmission,
    deadline_micros: u64,
    source_simulation_revision: u64,
}

pub struct HostedVoplayRuntime<G, B, E>
where
    G: Game,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    runtime: VoplayRuntime<G, B, E>,
    group: Option<HostedInstanceGroup>,
    app_surfaces: BTreeMap<GameSurfaceId, SurfaceHandle>,
    web_surfaces: BTreeMap<GameSurfaceId, HostedWebSurface>,
    pending_web_controls: VecDeque<PendingWebControl>,
    pending_web_frames: BTreeMap<GameSurfaceId, PendingWebFrame>,
    lane: Option<AppRuntimeVoplayLane>,
    haptics: HapticsBridge,
    audio_device: Option<HostedAudioDevice>,
    next_audio_control_sequence: u64,
}

impl<G, B, E> HostedVoplayRuntime<G, B, E>
where
    G: Game,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    pub const fn runtime(&self) -> &VoplayRuntime<G, B, E> {
        &self.runtime
    }

    pub fn owner_snapshot(&self) -> HostedVoplayOwnerSnapshot {
        HostedVoplayOwnerSnapshot {
            runtime: self.runtime.owner_snapshot(),
            group_open: self.group.is_some(),
            app_surfaces: self.app_surfaces.len(),
            web_surfaces: self.web_surfaces.len(),
            pending_web_controls: self.pending_web_controls.len(),
            pending_web_frames: self.pending_web_frames.len(),
            lane: self.lane.as_ref().map(AppRuntimeVoplayLane::owner_snapshot),
            haptics: self.haptics.owner_snapshot(),
            audio_device_active: self.audio_device.is_some(),
        }
    }

    pub fn runtime_mut(&mut self) -> &mut VoplayRuntime<G, B, E> {
        &mut self.runtime
    }

    pub fn connect_haptics_device(&mut self, device: Handle) -> Result<(), HostedVoplayError> {
        self.haptics.connect_device(device)?;
        Ok(())
    }

    pub fn disconnect_haptics_device(
        &mut self,
        device: Handle,
    ) -> Result<usize, HostedVoplayError> {
        Ok(self.haptics.disconnect_device(device)?)
    }

    pub fn request_rumble(&mut self, request: RumbleRequest) -> Result<(), HostedVoplayError> {
        self.haptics.request(request)?;
        Ok(())
    }

    pub fn cancel_rumble(&mut self, request_id: u64) -> Result<(), HostedVoplayError> {
        self.haptics.cancel(request_id)?;
        Ok(())
    }

    pub fn configure_input(
        &mut self,
        revision: u64,
        bindings: Vec<ActionBinding>,
    ) -> Result<(), HostedVoplayError> {
        self.runtime.configure_input(revision, bindings)?;
        Ok(())
    }

    pub fn step_input_scope(
        &mut self,
        scope: InputScopeId,
        tick_id: u64,
        through_timestamp_micros: u64,
    ) -> Result<(TickInputFrame, SimulationTick), HostedVoplayError> {
        Ok(self
            .runtime
            .step_input_scope(scope, tick_id, through_timestamp_micros)?)
    }

    pub fn expire_rumble(&mut self, now_millis: u64) -> Result<usize, HostedVoplayError> {
        Ok(self.haptics.expire(now_millis)?)
    }

    pub fn activate_audio_device(
        &mut self,
        caller: vo_runtime::host_services_v2::CallerEndpointHandle,
        gesture: AudioGestureToken,
        format: AudioDeviceFormat,
    ) -> Result<HostedAudioDevice, HostedVoplayError> {
        if gesture.engine != self.runtime.game().id() {
            return Err(HostedVoplayError::AppSession(String::from(
                "audio gesture belongs to another Voplay engine",
            )));
        }
        if self.audio_device.is_some() {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay audio device is already active",
            )));
        }
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
        })?;
        let device = group
            .open_audio_device(format)
            .map_err(HostedVoplayError::AppSession)?;
        let payload = encode_audio_device_activation(gesture, device.permit);
        if let Err(error) = queue_audio_control_packet(
            group,
            caller,
            self.runtime.game().id(),
            &mut self.next_audio_control_sequence,
            MessageKind::DeviceEvent,
            &payload,
        ) {
            let _ = group.release_audio_device(device.lease);
            return Err(error);
        }
        self.audio_device = Some(device);
        Ok(device)
    }

    pub fn suspend_audio_device(
        &mut self,
        caller: vo_runtime::host_services_v2::CallerEndpointHandle,
    ) -> Result<(), HostedVoplayError> {
        let device = self.audio_device.ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay audio device is not active"))
        })?;
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
        })?;
        group
            .suspend_audio_device(device.lease)
            .map_err(HostedVoplayError::AppSession)?;
        queue_audio_control_packet(
            group,
            caller,
            self.runtime.game().id(),
            &mut self.next_audio_control_sequence,
            MessageKind::EngineSuspend,
            &[],
        )
    }

    pub fn resume_audio_device(
        &mut self,
        caller: vo_runtime::host_services_v2::CallerEndpointHandle,
    ) -> Result<HostedAudioDevice, HostedVoplayError> {
        let lease = self
            .audio_device
            .map(|device| device.lease)
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("Voplay audio device is not active"))
            })?;
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
        })?;
        let device = group
            .resume_audio_device(lease)
            .map_err(HostedVoplayError::AppSession)?;
        queue_audio_control_packet(
            group,
            caller,
            self.runtime.game().id(),
            &mut self.next_audio_control_sequence,
            MessageKind::EngineResume,
            &encode_audio_resume(device.permit),
        )?;
        self.audio_device = Some(device);
        Ok(device)
    }

    pub fn mark_audio_device_lost(
        &mut self,
        caller: vo_runtime::host_services_v2::CallerEndpointHandle,
    ) -> Result<(), HostedVoplayError> {
        let device = self.audio_device.ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay audio device is not active"))
        })?;
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
        })?;
        group
            .mark_audio_device_lost(device.lease)
            .map_err(HostedVoplayError::AppSession)?;
        queue_audio_control_packet(
            group,
            caller,
            self.runtime.game().id(),
            &mut self.next_audio_control_sequence,
            MessageKind::DeviceEvent,
            &encode_audio_device_lost(),
        )
    }

    pub fn recover_audio_device(
        &mut self,
        caller: vo_runtime::host_services_v2::CallerEndpointHandle,
        format: AudioDeviceFormat,
    ) -> Result<HostedAudioDevice, HostedVoplayError> {
        let lease = self
            .audio_device
            .map(|device| device.lease)
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("Voplay audio device is not active"))
            })?;
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
        })?;
        let device = group
            .recover_audio_device(lease, format)
            .map_err(HostedVoplayError::AppSession)?;
        queue_audio_control_packet(
            group,
            caller,
            self.runtime.game().id(),
            &mut self.next_audio_control_sequence,
            MessageKind::DeviceEvent,
            &encode_audio_device_recovery(device.permit),
        )?;
        self.audio_device = Some(device);
        Ok(device)
    }

    pub fn release_audio_device(
        &mut self,
    ) -> Result<Option<AudioDeviceLeaseHandle>, HostedVoplayError> {
        let Some(device) = self.audio_device.take() else {
            return Ok(None);
        };
        self.group
            .as_mut()
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("Voplay framework group is closed"))
            })?
            .release_audio_device(device.lease)
            .map_err(HostedVoplayError::AppSession)?;
        Ok(Some(device.lease))
    }

    pub fn attach_surface(
        &mut self,
        session: &AppSession,
        hub: &DeviceHub,
        view: ViewHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        z_order: i32,
        input: SurfaceInputPolicy,
    ) -> Result<GameSurfaceId, HostedVoplayError> {
        let surface = session
            .attach_host_surface(SurfaceDescriptor {
                view,
                kind: SurfaceKind::Game,
                z_order,
                input,
                accepts_text: false,
                geometry: vo_app_runtime::SurfaceGeometry {
                    bounds: Some(vo_app_runtime::SurfaceRect {
                        x_milli: 0,
                        y_milli: 0,
                        width_milli: metrics.width.saturating_mul(1_000),
                        height_milli: metrics.height.saturating_mul(1_000),
                    }),
                    ..vo_app_runtime::SurfaceGeometry::default()
                },
            })
            .map_err(HostedVoplayError::AppSession)?;
        match self.runtime.attach(hub, surface, domain, metrics) {
            Ok(game_surface) => {
                self.app_surfaces.insert(game_surface, surface);
                Ok(game_surface)
            }
            Err(error) => {
                session.close_host_surface(surface).map_err(|rollback| {
                    HostedVoplayError::AppSession(format!(
                        "Voplay Surface attach failed with {error:?}; App Surface rollback failed: {rollback}"
                    ))
                })?;
                Err(HostedVoplayError::Runtime(error))
            }
        }
    }

    pub fn attach_web_surface(
        &mut self,
        session: &AppSession,
        hub: &DeviceHub,
        view: ViewHandle,
        domain: PresentationDomainId,
        metrics: SurfaceMetrics,
        z_order: i32,
        input: SurfaceInputPolicy,
    ) -> Result<GameSurfaceId, HostedVoplayError> {
        let app_surface = session
            .attach_host_surface(SurfaceDescriptor {
                view,
                kind: SurfaceKind::Game,
                z_order,
                input,
                accepts_text: false,
                geometry: vo_app_runtime::SurfaceGeometry {
                    bounds: Some(vo_app_runtime::SurfaceRect {
                        x_milli: 0,
                        y_milli: 0,
                        width_milli: metrics.width.saturating_mul(1_000),
                        height_milli: metrics.height.saturating_mul(1_000),
                    }),
                    ..vo_app_runtime::SurfaceGeometry::default()
                },
            })
            .map_err(HostedVoplayError::AppSession)?;
        let game_surface = match self
            .runtime
            .attach_remote(hub, app_surface, domain, metrics)
        {
            Ok(surface) => surface,
            Err(error) => {
                session.close_host_surface(app_surface).map_err(|rollback| {
                    HostedVoplayError::AppSession(format!(
                        "Voplay remote Surface attach failed with {error:?}; App Surface rollback failed: {rollback}"
                    ))
                })?;
                return Err(HostedVoplayError::Runtime(error));
            }
        };
        let session_handle = session.host_session_handle().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay web Surface has no App Session"))
        })?;
        let window = session
            .host_view_window(view)
            .map_err(HostedVoplayError::AppSession)?;
        let route = AppSurfaceRoute {
            session: session_handle,
            window,
            view,
            surface: app_surface,
            z_order,
            input_policy: web_input_policy(input),
        };
        if self.lane.is_none() {
            self.lane = Some(AppRuntimeVoplayLane::open(
                session,
                self.runtime.presentation().engine().id(),
            )?);
        }
        let publish = self
            .lane
            .as_mut()
            .expect("Voplay framework lane was opened above")
            .publish_surface_control(
                session,
                SurfaceControlAction::Attach,
                route,
                domain.handle,
                metrics,
                self.runtime.render_endpoint(),
                self.runtime.device_generation(),
            );
        if let Err(error) = publish {
            let runtime_rollback = self.runtime.detach_remote(game_surface);
            let app_rollback = session.close_host_surface(app_surface);
            if runtime_rollback.is_err() || app_rollback.is_err() {
                return Err(HostedVoplayError::AppSession(format!(
                    "Voplay SurfaceControl admission failed with {error:?}; rollback runtime={runtime_rollback:?} app={app_rollback:?}"
                )));
            }
            return Err(HostedVoplayError::Lane(error));
        }
        let caller = session.host_caller().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from(
                "Voplay web Surface has no framework endpoint",
            ))
        })?;
        let presentation_route = PresentationDomainRoute {
            owner: caller,
            engine: app_runtime_handle(self.runtime.presentation().engine().id()),
            domain: app_runtime_handle(domain.handle),
            view,
            surface: app_surface,
            render_endpoint: app_runtime_handle(self.runtime.render_endpoint()),
            logic_endpoint: Some(app_runtime_handle(
                self.runtime.presentation().engine().id(),
            )),
            timing_source_revision: 1,
            metrics_revision: 1,
            frame_budget_micros: DEFAULT_FRAME_BUDGET_MICROS,
            visibility: PresentationVisibility::Visible,
        };
        if let Err(register) = session.register_host_presentation_domain(presentation_route) {
            let detach = self
                .lane
                .as_mut()
                .expect("Voplay framework lane remains open")
                .publish_surface_control(
                    session,
                    SurfaceControlAction::Detach,
                    route,
                    domain.handle,
                    metrics,
                    self.runtime.render_endpoint(),
                    self.runtime.device_generation(),
                );
            let runtime_rollback = self.runtime.detach_remote(game_surface);
            let app_rollback = session.close_host_surface(app_surface);
            return Err(HostedVoplayError::AppSession(format!(
                "PresentationDomain registration failed: {register}; renderer detach={detach:?} runtime rollback={runtime_rollback:?} app rollback={app_rollback:?}"
            )));
        }
        if let Err(request) = session.request_host_display_pulse(view) {
            let unregister = session.unregister_host_presentation_domain(
                app_runtime_handle(self.runtime.presentation().engine().id()),
                app_runtime_handle(domain.handle),
            );
            let detach = self
                .lane
                .as_mut()
                .expect("Voplay framework lane remains open")
                .publish_surface_control(
                    session,
                    SurfaceControlAction::Detach,
                    route,
                    domain.handle,
                    metrics,
                    self.runtime.render_endpoint(),
                    self.runtime.device_generation(),
                );
            let runtime_rollback = self.runtime.detach_remote(game_surface);
            let app_rollback = session.close_host_surface(app_surface);
            return Err(HostedVoplayError::AppSession(format!(
                "initial display pulse request failed: {request}; unregister={unregister:?} renderer detach={detach:?} runtime rollback={runtime_rollback:?} app rollback={app_rollback:?}"
            )));
        }
        self.app_surfaces.insert(game_surface, app_surface);
        self.web_surfaces.insert(
            game_surface,
            HostedWebSurface {
                app_route: route,
                presentation_route,
                metrics,
                device_generation: self.runtime.device_generation(),
            },
        );
        Ok(game_surface)
    }

    pub fn request_web_display_pulse(
        &self,
        session: &AppSession,
        surface: GameSurfaceId,
    ) -> Result<DisplayTimingRequest, HostedVoplayError> {
        let route = self.web_surfaces.get(&surface).ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
        })?;
        session
            .request_host_display_pulse(route.app_route.view)
            .map_err(HostedVoplayError::AppSession)
    }

    pub fn resize_web_surface(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), HostedVoplayError> {
        self.require_idle_web_surface(surface)?;
        let previous = *self.web_surfaces.get(&surface).ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
        })?;
        if previous.metrics == metrics {
            return Ok(());
        }
        let mut next = previous;
        next.metrics = metrics;
        next.presentation_route.metrics_revision =
            next_revision(previous.presentation_route.metrics_revision, "metrics")?;
        next.presentation_route.timing_source_revision = next_revision(
            previous.presentation_route.timing_source_revision,
            "timing source",
        )?;
        let app_surface = previous.app_route.surface;
        let old_geometry = session
            .host_surface_descriptor(app_surface)
            .map_err(HostedVoplayError::AppSession)?
            .geometry;
        let geometry_revision = session
            .host_composition_revision()
            .map_err(HostedVoplayError::AppSession)?;
        session
            .update_host_surface_geometry(app_surface, surface_geometry(metrics), geometry_revision)
            .map_err(HostedVoplayError::AppSession)?;
        if let Err(resize) = self.runtime.resize_remote_surface(surface, metrics) {
            let geometry_rollback = rollback_surface_geometry(session, app_surface, old_geometry);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay resize failed: {resize:?}; geometry rollback={geometry_rollback:?}"
            )));
        }
        if let Err(update) = session.update_host_presentation_domain(
            next.presentation_route,
            previous.presentation_route.timing_source_revision,
        ) {
            let rollback = self
                .runtime
                .resize_remote_surface(surface, previous.metrics);
            let geometry_rollback = rollback_surface_geometry(session, app_surface, old_geometry);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay resize route update failed: {update}; runtime rollback={rollback:?}; geometry rollback={geometry_rollback:?}"
            )));
        }
        if let Err(publish) =
            self.publish_web_control(session, surface, SurfaceControlAction::Resize, next)
        {
            let route_rollback = session.update_host_presentation_domain(
                previous.presentation_route,
                next.presentation_route.timing_source_revision,
            );
            let runtime_rollback = self
                .runtime
                .resize_remote_surface(surface, previous.metrics);
            let geometry_rollback = rollback_surface_geometry(session, app_surface, old_geometry);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay resize control admission failed: {publish:?}; route rollback={route_rollback:?} runtime rollback={runtime_rollback:?}; geometry rollback={geometry_rollback:?}"
            )));
        }
        self.web_surfaces.insert(surface, next);
        Ok(())
    }

    pub fn suspend_web_surface(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
    ) -> Result<(), HostedVoplayError> {
        self.require_idle_web_surface(surface)?;
        let previous = *self.web_surfaces.get(&surface).ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
        })?;
        let mut next = previous;
        next.presentation_route.timing_source_revision = next_revision(
            previous.presentation_route.timing_source_revision,
            "timing source",
        )?;
        next.presentation_route.visibility = PresentationVisibility::Suspended;
        self.runtime.suspend_remote_surface(surface)?;
        if let Err(update) = session.update_host_presentation_domain(
            next.presentation_route,
            previous.presentation_route.timing_source_revision,
        ) {
            let rollback = self.runtime.resume_remote_surface(surface);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay suspend route update failed: {update}; runtime rollback={rollback:?}"
            )));
        }
        if let Err(publish) =
            self.publish_web_control(session, surface, SurfaceControlAction::Suspend, next)
        {
            let route_rollback = session.update_host_presentation_domain(
                previous.presentation_route,
                next.presentation_route.timing_source_revision,
            );
            let runtime_rollback = self.runtime.resume_remote_surface(surface);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay suspend control admission failed: {publish:?}; route rollback={route_rollback:?} runtime rollback={runtime_rollback:?}"
            )));
        }
        self.web_surfaces.insert(surface, next);
        Ok(())
    }

    pub fn resume_web_surface(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
    ) -> Result<DisplayTimingRequest, HostedVoplayError> {
        self.require_idle_web_surface(surface)?;
        let previous = *self.web_surfaces.get(&surface).ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
        })?;
        let mut next = previous;
        next.presentation_route.timing_source_revision = next_revision(
            previous.presentation_route.timing_source_revision,
            "timing source",
        )?;
        next.presentation_route.visibility = PresentationVisibility::Visible;
        self.runtime.resume_remote_surface(surface)?;
        if let Err(update) = session.update_host_presentation_domain(
            next.presentation_route,
            previous.presentation_route.timing_source_revision,
        ) {
            let rollback = self.runtime.suspend_remote_surface(surface);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay resume route update failed: {update}; runtime rollback={rollback:?}"
            )));
        }
        let timing = match session.request_host_display_pulse(next.app_route.view) {
            Ok(request) => request,
            Err(request) => {
                let route_rollback = session.update_host_presentation_domain(
                    previous.presentation_route,
                    next.presentation_route.timing_source_revision,
                );
                let runtime_rollback = self.runtime.suspend_remote_surface(surface);
                return Err(HostedVoplayError::AppSession(format!(
                    "Voplay resume timing request failed: {request}; route rollback={route_rollback:?} runtime rollback={runtime_rollback:?}"
                )));
            }
        };
        if let Err(publish) =
            self.publish_web_control(session, surface, SurfaceControlAction::Resume, next)
        {
            let route_rollback = session.update_host_presentation_domain(
                previous.presentation_route,
                next.presentation_route.timing_source_revision,
            );
            let runtime_rollback = self.runtime.suspend_remote_surface(surface);
            return Err(HostedVoplayError::AppSession(format!(
                "Voplay resume control admission failed: {publish:?}; route rollback={route_rollback:?} runtime rollback={runtime_rollback:?}"
            )));
        }
        self.web_surfaces.insert(surface, next);
        Ok(timing)
    }

    pub fn recover_web_renderer(
        &mut self,
        session: &AppSession,
        hub: &mut DeviceHub,
    ) -> Result<RendererRecoveryReport, HostedVoplayError> {
        if !self.pending_web_controls.is_empty() || !self.pending_web_frames.is_empty() {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay web renderer recovery requires drained control and frame lanes",
            )));
        }
        let surfaces = self.web_surfaces.keys().copied().collect::<Vec<_>>();
        for surface in &surfaces {
            self.require_idle_web_surface(*surface)?;
        }
        let status = hub
            .status(self.runtime.device_lease())
            .map_err(VoplayRuntimeError::from)?;
        let mut updates = Vec::with_capacity(surfaces.len());
        for surface in &surfaces {
            let previous = *self
                .web_surfaces
                .get(surface)
                .expect("recovery Surface came from this map");
            let mut next = previous;
            next.presentation_route.timing_source_revision = next_revision(
                previous.presentation_route.timing_source_revision,
                "timing source",
            )?;
            next.presentation_route.render_endpoint = app_runtime_handle(status.render_endpoint);
            next.device_generation = status.device_generation;
            if let Err(update) = session.update_host_presentation_domain(
                next.presentation_route,
                previous.presentation_route.timing_source_revision,
            ) {
                let rollback = rollback_presentation_routes(session, &updates);
                return Err(HostedVoplayError::AppSession(format!(
                    "Voplay recovery route update failed: {update}; prior route rollback={rollback:?}"
                )));
            }
            updates.push((*surface, previous, next));
        }
        let report = match self.runtime.recover_renderer(hub) {
            Ok(report) => report,
            Err(error) => {
                let rollback = rollback_presentation_routes(session, &updates);
                if rollback.is_empty() {
                    return Err(HostedVoplayError::Runtime(error));
                }
                return Err(HostedVoplayError::AppSession(format!(
                    "Voplay renderer recovery failed: {error:?}; route rollback={rollback:?}"
                )));
            }
        };
        for (surface, _previous, next) in updates {
            self.web_surfaces.insert(surface, next);
            self.pending_web_controls.push_back(PendingWebControl {
                surface,
                action: SurfaceControlAction::Rebind,
                route: next.app_route,
                metrics: next.metrics,
                render_endpoint: self.runtime.render_endpoint(),
                device_generation: self.runtime.device_generation(),
            });
        }
        Ok(report)
    }

    pub fn begin_web_pulse(
        &mut self,
        session: &AppSession,
        hub: &DeviceHub,
        surface: GameSurfaceId,
        graph_signature: u64,
        input: Option<&crate::world::InputFrame>,
    ) -> Result<Option<HostedWebPulse>, HostedVoplayError> {
        if self
            .pending_web_controls
            .iter()
            .any(|control| control.surface == surface)
        {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay web Surface has a control update waiting for lane admission",
            )));
        }
        if self.pending_web_frames.contains_key(&surface) {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay web Surface already has a frame waiting for lane admission",
            )));
        }
        let route = self
            .web_surfaces
            .get(&surface)
            .map(|state| state.app_route)
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
            })?;
        let Some(display_pulse) = session
            .take_host_presentation_domain_pulse(
                app_runtime_handle(surface.engine),
                app_runtime_handle(surface.domain.handle),
            )
            .map_err(HostedVoplayError::AppSession)?
        else {
            return Ok(None);
        };
        if display_pulse.view != route.view
            || display_pulse.surface != route.surface
            || display_pulse.render_endpoint != app_runtime_handle(self.runtime.render_endpoint())
        {
            return Err(HostedVoplayError::AppSession(String::from(
                "App Runtime display pulse route does not match the Voplay Surface",
            )));
        }
        let metrics = self
            .web_surfaces
            .get(&surface)
            .map(|state| state.metrics)
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from(
                    "Voplay web Surface metrics are unavailable",
                ))
            })?;
        let pulse_bytes = encode_target_presentation_pulse(
            surface.domain,
            display_pulse.pulse_id,
            display_pulse.observed_micros,
            display_pulse.deadline_micros,
            display_pulse.coalesced_pulses,
            metrics,
        )?;
        let group = self.group.as_mut().ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay provider group is closed"))
        })?;
        for caller in group.voplay_target_callers() {
            group
                .enqueue_voplay_presentation_pulses(caller, vec![pulse_bytes.clone()])
                .map_err(HostedVoplayError::AppSession)?;
        }
        let RemoteOutboxFrame {
            synchronization,
            presentation,
            source_simulation_revision,
        } = self.runtime.pulse_remote_scheduled(
            hub,
            surface,
            display_pulse.pulse_id,
            display_pulse.observed_micros,
            display_pulse.deadline_micros,
            graph_signature,
            input,
        )?;
        match presentation {
            RemoteFramePreparation::Terminal(outcome) => {
                session
                    .request_host_display_pulse(route.view)
                    .map_err(HostedVoplayError::AppSession)?;
                Ok(Some(HostedWebPulse {
                    synchronization,
                    terminal: Some(outcome),
                    queued_frame: None,
                }))
            }
            RemoteFramePreparation::Submit(frame) => {
                let frame_id = frame.frame_id;
                self.pending_web_frames.insert(
                    surface,
                    PendingWebFrame {
                        route,
                        frame,
                        deadline_micros: display_pulse.deadline_micros,
                        source_simulation_revision,
                    },
                );
                let _ = self.publish_pending_web_frames(session)?;
                session
                    .request_host_display_pulse(route.view)
                    .map_err(HostedVoplayError::AppSession)?;
                Ok(Some(HostedWebPulse {
                    synchronization,
                    terminal: None,
                    queued_frame: Some(frame_id),
                }))
            }
        }
    }

    pub fn service_web_lane(
        &mut self,
        session: &AppSession,
    ) -> Result<HostedWebService, HostedVoplayError> {
        let published_controls = self.publish_pending_web_controls(session)?;
        let published_frames = self.publish_pending_web_frames(session)?;
        self.publish_pending_haptics(session)?;
        let mut completed_frames = Vec::new();
        let mut routed_inputs = Vec::new();
        for _ in 0..256 {
            let inbound = {
                let lane = self.lane.as_mut().ok_or_else(|| {
                    HostedVoplayError::AppSession(String::from("Voplay framework lane is not open"))
                })?;
                lane.poll_inbound(session)?
            };
            let Some(inbound) = inbound else {
                break;
            };
            let outcome = match inbound {
                AppRuntimeVoplayInbound::FrameOutcome(outcome) => outcome,
                AppRuntimeVoplayInbound::PlatformInput {
                    window,
                    view,
                    surface,
                    event,
                } => {
                    let route = self.web_surfaces.iter().find(|(id, state)| {
                        id.domain.handle == event.scope.handle
                            && app_handle(state.app_route.window) == window
                            && app_handle(state.app_route.view) == view
                            && app_handle(state.app_route.surface) == surface
                    });
                    if route.is_none() {
                        return Err(HostedVoplayError::AppSession(String::from(
                            "Voplay platform input targets an unknown Surface route",
                        )));
                    }
                    match event.kind {
                        PLATFORM_INPUT_GAMEPAD_CONNECT => {
                            self.haptics.connect_device(event.device)?;
                        }
                        PLATFORM_INPUT_GAMEPAD_DISCONNECT => {
                            match self.haptics.disconnect_device(event.device) {
                                Ok(_) | Err(HapticsError::UnknownDevice) => {}
                                Err(error) => return Err(HostedVoplayError::Haptics(error)),
                            }
                        }
                        _ => {}
                    }
                    self.runtime.route_input(event.clone())?;
                    let input_frame = event.wire_bytes();
                    let group = self.group.as_mut().ok_or_else(|| {
                        HostedVoplayError::AppSession(String::from(
                            "Voplay provider group is closed",
                        ))
                    })?;
                    for caller in group.voplay_target_callers() {
                        group
                            .enqueue_voplay_input_frames(caller, vec![input_frame.clone()])
                            .map_err(HostedVoplayError::AppSession)?;
                    }
                    routed_inputs.push(event);
                    continue;
                }
                AppRuntimeVoplayInbound::HapticsResult {
                    request_id,
                    device,
                    outcome,
                } => {
                    match self.haptics.complete(request_id, device, outcome) {
                        Ok(()) | Err(HapticsError::UnknownRequest) => {}
                        Err(error) => return Err(HostedVoplayError::Haptics(error)),
                    }
                    continue;
                }
            };
            let engine = self.runtime.presentation().engine().id();
            let surface = GameSurfaceId {
                engine,
                surface: app_surface_handle(outcome.surface),
                domain: PresentationDomainId {
                    engine,
                    handle: outcome.domain,
                },
            };
            if !self.web_surfaces.contains_key(&surface) {
                return Err(HostedVoplayError::AppSession(String::from(
                    "Voplay frame outcome targets an unknown Surface",
                )));
            }
            let ack = RenderFrameAck {
                surface,
                pulse_id: outcome.pulse_id,
                frame_id: outcome.frame_id,
                render_endpoint: outcome.render_endpoint,
                device_generation: outcome.device_generation,
                rendered_revision: outcome.rendered_revision,
                observed_control_revision: outcome.observed_control_revision,
            };
            let terminal = self
                .runtime
                .complete_remote_frame(ack, remote_outcome(outcome.outcome))?;
            completed_frames.push((surface, outcome.frame_id, terminal));
        }
        let mut haptics_completions = Vec::new();
        while let Some(completion) = self.haptics.poll_completion() {
            haptics_completions.push(completion);
        }
        Ok(HostedWebService {
            published_controls,
            published_frames,
            completed_frames,
            routed_inputs,
            haptics_completions,
        })
    }

    fn publish_pending_haptics(&mut self, session: &AppSession) -> Result<(), HostedVoplayError> {
        while let Some(command) = self.haptics.next_command() {
            let lane = self.lane.as_mut().ok_or_else(|| {
                HostedVoplayError::AppSession(String::from(
                    "Voplay haptics requires an open web framework lane",
                ))
            })?;
            lane.publish_haptics_command(
                session,
                self.runtime.game().simulation_revision(),
                command,
            )?;
            self.haptics.consume_command();
        }
        Ok(())
    }

    fn publish_web_control(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
        action: SurfaceControlAction,
        state: HostedWebSurface,
    ) -> Result<(), HostedVoplayError> {
        self.lane
            .as_mut()
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("Voplay framework lane is not open"))
            })?
            .publish_surface_control(
                session,
                action,
                state.app_route,
                surface.domain.handle,
                state.metrics,
                self.runtime.render_endpoint(),
                state.device_generation,
            )?;
        Ok(())
    }

    fn require_idle_web_surface(&self, surface: GameSurfaceId) -> Result<(), HostedVoplayError> {
        if !self.web_surfaces.contains_key(&surface) {
            return Err(HostedVoplayError::AppSession(String::from(
                "unknown Voplay web Surface",
            )));
        }
        if self
            .pending_web_controls
            .iter()
            .any(|control| control.surface == surface)
            || self.pending_web_frames.contains_key(&surface)
            || self.runtime.remote_frame_pending(surface)?
        {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay web Surface has pending control or frame work",
            )));
        }
        Ok(())
    }

    fn publish_pending_web_controls(
        &mut self,
        session: &AppSession,
    ) -> Result<Vec<(GameSurfaceId, SurfaceControlAction)>, HostedVoplayError> {
        let mut published = Vec::new();
        while let Some(control) = self.pending_web_controls.front().copied() {
            self.lane
                .as_mut()
                .ok_or_else(|| {
                    HostedVoplayError::AppSession(String::from("Voplay framework lane is not open"))
                })?
                .publish_surface_control(
                    session,
                    control.action,
                    control.route,
                    control.surface.domain.handle,
                    control.metrics,
                    control.render_endpoint,
                    control.device_generation,
                )?;
            self.pending_web_controls.pop_front();
            published.push((control.surface, control.action));
        }
        Ok(published)
    }

    fn publish_pending_web_frames(
        &mut self,
        session: &AppSession,
    ) -> Result<Vec<(GameSurfaceId, u64)>, HostedVoplayError> {
        if !self.pending_web_controls.is_empty() {
            return Err(HostedVoplayError::AppSession(String::from(
                "Voplay control updates must enter the framework lane before frames",
            )));
        }
        let surfaces = self.pending_web_frames.keys().copied().collect::<Vec<_>>();
        let mut published = Vec::new();
        for surface in surfaces {
            let pending = self
                .pending_web_frames
                .get(&surface)
                .expect("pending Voplay Surface came from this map");
            self.lane
                .as_mut()
                .ok_or_else(|| {
                    HostedVoplayError::AppSession(String::from("Voplay framework lane is not open"))
                })?
                .publish_frame(
                    session,
                    pending.route,
                    &pending.frame,
                    pending.deadline_micros,
                    pending.source_simulation_revision,
                )?;
            let frame_id = pending.frame.frame_id;
            self.pending_web_frames.remove(&surface);
            published.push((surface, frame_id));
        }
        Ok(published)
    }

    pub fn detach_surface(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
    ) -> Result<(), HostedVoplayError> {
        if self.web_surfaces.contains_key(&surface) {
            return Err(HostedVoplayError::AppSession(String::from(
                "web Surface requires detach_web_surface",
            )));
        }
        let app_surface =
            self.app_surfaces.get(&surface).copied().ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("unknown App Surface"))
            })?;
        self.runtime.detach(surface)?;
        session
            .close_host_surface(app_surface)
            .map_err(HostedVoplayError::AppSession)?;
        self.app_surfaces.remove(&surface);
        Ok(())
    }

    pub fn detach_web_surface(
        &mut self,
        session: &AppSession,
        surface: GameSurfaceId,
    ) -> Result<(), HostedVoplayError> {
        self.require_idle_web_surface(surface)?;
        let state = *self.web_surfaces.get(&surface).ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("unknown Voplay web Surface"))
        })?;
        let route = state.app_route;
        let app_surface = *self
            .app_surfaces
            .get(&surface)
            .ok_or_else(|| HostedVoplayError::AppSession(String::from("unknown App Surface")))?;
        self.lane
            .as_mut()
            .ok_or_else(|| {
                HostedVoplayError::AppSession(String::from("Voplay framework lane is not open"))
            })?
            .publish_surface_control(
                session,
                SurfaceControlAction::Detach,
                route,
                surface.domain.handle,
                state.metrics,
                self.runtime.render_endpoint(),
                self.runtime.device_generation(),
            )?;
        session
            .unregister_host_presentation_domain(
                app_runtime_handle(surface.engine),
                app_runtime_handle(surface.domain.handle),
            )
            .map_err(HostedVoplayError::AppSession)?;
        self.runtime.detach_remote(surface)?;
        session
            .close_host_surface(app_surface)
            .map_err(HostedVoplayError::AppSession)?;
        self.pending_web_frames.remove(&surface);
        self.web_surfaces.remove(&surface);
        self.app_surfaces.remove(&surface);
        Ok(())
    }

    pub fn shutdown(
        mut self,
        session: &AppSession,
        hub: &mut DeviceHub,
    ) -> Result<ProviderGroupCloseReport, HostedVoplayError> {
        let mut first_error = None;
        let haptics_shutdown = self.haptics.shutdown();
        for command in haptics_shutdown.cancel_commands {
            let publish_result = self
                .lane
                .as_mut()
                .ok_or_else(|| {
                    HostedVoplayError::AppSession(String::from(
                        "Voplay haptics shutdown requires an open framework lane",
                    ))
                })
                .and_then(|lane| {
                    lane.publish_haptics_command(
                        session,
                        self.runtime.game().simulation_revision(),
                        command,
                    )
                    .map_err(HostedVoplayError::Lane)
                });
            if let Err(error) = publish_result {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.release_audio_device() {
            first_error.get_or_insert(error);
        }
        self.pending_web_controls.clear();
        self.pending_web_frames.clear();
        for surface in self.web_surfaces.keys().copied().collect::<Vec<_>>() {
            if let Err(error) = self.detach_web_surface(session, surface) {
                first_error.get_or_insert(error);
            }
        }
        for surface in self.app_surfaces.keys().copied().collect::<Vec<_>>() {
            if let Err(error) = self.detach_surface(session, surface) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.runtime.shutdown(hub) {
            first_error.get_or_insert(HostedVoplayError::Runtime(error));
        }
        if let Some(lane) = &mut self.lane {
            lane.shutdown();
        }
        let close_result = self
            .group
            .take()
            .unwrap()
            .close()
            .map_err(HostedVoplayError::AppSession);
        if let Some(error) = first_error {
            return Err(error);
        }
        close_result
    }
}

fn surface_geometry(metrics: SurfaceMetrics) -> vo_app_runtime::SurfaceGeometry {
    vo_app_runtime::SurfaceGeometry {
        bounds: Some(vo_app_runtime::SurfaceRect {
            x_milli: 0,
            y_milli: 0,
            width_milli: metrics.width.saturating_mul(1_000),
            height_milli: metrics.height.saturating_mul(1_000),
        }),
        ..vo_app_runtime::SurfaceGeometry::default()
    }
}

fn rollback_surface_geometry(
    session: &AppSession,
    surface: SurfaceHandle,
    geometry: vo_app_runtime::SurfaceGeometry,
) -> Result<u64, String> {
    let revision = session.host_composition_revision()?;
    session.update_host_surface_geometry(surface, geometry, revision)
}

pub fn begin_attach(
    session: &AppSession,
    plan: DynamicInstanceGroupPlan,
) -> Result<PendingHostedInstanceGroup, HostedVoplayError> {
    session
        .begin_dynamic_instance_group(plan)
        .map_err(HostedVoplayError::AppSession)
}

pub fn attach<F, B, E>(
    group: HostedInstanceGroup,
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
) -> Result<HostedVoplayRuntime<F::Game, B, E>, HostedVoplayError>
where
    F: TargetIslandFactory,
    B: RenderBackend,
    E: FrameCommandEncoder,
{
    let runtime = run(
        id,
        descriptor,
        factory,
        init,
        hub,
        device,
        render_endpoint,
        backend,
        encoder,
        config,
    )?;
    let haptics = HapticsBridge::new(id, HapticsConfig::default())?;
    Ok(HostedVoplayRuntime {
        runtime,
        group: Some(group),
        app_surfaces: BTreeMap::new(),
        web_surfaces: BTreeMap::new(),
        pending_web_controls: VecDeque::new(),
        pending_web_frames: BTreeMap::new(),
        lane: None,
        haptics,
        audio_device: None,
        next_audio_control_sequence: 1,
    })
}

fn queue_audio_control_packet(
    group: &mut HostedInstanceGroup,
    caller: vo_runtime::host_services_v2::CallerEndpointHandle,
    engine: EngineId,
    next_sequence: &mut u64,
    kind: MessageKind,
    payload: &[u8],
) -> Result<(), HostedVoplayError> {
    let sequence = *next_sequence;
    let packet = encode_packet(
        PacketHeader {
            kind,
            engine,
            channel_epoch: 1,
            commit_id: 0,
            base_revision: 0,
            new_revision: 0,
            required_control_revision: 0,
            source_simulation_revision: 0,
            sequence,
            payload_len: 0,
        },
        payload,
    )
    .map_err(|error| {
        HostedVoplayError::AppSession(format!(
            "failed to encode Voplay audio lifecycle packet: {error:?}"
        ))
    })?;
    group
        .enqueue_voplay_audio_packets(caller, vec![packet])
        .map_err(HostedVoplayError::AppSession)?;
    *next_sequence = sequence.checked_add(1).ok_or_else(|| {
        HostedVoplayError::AppSession(String::from("Voplay audio control sequence exhausted"))
    })?;
    Ok(())
}

fn encode_target_presentation_pulse(
    domain: PresentationDomainId,
    pulse_id: u64,
    observed_micros: u64,
    deadline_micros: u64,
    coalesced_pulses: u64,
    metrics: SurfaceMetrics,
) -> Result<Vec<u8>, HostedVoplayError> {
    if !domain.handle.is_valid()
        || pulse_id == 0
        || deadline_micros < observed_micros
        || !metrics.is_valid()
    {
        return Err(HostedVoplayError::AppSession(String::from(
            "Voplay presentation pulse is invalid",
        )));
    }
    let observed_nanos = observed_micros.checked_mul(1_000).ok_or_else(|| {
        HostedVoplayError::AppSession(String::from("Voplay presentation pulse timestamp overflow"))
    })?;
    let deadline_nanos = deadline_micros.checked_mul(1_000).ok_or_else(|| {
        HostedVoplayError::AppSession(String::from("Voplay presentation pulse deadline overflow"))
    })?;
    let scale_milli = u64::from(metrics.scale_numerator)
        .checked_mul(1_000)
        .and_then(|value| value.checked_div(u64::from(metrics.scale_denominator)))
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            HostedVoplayError::AppSession(String::from("Voplay presentation scale is invalid"))
        })?;
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(b"VPUL1\0");
    bytes.extend_from_slice(&domain.handle.index.to_le_bytes());
    bytes.extend_from_slice(&domain.handle.generation.to_le_bytes());
    bytes.extend_from_slice(&pulse_id.to_le_bytes());
    bytes.extend_from_slice(&observed_nanos.to_le_bytes());
    bytes.extend_from_slice(&deadline_nanos.to_le_bytes());
    bytes.extend_from_slice(&coalesced_pulses.to_le_bytes());
    bytes.extend_from_slice(&metrics.width.to_le_bytes());
    bytes.extend_from_slice(&metrics.height.to_le_bytes());
    bytes.extend_from_slice(&scale_milli.to_le_bytes());
    Ok(bytes)
}

fn web_input_policy(input: SurfaceInputPolicy) -> u8 {
    match input {
        SurfaceInputPolicy::Exclusive | SurfaceInputPolicy::Interactive => 1,
        SurfaceInputPolicy::Observe => 2,
        SurfaceInputPolicy::Passthrough => 3,
    }
}

fn app_surface_handle(handle: Handle) -> SurfaceHandle {
    SurfaceHandle {
        index: handle.index,
        generation: handle.generation,
    }
}

fn app_runtime_handle(handle: Handle) -> GenerationalHandle {
    GenerationalHandle {
        index: handle.index,
        generation: handle.generation,
    }
}

fn remote_outcome(outcome: FrameOutcomeKind) -> RemoteFrameOutcome {
    match outcome {
        FrameOutcomeKind::Presented => RemoteFrameOutcome::Presented,
        FrameOutcomeKind::DeadlineMissed => RemoteFrameOutcome::DeadlineMissed,
        FrameOutcomeKind::Suspended => RemoteFrameOutcome::Suspended,
        FrameOutcomeKind::SurfaceLost => RemoteFrameOutcome::SurfaceLost,
        FrameOutcomeKind::DeviceLost => RemoteFrameOutcome::DeviceLost,
        FrameOutcomeKind::RejectedBeforeSubmit => RemoteFrameOutcome::RejectedBeforeSubmit,
        FrameOutcomeKind::OutcomeUnknown => RemoteFrameOutcome::OutcomeUnknown,
    }
}

fn app_handle(handle: impl Into<GenerationalHandle>) -> Handle {
    let handle = handle.into();
    Handle {
        index: handle.index,
        generation: handle.generation,
    }
}

fn next_revision(current: u64, label: &str) -> Result<u64, HostedVoplayError> {
    current
        .checked_add(1)
        .ok_or_else(|| HostedVoplayError::AppSession(format!("Voplay {label} revision exhausted")))
}

fn rollback_presentation_routes(
    session: &AppSession,
    updates: &[(GameSurfaceId, HostedWebSurface, HostedWebSurface)],
) -> Vec<String> {
    let mut failures = Vec::new();
    for (surface, previous, next) in updates.iter().rev() {
        if let Err(error) = session.update_host_presentation_domain(
            previous.presentation_route,
            next.presentation_route.timing_source_revision,
        ) {
            failures.push(format!("{surface:?}: {error}"));
        }
    }
    failures
}
