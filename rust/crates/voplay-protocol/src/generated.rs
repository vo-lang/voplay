// @generated from framework schema. DO NOT EDIT.
pub const SCHEMA_ID: &str = "voplay.engine";
pub const SCHEMA_FORMAT: u32 = 1;
pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 1;
pub const SCHEMA_IDENTITY: [u8; 16] = [
    206, 245, 237, 231, 88, 77, 170, 113, 29, 84, 60, 189, 166, 204, 202, 241,
];
pub const MAJOR_COMPAT_FINGERPRINT: [u8; 32] = [
    144, 249, 88, 53, 204, 13, 79, 236, 201, 174, 144, 69, 245, 109, 57, 183, 29, 57, 151, 61, 115,
    177, 74, 89, 244, 251, 123, 68, 212, 103, 28, 188,
];
pub const EXACT_SCHEMA_FINGERPRINT: [u8; 32] = [
    254, 141, 190, 190, 145, 134, 236, 92, 49, 20, 233, 54, 105, 125, 113, 77, 59, 191, 22, 106,
    121, 226, 94, 198, 251, 202, 144, 105, 197, 44, 191, 161,
];
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 262144;
pub const MAX_PACKET_BYTES: usize = 67108864;
pub const MAX_SNAPSHOT_OBJECTS: usize = 1000000;
pub const MAX_STAGED_EVENT_BYTES: usize = 4194304;
pub const MAX_STAGED_EVENTS: usize = 4096;
pub const MAX_TRANSACTION_OPS: usize = 131072;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum MessageKind {
    RenderStateTransaction = 1,
    RenderStateAck = 2,
    RenderStateSnapshot = 3,
    RenderEvent = 4,
    RenderEventResult = 5,
    RenderControlTransaction = 6,
    RenderControlAck = 7,
    AudioControlTransaction = 8,
    AudioEvent = 9,
    AudioEventResult = 10,
    AudioControlAck = 11,
    EngineStart = 12,
    EngineReady = 13,
    EngineSuspend = 14,
    EngineResume = 15,
    EngineClose = 16,
    EngineClosed = 17,
    TickInput = 18,
    TickResult = 19,
    FramePulse = 20,
    AssetRequest = 21,
    AssetCompletion = 22,
    AssetControl = 23,
    PhysicsBatch = 24,
    PhysicsResult = 25,
    Diagnostics = 26,
    InspectionRequest = 27,
    InspectionResponse = 28,
    RenderTransient = 29,
    SurfaceControl = 30,
    DeviceEvent = 31,
    WorkerWake = 32,
    PlatformInput = 33,
    HapticsCommand = 34,
    HapticsResult = 35,
    AudioAssetData = 36,
    RenderAssetData = 37,
    RenderAssetAck = 38,
    AudioAssetAck = 39,
    RenderReadbackRequest = 40,
    RenderReadbackResult = 41,
    ControlLeaseRequest = 42,
    ControlLeaseGranted = 43,
    ControlCommit = 44,
    ControlCommitted = 45,
    ControlRealizationResult = 46,
    ControlObserved = 47,
    ControlObservedAck = 48,
    ControlStateAdopt = 49,
    ControlStateAdopted = 50,
}

impl MessageKind {
    pub const fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::RenderStateTransaction),
            2 => Some(Self::RenderStateAck),
            3 => Some(Self::RenderStateSnapshot),
            4 => Some(Self::RenderEvent),
            5 => Some(Self::RenderEventResult),
            6 => Some(Self::RenderControlTransaction),
            7 => Some(Self::RenderControlAck),
            8 => Some(Self::AudioControlTransaction),
            9 => Some(Self::AudioEvent),
            10 => Some(Self::AudioEventResult),
            11 => Some(Self::AudioControlAck),
            12 => Some(Self::EngineStart),
            13 => Some(Self::EngineReady),
            14 => Some(Self::EngineSuspend),
            15 => Some(Self::EngineResume),
            16 => Some(Self::EngineClose),
            17 => Some(Self::EngineClosed),
            18 => Some(Self::TickInput),
            19 => Some(Self::TickResult),
            20 => Some(Self::FramePulse),
            21 => Some(Self::AssetRequest),
            22 => Some(Self::AssetCompletion),
            23 => Some(Self::AssetControl),
            24 => Some(Self::PhysicsBatch),
            25 => Some(Self::PhysicsResult),
            26 => Some(Self::Diagnostics),
            27 => Some(Self::InspectionRequest),
            28 => Some(Self::InspectionResponse),
            29 => Some(Self::RenderTransient),
            30 => Some(Self::SurfaceControl),
            31 => Some(Self::DeviceEvent),
            32 => Some(Self::WorkerWake),
            33 => Some(Self::PlatformInput),
            34 => Some(Self::HapticsCommand),
            35 => Some(Self::HapticsResult),
            36 => Some(Self::AudioAssetData),
            37 => Some(Self::RenderAssetData),
            38 => Some(Self::RenderAssetAck),
            39 => Some(Self::AudioAssetAck),
            40 => Some(Self::RenderReadbackRequest),
            41 => Some(Self::RenderReadbackResult),
            42 => Some(Self::ControlLeaseRequest),
            43 => Some(Self::ControlLeaseGranted),
            44 => Some(Self::ControlCommit),
            45 => Some(Self::ControlCommitted),
            46 => Some(Self::ControlRealizationResult),
            47 => Some(Self::ControlObserved),
            48 => Some(Self::ControlObservedAck),
            49 => Some(Self::ControlStateAdopt),
            50 => Some(Self::ControlStateAdopted),
            _ => None,
        }
    }
}

pub const HEADER_BYTES: usize = 80;
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(C)]
pub struct GenerationalHandle {
    pub index: u32,
    pub generation: u32,
}

impl GenerationalHandle {
    pub const INVALID: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    pub const fn is_valid(self) -> bool {
        self.index != u32::MAX && self.generation != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameworkPacketHeader {
    pub kind: MessageKind,
    pub engine: GenerationalHandle,
    pub channel_epoch: u64,
    pub commit_id: u64,
    pub base_revision: u64,
    pub new_revision: u64,
    pub required_control_revision: u64,
    pub source_simulation_revision: u64,
    pub sequence: u64,
    pub payload_len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameworkPacketCodecError {
    Truncated,
    UnknownKind,
    InvalidHandle,
    PayloadTooLarge,
    LengthMismatch,
    LengthOverflow,
}

pub fn decode_framework_packet(
    bytes: &[u8],
) -> Result<(FrameworkPacketHeader, &[u8]), FrameworkPacketCodecError> {
    if bytes.len() < HEADER_BYTES {
        return Err(FrameworkPacketCodecError::Truncated);
    }
    let payload_length = read_framework_u32(bytes, 76);
    let payload_bytes = payload_length as usize;
    if payload_bytes > MAX_PACKET_BYTES.saturating_sub(HEADER_BYTES) {
        return Err(FrameworkPacketCodecError::PayloadTooLarge);
    }
    if bytes.len() != HEADER_BYTES + payload_bytes {
        return Err(FrameworkPacketCodecError::LengthMismatch);
    }
    let header = FrameworkPacketHeader {
        kind: MessageKind::from_wire(read_framework_u16(bytes, 0))
            .ok_or(FrameworkPacketCodecError::UnknownKind)?,
        engine: {
            let value = GenerationalHandle {
                index: read_framework_u32(bytes, 4),
                generation: read_framework_u32(bytes, 8),
            };
            if !value.is_valid() {
                return Err(FrameworkPacketCodecError::InvalidHandle);
            }
            value
        },
        channel_epoch: read_framework_u64(bytes, 12),
        commit_id: read_framework_u64(bytes, 20),
        base_revision: read_framework_u64(bytes, 28),
        new_revision: read_framework_u64(bytes, 36),
        required_control_revision: read_framework_u64(bytes, 44),
        source_simulation_revision: read_framework_u64(bytes, 52),
        sequence: read_framework_u64(bytes, 60),
        payload_len: read_framework_u32(bytes, 76),
    };
    Ok((header, &bytes[HEADER_BYTES..]))
}

pub fn encode_framework_packet(
    header: FrameworkPacketHeader,
    payload: &[u8],
) -> Result<Vec<u8>, FrameworkPacketCodecError> {
    if payload.len() > MAX_PACKET_BYTES.saturating_sub(HEADER_BYTES) {
        return Err(FrameworkPacketCodecError::PayloadTooLarge);
    }
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| FrameworkPacketCodecError::LengthOverflow)?;
    if header.payload_len != payload_length {
        return Err(FrameworkPacketCodecError::LengthMismatch);
    }
    if !header.engine.is_valid() {
        return Err(FrameworkPacketCodecError::InvalidHandle);
    }
    let mut bytes = vec![0u8; HEADER_BYTES + payload.len()];
    bytes[0..2].copy_from_slice(&(header.kind as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&header.engine.index.to_le_bytes());
    bytes[8..12].copy_from_slice(&header.engine.generation.to_le_bytes());
    bytes[12..20].copy_from_slice(&header.channel_epoch.to_le_bytes());
    bytes[20..28].copy_from_slice(&header.commit_id.to_le_bytes());
    bytes[28..36].copy_from_slice(&header.base_revision.to_le_bytes());
    bytes[36..44].copy_from_slice(&header.new_revision.to_le_bytes());
    bytes[44..52].copy_from_slice(&header.required_control_revision.to_le_bytes());
    bytes[52..60].copy_from_slice(&header.source_simulation_revision.to_le_bytes());
    bytes[60..68].copy_from_slice(&header.sequence.to_le_bytes());
    bytes[76..80].copy_from_slice(&header.payload_len.to_le_bytes());
    bytes[HEADER_BYTES..].copy_from_slice(payload);
    Ok(bytes)
}

fn read_framework_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_framework_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_framework_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
