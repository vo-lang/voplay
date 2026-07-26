use crate::endpoint::OwnedEndpointPacket;
use crate::fault_injection::{
    FaultInjectionError, FaultInjectionMetrics, FaultInjector, FaultPoint, FaultRule,
    FaultTraceEvent, InjectedFault,
};
use voplay_protocol::{generated::MessageKind, EngineId, PacketHeader};

const COMMAND_MAGIC: &[u8; 4] = b"VFI1";
const RESPONSE_MAGIC: &[u8; 4] = b"VFI2";
const VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultWireCommand {
    Install(FaultRule),
    Remove(FaultPoint),
    Clear,
    Metrics,
    Trace { after_sequence: u64, limit: u32 },
    ClearTrace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultWireError {
    Malformed,
    Unsupported,
    Injection(FaultInjectionError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultWireResponse {
    pub action: u16,
    pub affected: u64,
    pub metrics: FaultInjectionMetrics,
    pub trace: Vec<FaultTraceEvent>,
}

pub fn is_fault_wire_command(bytes: &[u8]) -> bool {
    bytes.starts_with(COMMAND_MAGIC)
}

pub fn decode_fault_wire_command(bytes: &[u8]) -> Result<FaultWireCommand, FaultWireError> {
    if bytes.len() < 8 || &bytes[..4] != COMMAND_MAGIC || read_u16(bytes, 4) != VERSION {
        return Err(FaultWireError::Malformed);
    }
    match read_u16(bytes, 6) {
        1 if bytes.len() == 40 => Ok(FaultWireCommand::Install(FaultRule {
            point: decode_point(bytes[8])?,
            fault: decode_fault(bytes[9])?,
            skip: read_u64(bytes, 16),
            every: read_u64(bytes, 24),
            remaining: read_u64(bytes, 32),
        })),
        2 if bytes.len() == 16 => Ok(FaultWireCommand::Remove(decode_point(bytes[8])?)),
        3 if bytes.len() == 8 => Ok(FaultWireCommand::Clear),
        4 if bytes.len() == 8 => Ok(FaultWireCommand::Metrics),
        5 if bytes.len() == 24 && read_u32(bytes, 16) > 0 && read_u32(bytes, 16) <= 4096 => {
            Ok(FaultWireCommand::Trace {
                after_sequence: read_u64(bytes, 8),
                limit: read_u32(bytes, 16),
            })
        }
        6 if bytes.len() == 8 => Ok(FaultWireCommand::ClearTrace),
        _ => Err(FaultWireError::Unsupported),
    }
}

pub fn encode_fault_wire_command(command: FaultWireCommand) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend_from_slice(COMMAND_MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    match command {
        FaultWireCommand::Install(rule) => {
            bytes.extend_from_slice(&1_u16.to_le_bytes());
            bytes.push(encode_point(rule.point));
            bytes.push(encode_fault(rule.fault));
            bytes.extend_from_slice(&[0; 6]);
            bytes.extend_from_slice(&rule.skip.to_le_bytes());
            bytes.extend_from_slice(&rule.every.to_le_bytes());
            bytes.extend_from_slice(&rule.remaining.to_le_bytes());
        }
        FaultWireCommand::Remove(point) => {
            bytes.extend_from_slice(&2_u16.to_le_bytes());
            bytes.push(encode_point(point));
            bytes.extend_from_slice(&[0; 7]);
        }
        FaultWireCommand::Clear => bytes.extend_from_slice(&3_u16.to_le_bytes()),
        FaultWireCommand::Metrics => bytes.extend_from_slice(&4_u16.to_le_bytes()),
        FaultWireCommand::Trace {
            after_sequence,
            limit,
        } => {
            bytes.extend_from_slice(&5_u16.to_le_bytes());
            bytes.extend_from_slice(&after_sequence.to_le_bytes());
            bytes.extend_from_slice(&limit.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
        }
        FaultWireCommand::ClearTrace => bytes.extend_from_slice(&6_u16.to_le_bytes()),
    }
    bytes
}

pub fn apply_fault_wire_command(
    injector: &mut FaultInjector,
    bytes: &[u8],
) -> Result<Vec<u8>, FaultWireError> {
    let command = decode_fault_wire_command(bytes)?;
    let mut trace = Vec::new();
    let affected = match command {
        FaultWireCommand::Install(rule) => {
            injector.replace(rule).map_err(FaultWireError::Injection)?;
            1
        }
        FaultWireCommand::Remove(point) => {
            injector.remove(point).map_err(FaultWireError::Injection)?;
            1
        }
        FaultWireCommand::Clear => injector.clear(),
        FaultWireCommand::Metrics => 0,
        FaultWireCommand::Trace {
            after_sequence,
            limit,
        } => {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX).min(4096);
            trace.extend(injector.trace_after(after_sequence, limit));
            trace.len()
        }
        FaultWireCommand::ClearTrace => injector.clear_trace(),
    };
    Ok(encode_fault_wire_response(
        command,
        affected,
        injector.metrics(),
        &trace,
    ))
}

pub fn fault_wire_response_packet(
    engine: EngineId,
    source: PacketHeader,
    payload: Vec<u8>,
) -> OwnedEndpointPacket {
    OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::InspectionResponse,
            engine,
            channel_epoch: source.channel_epoch,
            commit_id: source.commit_id,
            base_revision: 0,
            new_revision: 0,
            required_control_revision: 0,
            source_simulation_revision: source.source_simulation_revision,
            sequence: source.sequence,
            payload_len: payload.len() as u32,
        },
        payload,
    }
}

pub fn encode_fault_wire_response(
    command: FaultWireCommand,
    affected: usize,
    metrics: FaultInjectionMetrics,
    trace: &[FaultTraceEvent],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(56 + trace.len().saturating_mul(32));
    bytes.extend_from_slice(RESPONSE_MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(
        &match command {
            FaultWireCommand::Install(_) => 1_u16,
            FaultWireCommand::Remove(_) => 2,
            FaultWireCommand::Clear => 3,
            FaultWireCommand::Metrics => 4,
            FaultWireCommand::Trace { .. } => 5,
            FaultWireCommand::ClearTrace => 6,
        }
        .to_le_bytes(),
    );
    bytes.extend_from_slice(&(affected as u64).to_le_bytes());
    bytes.extend_from_slice(&(metrics.installed_rules as u64).to_le_bytes());
    bytes.extend_from_slice(&metrics.evaluated.to_le_bytes());
    bytes.extend_from_slice(&metrics.injected.to_le_bytes());
    bytes.extend_from_slice(&metrics.exhausted.to_le_bytes());
    bytes.extend_from_slice(&(trace.len() as u64).to_le_bytes());
    for event in trace {
        bytes.extend_from_slice(&event.sequence.to_le_bytes());
        bytes.push(encode_point(event.point));
        bytes.push(encode_fault(event.fault));
        bytes.extend_from_slice(&[0; 6]);
        bytes.extend_from_slice(&event.visit.to_le_bytes());
        bytes.extend_from_slice(&event.remaining_after.to_le_bytes());
    }
    bytes
}

pub fn decode_fault_wire_response(bytes: &[u8]) -> Result<FaultWireResponse, FaultWireError> {
    if bytes.len() < 56
        || &bytes[..4] != RESPONSE_MAGIC
        || read_u16(bytes, 4) != VERSION
        || !(1..=6).contains(&read_u16(bytes, 6))
    {
        return Err(FaultWireError::Malformed);
    }
    let trace_count =
        usize::try_from(read_u64(bytes, 48)).map_err(|_| FaultWireError::Malformed)?;
    let expected_bytes = trace_count
        .checked_mul(32)
        .and_then(|trace_bytes| 56_usize.checked_add(trace_bytes))
        .ok_or(FaultWireError::Malformed)?;
    if trace_count > 4096 || bytes.len() != expected_bytes {
        return Err(FaultWireError::Malformed);
    }
    let installed_rules =
        usize::try_from(read_u64(bytes, 16)).map_err(|_| FaultWireError::Malformed)?;
    let mut trace = Vec::with_capacity(trace_count);
    for index in 0..trace_count {
        let offset = 56 + index * 32;
        trace.push(FaultTraceEvent {
            sequence: read_u64(bytes, offset),
            point: decode_point(bytes[offset + 8])?,
            fault: decode_fault(bytes[offset + 9])?,
            visit: read_u64(bytes, offset + 16),
            remaining_after: read_u64(bytes, offset + 24),
        });
    }
    Ok(FaultWireResponse {
        action: read_u16(bytes, 6),
        affected: read_u64(bytes, 8),
        metrics: FaultInjectionMetrics {
            installed_rules,
            evaluated: read_u64(bytes, 24),
            injected: read_u64(bytes, 32),
            exhausted: read_u64(bytes, 40),
        },
        trace,
    })
}

const fn encode_point(point: FaultPoint) -> u8 {
    match point {
        FaultPoint::ProtocolDecode => 1,
        FaultPoint::EndpointQueue => 2,
        FaultPoint::WorkerDispatch => 3,
        FaultPoint::ResourceAcquire => 4,
        FaultPoint::SurfaceSubmit => 5,
        FaultPoint::SurfacePresent => 6,
        FaultPoint::DeviceOperation => 7,
        FaultPoint::AudioOperation => 8,
        FaultPoint::Readback => 9,
        FaultPoint::FrameCapture => 10,
        FaultPoint::Shutdown => 11,
    }
}

fn decode_point(value: u8) -> Result<FaultPoint, FaultWireError> {
    match value {
        1 => Ok(FaultPoint::ProtocolDecode),
        2 => Ok(FaultPoint::EndpointQueue),
        3 => Ok(FaultPoint::WorkerDispatch),
        4 => Ok(FaultPoint::ResourceAcquire),
        5 => Ok(FaultPoint::SurfaceSubmit),
        6 => Ok(FaultPoint::SurfacePresent),
        7 => Ok(FaultPoint::DeviceOperation),
        8 => Ok(FaultPoint::AudioOperation),
        9 => Ok(FaultPoint::Readback),
        10 => Ok(FaultPoint::FrameCapture),
        11 => Ok(FaultPoint::Shutdown),
        _ => Err(FaultWireError::Unsupported),
    }
}

const fn encode_fault(fault: InjectedFault) -> u8 {
    match fault {
        InjectedFault::RejectBeforeDispatch => 1,
        InjectedFault::FailOwner => 2,
        InjectedFault::DropLatestOnly => 3,
        InjectedFault::OutcomeUnknown => 4,
        InjectedFault::SurfaceLost => 5,
        InjectedFault::DeviceLost => 6,
        InjectedFault::AudioDeviceLost => 7,
    }
}

fn decode_fault(value: u8) -> Result<InjectedFault, FaultWireError> {
    match value {
        1 => Ok(InjectedFault::RejectBeforeDispatch),
        2 => Ok(InjectedFault::FailOwner),
        3 => Ok(InjectedFault::DropLatestOnly),
        4 => Ok(InjectedFault::OutcomeUnknown),
        5 => Ok(InjectedFault::SurfaceLost),
        6 => Ok(InjectedFault::DeviceLost),
        7 => Ok(InjectedFault::AudioDeviceLost),
        _ => Err(FaultWireError::Unsupported),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
