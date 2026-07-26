use crate::{
    decode_packet, encode_packet, DecodeError, EncodeError, Handle, MessageKind, PacketHeader,
};

const SURFACE_CONTROL_PAYLOAD_BYTES: usize = 78;
const FRAME_PREFIX_BYTES: usize = 92;
const FRAME_OUTCOME_PAYLOAD_BYTES: usize = 89;
const PLATFORM_INPUT_PREFIX_BYTES: usize = 80;
pub const PLATFORM_INPUT_FLAG_CONSUMED_BY_UI: u16 = 1 << 15;
pub const PLATFORM_INPUT_MODIFIER_MASK: u16 = !PLATFORM_INPUT_FLAG_CONSUMED_BY_UI;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SurfaceControlAction {
    Attach = 1,
    Resize = 2,
    Suspend = 3,
    Resume = 4,
    Detach = 5,
    Rebind = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceMetricsWire {
    pub width: u32,
    pub height: u32,
    pub scale_numerator: u32,
    pub scale_denominator: u32,
}

impl SurfaceMetricsWire {
    pub const fn is_valid(self) -> bool {
        self.width != 0
            && self.height != 0
            && self.scale_numerator != 0
            && self.scale_denominator != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceControlWire {
    pub action: SurfaceControlAction,
    pub session: Handle,
    pub window: Handle,
    pub view: Handle,
    pub surface: Handle,
    pub domain: Handle,
    pub metrics: SurfaceMetricsWire,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub z_order: i32,
    pub input_policy: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameSubmissionWire<'a> {
    pub session: Handle,
    pub window: Handle,
    pub view: Handle,
    pub surface: Handle,
    pub domain: Handle,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub pulse_id: u64,
    pub frame_id: u64,
    pub deadline_micros: u64,
    pub graph_signature: u64,
    pub commands: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameOutcomeKind {
    Presented = 1,
    DeadlineMissed = 2,
    Suspended = 3,
    SurfaceLost = 4,
    DeviceLost = 5,
    RejectedBeforeSubmit = 6,
    OutcomeUnknown = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameOutcomeWire {
    pub outcome: FrameOutcomeKind,
    pub session: Handle,
    pub surface: Handle,
    pub domain: Handle,
    pub render_endpoint: Handle,
    pub device_generation: u64,
    pub pulse_id: u64,
    pub frame_id: u64,
    pub rendered_revision: u64,
    pub observed_control_revision: u64,
    pub fence_value: u64,
    pub completion_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformInputWire<'a> {
    pub kind: u16,
    pub flags: u16,
    pub consumed_by_ui: bool,
    pub session: Handle,
    pub window: Handle,
    pub view: Handle,
    pub surface: Handle,
    pub domain: Handle,
    pub timestamp_micros: u64,
    pub event_sequence: u64,
    pub device: Handle,
    pub code: u32,
    pub value: i32,
    pub payload: &'a [u8],
}

pub fn encode_platform_input(
    header: PacketHeader,
    input: PlatformInputWire<'_>,
) -> Result<Vec<u8>, WebLaneCodecError> {
    if header.kind != MessageKind::PlatformInput
        || !(1..=16).contains(&input.kind)
        || input.flags & !PLATFORM_INPUT_MODIFIER_MASK != 0
        || !input.session.is_valid()
        || !input.window.is_valid()
        || !input.view.is_valid()
        || !input.surface.is_valid()
        || !input.domain.is_valid()
        || input.timestamp_micros == 0
        || input.event_sequence == 0
        || !input.device.is_valid()
    {
        return Err(WebLaneCodecError::InvalidIdentity);
    }
    let payload_len =
        u32::try_from(input.payload.len()).map_err(|_| WebLaneCodecError::LengthOverflow)?;
    let mut payload = Vec::with_capacity(
        PLATFORM_INPUT_PREFIX_BYTES
            .checked_add(input.payload.len())
            .ok_or(WebLaneCodecError::LengthOverflow)?,
    );
    payload.push(1);
    payload.push(input.kind as u8);
    let flags = input.flags
        | if input.consumed_by_ui {
            PLATFORM_INPUT_FLAG_CONSUMED_BY_UI
        } else {
            0
        };
    payload.extend_from_slice(&flags.to_le_bytes());
    put_handle(&mut payload, input.session);
    put_handle(&mut payload, input.window);
    put_handle(&mut payload, input.view);
    put_handle(&mut payload, input.surface);
    put_handle(&mut payload, input.domain);
    put_u64(&mut payload, input.timestamp_micros);
    put_u64(&mut payload, input.event_sequence);
    put_handle(&mut payload, input.device);
    put_u32(&mut payload, input.code);
    payload.extend_from_slice(&input.value.to_le_bytes());
    put_u32(&mut payload, payload_len);
    debug_assert_eq!(payload.len(), PLATFORM_INPUT_PREFIX_BYTES);
    payload.extend_from_slice(input.payload);
    encode_packet(header, &payload).map_err(WebLaneCodecError::PacketEncode)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebLaneCodecError {
    PacketDecode(DecodeError),
    PacketEncode(EncodeError),
    UnexpectedKind,
    InvalidIdentity,
    InvalidPayload,
    LengthOverflow,
}

pub fn encode_surface_control(
    header: PacketHeader,
    control: SurfaceControlWire,
) -> Result<Vec<u8>, WebLaneCodecError> {
    if header.kind != MessageKind::SurfaceControl || !valid_surface_control(control) {
        return Err(WebLaneCodecError::InvalidIdentity);
    }
    let mut payload = Vec::with_capacity(SURFACE_CONTROL_PAYLOAD_BYTES);
    payload.push(control.action as u8);
    put_handle(&mut payload, control.session);
    put_handle(&mut payload, control.window);
    put_handle(&mut payload, control.view);
    put_handle(&mut payload, control.surface);
    put_handle(&mut payload, control.domain);
    put_u32(&mut payload, control.metrics.width);
    put_u32(&mut payload, control.metrics.height);
    put_u32(&mut payload, control.metrics.scale_numerator);
    put_u32(&mut payload, control.metrics.scale_denominator);
    put_handle(&mut payload, control.render_endpoint);
    put_u64(&mut payload, control.device_generation);
    payload.extend_from_slice(&control.z_order.to_le_bytes());
    payload.push(control.input_policy);
    debug_assert_eq!(payload.len(), SURFACE_CONTROL_PAYLOAD_BYTES);
    encode_packet(header, &payload).map_err(WebLaneCodecError::PacketEncode)
}

pub fn encode_frame_submission(
    header: PacketHeader,
    frame: FrameSubmissionWire<'_>,
) -> Result<Vec<u8>, WebLaneCodecError> {
    if header.kind != MessageKind::FramePulse
        || !frame.session.is_valid()
        || !frame.window.is_valid()
        || !frame.view.is_valid()
        || !frame.surface.is_valid()
        || !frame.domain.is_valid()
        || !frame.render_endpoint.is_valid()
        || frame.device_generation == 0
        || frame.pulse_id == 0
        || frame.frame_id == 0
        || frame.graph_signature == 0
    {
        return Err(WebLaneCodecError::InvalidIdentity);
    }
    let command_len =
        u32::try_from(frame.commands.len()).map_err(|_| WebLaneCodecError::LengthOverflow)?;
    let mut payload = Vec::with_capacity(
        FRAME_PREFIX_BYTES
            .checked_add(frame.commands.len())
            .ok_or(WebLaneCodecError::LengthOverflow)?,
    );
    put_handle(&mut payload, frame.session);
    put_handle(&mut payload, frame.window);
    put_handle(&mut payload, frame.view);
    put_handle(&mut payload, frame.surface);
    put_handle(&mut payload, frame.domain);
    put_handle(&mut payload, frame.render_endpoint);
    put_u64(&mut payload, frame.device_generation);
    put_u64(&mut payload, frame.pulse_id);
    put_u64(&mut payload, frame.frame_id);
    put_u64(&mut payload, frame.deadline_micros);
    put_u64(&mut payload, frame.graph_signature);
    put_u32(&mut payload, command_len);
    debug_assert_eq!(payload.len(), FRAME_PREFIX_BYTES);
    payload.extend_from_slice(frame.commands);
    encode_packet(header, &payload).map_err(WebLaneCodecError::PacketEncode)
}

pub fn decode_frame_outcome(
    bytes: &[u8],
) -> Result<(PacketHeader, FrameOutcomeWire), WebLaneCodecError> {
    let (header, payload) = decode_packet(bytes).map_err(WebLaneCodecError::PacketDecode)?;
    if header.kind != MessageKind::DeviceEvent {
        return Err(WebLaneCodecError::UnexpectedKind);
    }
    if payload.len() != FRAME_OUTCOME_PAYLOAD_BYTES {
        return Err(WebLaneCodecError::InvalidPayload);
    }
    let mut reader = Reader::new(payload);
    let outcome = match reader.u8()? {
        1 => FrameOutcomeKind::Presented,
        2 => FrameOutcomeKind::DeadlineMissed,
        3 => FrameOutcomeKind::Suspended,
        4 => FrameOutcomeKind::SurfaceLost,
        5 => FrameOutcomeKind::DeviceLost,
        6 => FrameOutcomeKind::RejectedBeforeSubmit,
        7 => FrameOutcomeKind::OutcomeUnknown,
        _ => return Err(WebLaneCodecError::InvalidPayload),
    };
    let value = FrameOutcomeWire {
        outcome,
        session: reader.handle()?,
        surface: reader.handle()?,
        domain: reader.handle()?,
        render_endpoint: reader.handle()?,
        device_generation: reader.nonzero_u64()?,
        pulse_id: reader.nonzero_u64()?,
        frame_id: reader.nonzero_u64()?,
        rendered_revision: reader.u64()?,
        observed_control_revision: reader.u64()?,
        fence_value: reader.u64()?,
        completion_micros: reader.u64()?,
    };
    reader.finish()?;
    Ok((header, value))
}

pub fn decode_platform_input(
    bytes: &[u8],
) -> Result<(PacketHeader, PlatformInputWire<'_>), WebLaneCodecError> {
    let (header, payload) = decode_packet(bytes).map_err(WebLaneCodecError::PacketDecode)?;
    if header.kind != MessageKind::PlatformInput {
        return Err(WebLaneCodecError::UnexpectedKind);
    }
    if payload.len() < PLATFORM_INPUT_PREFIX_BYTES {
        return Err(WebLaneCodecError::InvalidPayload);
    }
    let mut reader = Reader::new(payload);
    if reader.u8()? != 1 {
        return Err(WebLaneCodecError::InvalidPayload);
    }
    let kind = u16::from(reader.u8()?);
    if !(1..=16).contains(&kind) {
        return Err(WebLaneCodecError::InvalidPayload);
    }
    let wire_flags = reader.u16()?;
    let value = PlatformInputWire {
        kind,
        flags: wire_flags & PLATFORM_INPUT_MODIFIER_MASK,
        consumed_by_ui: wire_flags & PLATFORM_INPUT_FLAG_CONSUMED_BY_UI != 0,
        session: reader.handle()?,
        window: reader.handle()?,
        view: reader.handle()?,
        surface: reader.handle()?,
        domain: reader.handle()?,
        timestamp_micros: reader.u64()?,
        event_sequence: reader.nonzero_u64()?,
        device: reader.handle()?,
        code: reader.u32()?,
        value: reader.i32()?,
        payload: reader.bytes32()?,
    };
    reader.finish()?;
    Ok((header, value))
}

fn valid_surface_control(control: SurfaceControlWire) -> bool {
    control.session.is_valid()
        && control.window.is_valid()
        && control.view.is_valid()
        && control.surface.is_valid()
        && control.domain.is_valid()
        && control.metrics.is_valid()
        && control.render_endpoint.is_valid()
        && control.device_generation != 0
        && control.input_policy != 0
}

fn put_handle(output: &mut Vec<u8>, handle: Handle) {
    put_u32(output, handle.index);
    put_u32(output, handle.generation);
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], WebLaneCodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(WebLaneCodecError::InvalidPayload)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(WebLaneCodecError::InvalidPayload)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, WebLaneCodecError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, WebLaneCodecError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| WebLaneCodecError::InvalidPayload)?,
        ))
    }

    fn u16(&mut self) -> Result<u16, WebLaneCodecError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| WebLaneCodecError::InvalidPayload)?,
        ))
    }

    fn i32(&mut self) -> Result<i32, WebLaneCodecError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| WebLaneCodecError::InvalidPayload)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, WebLaneCodecError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| WebLaneCodecError::InvalidPayload)?,
        ))
    }

    fn nonzero_u64(&mut self) -> Result<u64, WebLaneCodecError> {
        let value = self.u64()?;
        if value == 0 {
            return Err(WebLaneCodecError::InvalidPayload);
        }
        Ok(value)
    }

    fn handle(&mut self) -> Result<Handle, WebLaneCodecError> {
        let value = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !value.is_valid() {
            return Err(WebLaneCodecError::InvalidIdentity);
        }
        Ok(value)
    }

    fn bytes32(&mut self) -> Result<&'a [u8], WebLaneCodecError> {
        let length = usize::try_from(self.u32()?).map_err(|_| WebLaneCodecError::InvalidPayload)?;
        self.take(length)
    }

    fn finish(self) -> Result<(), WebLaneCodecError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(WebLaneCodecError::InvalidPayload)
        }
    }
}
