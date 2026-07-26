use voplay_protocol::{EngineId, Handle, PacketHeader};

use crate::{
    control::{ControlKind, StableControlRef},
    render_readback::{
        ReadbackFormat, ReadbackOutcome, ReadbackRegion, ReadbackRequest, ReadbackRequestId,
        ReadbackResult, ReadbackTarget,
    },
};

const REQUEST_MAGIC: &[u8; 4] = b"VRQ1";
const RESULT_MAGIC: &[u8; 4] = b"VRC1";
const REQUEST_BYTES: usize = 68;
const CANCEL_BYTES: usize = 16;
const RESULT_PREFIX_BYTES: usize = 44;
pub const MAX_WIRE_READBACK_BYTES: usize = 60 * 1024 * 1024;
pub const MAX_WIRE_READBACK_DIAGNOSTIC_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackWireCommand {
    Submit(ReadbackRequest),
    Cancel(ReadbackRequestId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackWireError {
    InvalidHeader,
    InvalidRequest,
    Capacity,
    Truncated,
    TrailingBytes,
}

pub fn encode_readback_request(request: ReadbackRequest) -> Result<Vec<u8>, ReadbackWireError> {
    validate_request(request)?;
    let mut bytes = Vec::with_capacity(REQUEST_BYTES);
    bytes.extend_from_slice(REQUEST_MAGIC);
    bytes.push(1);
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&request.id.0.to_le_bytes());
    put_handle(&mut bytes, request.target.target);
    bytes.extend_from_slice(&request.expected_target_revision.to_le_bytes());
    put_handle(&mut bytes, request.endpoint_generation);
    bytes.extend_from_slice(&request.region.x.to_le_bytes());
    bytes.extend_from_slice(&request.region.y.to_le_bytes());
    bytes.extend_from_slice(&request.region.width.to_le_bytes());
    bytes.extend_from_slice(&request.region.height.to_le_bytes());
    bytes.push(format_tag(request.format));
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&request.deadline_millis.to_le_bytes());
    debug_assert_eq!(bytes.len(), REQUEST_BYTES);
    Ok(bytes)
}

pub fn encode_readback_cancel(id: ReadbackRequestId) -> Result<Vec<u8>, ReadbackWireError> {
    if id.0 == 0 {
        return Err(ReadbackWireError::InvalidRequest);
    }
    let mut bytes = Vec::with_capacity(CANCEL_BYTES);
    bytes.extend_from_slice(REQUEST_MAGIC);
    bytes.push(2);
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&id.0.to_le_bytes());
    Ok(bytes)
}

pub fn decode_readback_request(
    header: PacketHeader,
    payload: &[u8],
    endpoint_generation: Handle,
) -> Result<ReadbackWireCommand, ReadbackWireError> {
    if !header.engine.is_valid()
        || header.commit_id == 0
        || header.required_control_revision == 0
        || !endpoint_generation.is_valid()
        || payload.get(..4) != Some(REQUEST_MAGIC)
        || payload.get(5..8) != Some(&[0; 3])
    {
        return Err(ReadbackWireError::InvalidHeader);
    }
    match payload.get(4).copied() {
        Some(1) if payload.len() == REQUEST_BYTES => {
            let request = ReadbackRequest {
                id: ReadbackRequestId(read_u64(payload, 8)?),
                target: ReadbackTarget {
                    engine: header.engine,
                    target: read_handle(payload, 16)?,
                },
                expected_target_revision: read_u64(payload, 24)?,
                required_control_revision: header.required_control_revision,
                endpoint_generation: read_handle(payload, 32)?,
                region: ReadbackRegion {
                    x: read_u32(payload, 40)?,
                    y: read_u32(payload, 44)?,
                    width: read_u32(payload, 48)?,
                    height: read_u32(payload, 52)?,
                },
                format: decode_format(payload[56])?,
                deadline_millis: read_u64(payload, 60)?,
            };
            if request.id.0 != header.commit_id
                || request.expected_target_revision != header.base_revision
                || request.endpoint_generation != endpoint_generation
            {
                return Err(ReadbackWireError::InvalidRequest);
            }
            validate_request(request)?;
            Ok(ReadbackWireCommand::Submit(request))
        }
        Some(2) if payload.len() == CANCEL_BYTES => {
            let id = ReadbackRequestId(read_u64(payload, 8)?);
            if id.0 == 0 || id.0 != header.commit_id {
                return Err(ReadbackWireError::InvalidRequest);
            }
            Ok(ReadbackWireCommand::Cancel(id))
        }
        _ if payload.len() < CANCEL_BYTES => Err(ReadbackWireError::Truncated),
        _ => Err(ReadbackWireError::TrailingBytes),
    }
}

pub fn encode_readback_result(result: &ReadbackResult) -> Result<Vec<u8>, ReadbackWireError> {
    if result.id.0 == 0
        || !result.target.engine.is_valid()
        || !result.target.target.is_valid()
        || result.bytes.len() > MAX_WIRE_READBACK_BYTES
        || result.diagnostic.len() > MAX_WIRE_READBACK_DIAGNOSTIC_BYTES
        || result.lease.is_some()
        || (result.outcome == ReadbackOutcome::Completed
            && (result.target_revision == 0
                || result.row_bytes == 0
                || result.bytes.is_empty()
                || !result.diagnostic.is_empty()))
        || (result.outcome != ReadbackOutcome::Completed && !result.bytes.is_empty())
    {
        return Err(ReadbackWireError::InvalidRequest);
    }
    let total = RESULT_PREFIX_BYTES
        .checked_add(result.bytes.len())
        .and_then(|bytes| bytes.checked_add(result.diagnostic.len()))
        .ok_or(ReadbackWireError::Capacity)?;
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(RESULT_MAGIC);
    bytes.push(outcome_tag(result.outcome));
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&result.id.0.to_le_bytes());
    put_handle(&mut bytes, result.target.target);
    bytes.extend_from_slice(&result.target_revision.to_le_bytes());
    bytes.extend_from_slice(&result.row_bytes.to_le_bytes());
    bytes.extend_from_slice(&(result.bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(result.diagnostic.len() as u32).to_le_bytes());
    debug_assert_eq!(bytes.len(), RESULT_PREFIX_BYTES);
    bytes.extend_from_slice(&result.bytes);
    bytes.extend_from_slice(&result.diagnostic);
    Ok(bytes)
}

pub fn decode_readback_result(
    payload: &[u8],
    engine: EngineId,
) -> Result<ReadbackResult, ReadbackWireError> {
    if payload.len() < RESULT_PREFIX_BYTES
        || payload.get(..4) != Some(RESULT_MAGIC)
        || payload.get(5..8) != Some(&[0; 3])
        || !engine.is_valid()
    {
        return Err(ReadbackWireError::Truncated);
    }
    let bytes_len = read_u32(payload, 36)? as usize;
    let diagnostic_len = read_u32(payload, 40)? as usize;
    if bytes_len > MAX_WIRE_READBACK_BYTES || diagnostic_len > MAX_WIRE_READBACK_DIAGNOSTIC_BYTES {
        return Err(ReadbackWireError::Capacity);
    }
    let bytes_end = RESULT_PREFIX_BYTES
        .checked_add(bytes_len)
        .ok_or(ReadbackWireError::Capacity)?;
    let end = bytes_end
        .checked_add(diagnostic_len)
        .ok_or(ReadbackWireError::Capacity)?;
    if end != payload.len() {
        return Err(if end > payload.len() {
            ReadbackWireError::Truncated
        } else {
            ReadbackWireError::TrailingBytes
        });
    }
    let result = ReadbackResult {
        id: ReadbackRequestId(read_u64(payload, 8)?),
        target: ReadbackTarget {
            engine,
            target: read_handle(payload, 16)?,
        },
        outcome: decode_outcome(payload[4])?,
        target_revision: read_u64(payload, 24)?,
        row_bytes: read_u32(payload, 32)?,
        bytes: payload[RESULT_PREFIX_BYTES..bytes_end].to_vec(),
        lease: None,
        diagnostic: payload[bytes_end..end].to_vec(),
    };
    encode_readback_result(&result)?;
    Ok(result)
}

pub fn stable_readback_target(request: ReadbackRequest) -> StableControlRef {
    StableControlRef {
        engine: request.target.engine,
        kind: ControlKind::RenderTarget,
        handle: request.target.target,
    }
}

fn validate_request(request: ReadbackRequest) -> Result<(), ReadbackWireError> {
    if request.id.0 == 0
        || !request.target.engine.is_valid()
        || !request.target.target.is_valid()
        || request.expected_target_revision == 0
        || request.required_control_revision == 0
        || !request.endpoint_generation.is_valid()
        || request.region.width == 0
        || request.region.height == 0
        || request.deadline_millis == 0
    {
        return Err(ReadbackWireError::InvalidRequest);
    }
    Ok(())
}

const fn format_tag(format: ReadbackFormat) -> u8 {
    match format {
        ReadbackFormat::Rgba8 => 1,
        ReadbackFormat::Bgra8 => 2,
        ReadbackFormat::Rgba16Float => 3,
        ReadbackFormat::Depth32Float => 4,
    }
}

fn decode_format(tag: u8) -> Result<ReadbackFormat, ReadbackWireError> {
    match tag {
        1 => Ok(ReadbackFormat::Rgba8),
        2 => Ok(ReadbackFormat::Bgra8),
        3 => Ok(ReadbackFormat::Rgba16Float),
        4 => Ok(ReadbackFormat::Depth32Float),
        _ => Err(ReadbackWireError::InvalidRequest),
    }
}

const fn outcome_tag(outcome: ReadbackOutcome) -> u8 {
    match outcome {
        ReadbackOutcome::Completed => 1,
        ReadbackOutcome::CancelledBeforeDispatch => 2,
        ReadbackOutcome::DeadlineExceeded => 3,
        ReadbackOutcome::TargetRevisionChanged => 4,
        ReadbackOutcome::EndpointRestartedBeforeDispatch => 5,
        ReadbackOutcome::OutcomeUnknownOnEndpointRestart => 6,
        ReadbackOutcome::Failed => 7,
        ReadbackOutcome::ShutdownBeforeDispatch => 8,
        ReadbackOutcome::OutcomeUnknownOnShutdown => 9,
    }
}

fn decode_outcome(tag: u8) -> Result<ReadbackOutcome, ReadbackWireError> {
    match tag {
        1 => Ok(ReadbackOutcome::Completed),
        2 => Ok(ReadbackOutcome::CancelledBeforeDispatch),
        3 => Ok(ReadbackOutcome::DeadlineExceeded),
        4 => Ok(ReadbackOutcome::TargetRevisionChanged),
        5 => Ok(ReadbackOutcome::EndpointRestartedBeforeDispatch),
        6 => Ok(ReadbackOutcome::OutcomeUnknownOnEndpointRestart),
        7 => Ok(ReadbackOutcome::Failed),
        8 => Ok(ReadbackOutcome::ShutdownBeforeDispatch),
        9 => Ok(ReadbackOutcome::OutcomeUnknownOnShutdown),
        _ => Err(ReadbackWireError::InvalidRequest),
    }
}

fn put_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn read_handle(bytes: &[u8], offset: usize) -> Result<Handle, ReadbackWireError> {
    Ok(Handle {
        index: read_u32(bytes, offset)?,
        generation: read_u32(bytes, offset + 4)?,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ReadbackWireError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(ReadbackWireError::Truncated)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| ReadbackWireError::Truncated)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ReadbackWireError> {
    bytes
        .get(offset..offset + 8)
        .ok_or(ReadbackWireError::Truncated)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| ReadbackWireError::Truncated)
}
