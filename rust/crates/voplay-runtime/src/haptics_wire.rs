use voplay_protocol::{
    decode_packet, encode_packet, DecodeError, EncodeError, EngineId, Handle, MessageKind,
    PacketHeader,
};

use crate::haptics::{HapticsCommand, HapticsOutcome};

pub const START_PAYLOAD_BYTES: usize = 41;
pub const CANCEL_PAYLOAD_BYTES: usize = 17;
pub const RESULT_PAYLOAD_BYTES: usize = 17;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HapticsResultWire {
    pub request_id: u64,
    pub device: Handle,
    pub outcome: HapticsOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HapticsWireError {
    Decode(DecodeError),
    Encode(EncodeError),
    WrongKind,
    WrongEngine,
    InvalidHeader,
    InvalidPayload,
}

impl From<DecodeError> for HapticsWireError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl From<EncodeError> for HapticsWireError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

pub fn encode_haptics_command(
    engine: EngineId,
    channel_epoch: u64,
    sequence: u64,
    source_simulation_revision: u64,
    command: HapticsCommand,
) -> Result<Vec<u8>, HapticsWireError> {
    if !engine.is_valid() || channel_epoch == 0 || sequence == 0 {
        return Err(HapticsWireError::InvalidHeader);
    }
    let (request_id, payload) = match command {
        HapticsCommand::Start(request) => {
            if request.scope.engine != engine
                || request.request_id == 0
                || !request.scope.handle.is_valid()
                || !request.device.is_valid()
                || request.duration_millis == 0
                || (request.strong_magnitude == 0 && request.weak_magnitude == 0)
                || request.deadline_millis == 0
            {
                return Err(HapticsWireError::InvalidPayload);
            }
            let mut payload = Vec::with_capacity(START_PAYLOAD_BYTES);
            payload.push(1);
            payload.extend_from_slice(&request.request_id.to_le_bytes());
            push_handle(&mut payload, request.scope.handle);
            push_handle(&mut payload, request.device);
            payload.extend_from_slice(&request.duration_millis.to_le_bytes());
            payload.extend_from_slice(&request.strong_magnitude.to_le_bytes());
            payload.extend_from_slice(&request.weak_magnitude.to_le_bytes());
            payload.extend_from_slice(&request.deadline_millis.to_le_bytes());
            (request.request_id, payload)
        }
        HapticsCommand::Cancel { request_id, device } => {
            if request_id == 0 || !device.is_valid() {
                return Err(HapticsWireError::InvalidPayload);
            }
            let mut payload = Vec::with_capacity(CANCEL_PAYLOAD_BYTES);
            payload.push(2);
            payload.extend_from_slice(&request_id.to_le_bytes());
            push_handle(&mut payload, device);
            (request_id, payload)
        }
    };
    encode_packet(
        PacketHeader {
            kind: MessageKind::HapticsCommand,
            engine,
            channel_epoch,
            commit_id: request_id,
            base_revision: 0,
            new_revision: 0,
            required_control_revision: 0,
            source_simulation_revision,
            sequence,
            payload_len: 0,
        },
        &payload,
    )
    .map_err(Into::into)
}

pub fn decode_haptics_result(
    packet: &[u8],
    expected_engine: EngineId,
    expected_channel_epoch: u64,
) -> Result<(PacketHeader, HapticsResultWire), HapticsWireError> {
    let (header, payload) = decode_packet(packet)?;
    decode_haptics_result_parts(header, payload, expected_engine, expected_channel_epoch)
}

pub fn decode_haptics_result_parts(
    header: PacketHeader,
    payload: &[u8],
    expected_engine: EngineId,
    expected_channel_epoch: u64,
) -> Result<(PacketHeader, HapticsResultWire), HapticsWireError> {
    if header.kind != MessageKind::HapticsResult {
        return Err(HapticsWireError::WrongKind);
    }
    if header.engine != expected_engine {
        return Err(HapticsWireError::WrongEngine);
    }
    if header.channel_epoch != expected_channel_epoch
        || header.sequence == 0
        || header.commit_id == 0
        || header.base_revision != 0
        || header.new_revision != 0
        || header.required_control_revision != 0
    {
        return Err(HapticsWireError::InvalidHeader);
    }
    if payload.len() != RESULT_PAYLOAD_BYTES {
        return Err(HapticsWireError::InvalidPayload);
    }
    let request_id = read_u64(payload, 0)?;
    let device = read_handle(payload, 8)?;
    if request_id == 0 || request_id != header.commit_id || !device.is_valid() {
        return Err(HapticsWireError::InvalidPayload);
    }
    let outcome = match payload[16] {
        1 => HapticsOutcome::Succeeded,
        2 => HapticsOutcome::Unsupported,
        3 => HapticsOutcome::Cancelled,
        4 => HapticsOutcome::DeviceLost,
        5 => HapticsOutcome::Failed,
        _ => return Err(HapticsWireError::InvalidPayload),
    };
    Ok((
        header,
        HapticsResultWire {
            request_id,
            device,
            outcome,
        },
    ))
}

fn push_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn read_handle(bytes: &[u8], offset: usize) -> Result<Handle, HapticsWireError> {
    Ok(Handle {
        index: read_u32(bytes, offset)?,
        generation: read_u32(bytes, offset + 4)?,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, HapticsWireError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(HapticsWireError::InvalidPayload)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, HapticsWireError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(HapticsWireError::InvalidPayload)
}
