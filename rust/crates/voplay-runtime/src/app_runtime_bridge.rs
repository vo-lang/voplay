use vo_app_protocol::{SessionHandle, SurfaceHandle, ViewHandle, WindowHandle};
use vo_app_runtime::{
    decode_envelope, AppMessageKind, AppSession, EndpointChannelBinding, LaneLimits,
};
use voplay_protocol::{
    web_lane::{
        decode_frame_outcome, decode_platform_input, encode_frame_submission,
        encode_surface_control, FrameOutcomeWire, FrameSubmissionWire, SurfaceControlAction,
        SurfaceControlWire, SurfaceMetricsWire, WebLaneCodecError,
    },
    EngineId, Handle, MessageKind, PacketHeader,
};

use crate::haptics::{HapticsCommand, HapticsOutcome};
use crate::haptics_wire::{decode_haptics_result_parts, encode_haptics_command, HapticsWireError};
use crate::input::{InputScopeId, NormalizedInputEvent};
use crate::surface::{RenderFrameSubmission, SurfaceMetrics};

const FRAMEWORK_LANE_OWNER: &str = "voplay";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppSurfaceRoute {
    pub session: SessionHandle,
    pub window: WindowHandle,
    pub view: ViewHandle,
    pub surface: SurfaceHandle,
    pub z_order: i32,
    pub input_policy: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppRuntimeVoplayLaneError {
    AppSession(String),
    Codec(WebLaneCodecError),
    HapticsCodec(HapticsWireError),
    AppEnvelope,
    WrongEngine,
    WrongSession,
    WrongChannel,
    SequenceExhausted,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppRuntimeVoplayInbound {
    FrameOutcome(FrameOutcomeWire),
    PlatformInput {
        window: Handle,
        view: Handle,
        surface: Handle,
        event: NormalizedInputEvent,
    },
    HapticsResult {
        request_id: u64,
        device: Handle,
        outcome: HapticsOutcome,
    },
}

pub struct AppRuntimeVoplayLane {
    binding: EndpointChannelBinding,
    engine: EngineId,
    next_outbound_sequence: u64,
    next_inbound_sequence: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppRuntimeVoplayLaneOwnerSnapshot {
    pub binding: EndpointChannelBinding,
    pub engine: EngineId,
    pub next_outbound_sequence: u64,
    pub next_inbound_sequence: u64,
    pub closed: bool,
}

impl AppRuntimeVoplayLane {
    pub fn open(session: &AppSession, engine: EngineId) -> Result<Self, AppRuntimeVoplayLaneError> {
        if !engine.is_valid() {
            return Err(AppRuntimeVoplayLaneError::WrongEngine);
        }
        let binding = session
            .open_host_framework_channel_for(
                FRAMEWORK_LANE_OWNER,
                LaneLimits {
                    max_packet_bytes: vo_app_runtime::MAX_PACKET_BYTES as u32,
                    max_messages: 256,
                    max_bytes: 32 * 1024 * 1024,
                },
            )
            .map_err(AppRuntimeVoplayLaneError::AppSession)?;
        Ok(Self {
            binding,
            engine,
            next_outbound_sequence: 1,
            next_inbound_sequence: 1,
            closed: false,
        })
    }

    pub const fn binding(&self) -> EndpointChannelBinding {
        self.binding
    }

    pub const fn owner_snapshot(&self) -> AppRuntimeVoplayLaneOwnerSnapshot {
        AppRuntimeVoplayLaneOwnerSnapshot {
            binding: self.binding,
            engine: self.engine,
            next_outbound_sequence: self.next_outbound_sequence,
            next_inbound_sequence: self.next_inbound_sequence,
            closed: self.closed,
        }
    }

    pub fn shutdown(&mut self) -> AppRuntimeVoplayLaneOwnerSnapshot {
        self.closed = true;
        self.owner_snapshot()
    }

    pub fn publish_surface_control(
        &mut self,
        session: &AppSession,
        action: SurfaceControlAction,
        route: AppSurfaceRoute,
        domain: Handle,
        metrics: SurfaceMetrics,
        render_endpoint: Handle,
        device_generation: u64,
    ) -> Result<(), AppRuntimeVoplayLaneError> {
        self.validate_route(route)?;
        let packet = encode_surface_control(
            self.header(MessageKind::SurfaceControl, 0, 0, 0, 0, 0),
            SurfaceControlWire {
                action,
                session: app_handle(route.session),
                window: app_handle(route.window),
                view: app_handle(route.view),
                surface: app_handle(route.surface),
                domain,
                metrics: SurfaceMetricsWire {
                    width: metrics.width,
                    height: metrics.height,
                    scale_numerator: metrics.scale_numerator,
                    scale_denominator: metrics.scale_denominator,
                },
                render_endpoint,
                device_generation,
                z_order: route.z_order,
                input_policy: route.input_policy,
            },
        )
        .map_err(AppRuntimeVoplayLaneError::Codec)?;
        self.publish(session, &packet)
    }

    pub fn publish_frame(
        &mut self,
        session: &AppSession,
        route: AppSurfaceRoute,
        frame: &RenderFrameSubmission,
        deadline_micros: u64,
        source_simulation_revision: u64,
    ) -> Result<(), AppRuntimeVoplayLaneError> {
        self.validate_route(route)?;
        if frame.surface.engine != self.engine || frame.surface.surface != route.surface {
            return Err(AppRuntimeVoplayLaneError::WrongEngine);
        }
        let packet = encode_frame_submission(
            self.header(
                MessageKind::FramePulse,
                frame.frame_id,
                frame.required_render_revision,
                frame.required_render_revision,
                frame.required_control_revision,
                source_simulation_revision,
            ),
            FrameSubmissionWire {
                session: app_handle(route.session),
                window: app_handle(route.window),
                view: app_handle(route.view),
                surface: app_handle(route.surface),
                domain: frame.surface.domain.handle,
                render_endpoint: frame.render_endpoint,
                device_generation: frame.device_generation,
                pulse_id: frame.pulse_id,
                frame_id: frame.frame_id,
                deadline_micros,
                graph_signature: frame.graph_signature,
                commands: &frame.commands,
            },
        )
        .map_err(AppRuntimeVoplayLaneError::Codec)?;
        self.publish(session, &packet)
    }

    pub fn publish_haptics_command(
        &mut self,
        session: &AppSession,
        source_simulation_revision: u64,
        command: HapticsCommand,
    ) -> Result<(), AppRuntimeVoplayLaneError> {
        let packet = encode_haptics_command(
            self.engine,
            self.binding.channel_epoch,
            self.next_outbound_sequence,
            source_simulation_revision,
            command,
        )
        .map_err(AppRuntimeVoplayLaneError::HapticsCodec)?;
        self.publish(session, &packet)
    }

    pub fn poll_inbound(
        &mut self,
        session: &AppSession,
    ) -> Result<Option<AppRuntimeVoplayInbound>, AppRuntimeVoplayLaneError> {
        self.ensure_open()?;
        let Some(packet) = session
            .take_inbound_host_framework_packet_for(FRAMEWORK_LANE_OWNER)
            .map_err(AppRuntimeVoplayLaneError::AppSession)?
        else {
            return Ok(None);
        };
        let (envelope, payload) =
            decode_envelope(&packet.bytes).map_err(|_| AppRuntimeVoplayLaneError::AppEnvelope)?;
        if envelope.message_kind != AppMessageKind::FrameworkPayload
            || envelope.channel != self.binding.channel
            || envelope.channel_epoch != self.binding.channel_epoch
        {
            return Err(AppRuntimeVoplayLaneError::WrongChannel);
        }
        let (packet_header, framework_payload) =
            voplay_protocol::decode_packet(payload).map_err(|error| {
                AppRuntimeVoplayLaneError::Codec(WebLaneCodecError::PacketDecode(error))
            })?;
        if packet_header.engine != self.engine
            || packet_header.channel_epoch != self.binding.channel_epoch
            || packet_header.sequence != self.next_inbound_sequence
        {
            return Err(AppRuntimeVoplayLaneError::WrongEngine);
        }
        let inbound = match packet_header.kind {
            MessageKind::DeviceEvent => {
                let (header, outcome) =
                    decode_frame_outcome(payload).map_err(AppRuntimeVoplayLaneError::Codec)?;
                if header.commit_id != outcome.frame_id
                    || header.base_revision != outcome.rendered_revision
                    || header.new_revision != outcome.rendered_revision
                    || header.required_control_revision != outcome.observed_control_revision
                    || outcome.session != app_handle(self.binding.session)
                {
                    return Err(AppRuntimeVoplayLaneError::WrongSession);
                }
                AppRuntimeVoplayInbound::FrameOutcome(outcome)
            }
            MessageKind::PlatformInput => {
                let (_, input) =
                    decode_platform_input(payload).map_err(AppRuntimeVoplayLaneError::Codec)?;
                if input.session != app_handle(self.binding.session) {
                    return Err(AppRuntimeVoplayLaneError::WrongSession);
                }
                AppRuntimeVoplayInbound::PlatformInput {
                    window: input.window,
                    view: input.view,
                    surface: input.surface,
                    event: NormalizedInputEvent {
                        sequence: input.event_sequence,
                        timestamp_micros: input.timestamp_micros,
                        scope: InputScopeId {
                            engine: self.engine,
                            handle: input.domain,
                        },
                        device: input.device,
                        kind: input.kind,
                        code: input.code,
                        value: input.value,
                        modifiers: input.flags,
                        consumed_by_ui: input.consumed_by_ui,
                        payload: input.payload.to_vec(),
                    },
                }
            }
            MessageKind::HapticsResult => {
                let (_, result) = decode_haptics_result_parts(
                    packet_header,
                    framework_payload,
                    self.engine,
                    self.binding.channel_epoch,
                )
                .map_err(AppRuntimeVoplayLaneError::HapticsCodec)?;
                AppRuntimeVoplayInbound::HapticsResult {
                    request_id: result.request_id,
                    device: result.device,
                    outcome: result.outcome,
                }
            }
            _ => {
                return Err(AppRuntimeVoplayLaneError::Codec(
                    WebLaneCodecError::UnexpectedKind,
                ))
            }
        };
        self.next_inbound_sequence = self
            .next_inbound_sequence
            .checked_add(1)
            .ok_or(AppRuntimeVoplayLaneError::SequenceExhausted)?;
        Ok(Some(inbound))
    }

    fn publish(
        &mut self,
        session: &AppSession,
        packet: &[u8],
    ) -> Result<(), AppRuntimeVoplayLaneError> {
        self.ensure_open()?;
        let next = self
            .next_outbound_sequence
            .checked_add(1)
            .ok_or(AppRuntimeVoplayLaneError::SequenceExhausted)?;
        session
            .publish_host_framework_payload_for(FRAMEWORK_LANE_OWNER, packet)
            .map_err(AppRuntimeVoplayLaneError::AppSession)?;
        self.next_outbound_sequence = next;
        Ok(())
    }

    fn header(
        &self,
        kind: MessageKind,
        commit_id: u64,
        base_revision: u64,
        new_revision: u64,
        required_control_revision: u64,
        source_simulation_revision: u64,
    ) -> PacketHeader {
        PacketHeader {
            kind,
            engine: self.engine,
            channel_epoch: self.binding.channel_epoch,
            commit_id,
            base_revision,
            new_revision,
            required_control_revision,
            source_simulation_revision,
            sequence: self.next_outbound_sequence,
            payload_len: 0,
        }
    }

    fn validate_route(&self, route: AppSurfaceRoute) -> Result<(), AppRuntimeVoplayLaneError> {
        self.ensure_open()?;
        if route.session != self.binding.session
            || !route.window.is_valid()
            || !route.view.is_valid()
            || !route.surface.is_valid()
            || route.input_policy == 0
        {
            return Err(AppRuntimeVoplayLaneError::WrongSession);
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), AppRuntimeVoplayLaneError> {
        if self.closed {
            Err(AppRuntimeVoplayLaneError::Closed)
        } else {
            Ok(())
        }
    }
}

fn app_handle(handle: impl Into<vo_app_protocol::GenerationalHandle>) -> Handle {
    let handle = handle.into();
    Handle {
        index: handle.index,
        generation: handle.generation,
    }
}
