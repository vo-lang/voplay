pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub use generated::{
    FrameworkPacketHeader as PacketHeader, GenerationalHandle as Handle, MessageKind, HEADER_BYTES,
    MAX_PACKET_BYTES,
};

pub type EngineId = Handle;
pub type EntityId = Handle;
pub type PresentationDomainId = Handle;

pub mod web_lane;

pub const TICK_OUTPUT_MAGIC: &[u8] = b"voplay-tick-output-v1\0";
pub const MAX_TICK_OUTPUT_PACKETS: usize = 4096;
pub const MAX_TICK_OUTPUT_PACKET_BYTES: usize = 1024 * 1024;
pub const MAX_TICK_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum TickOutputRole {
    Render = 1,
    Asset = 2,
    Audio = 3,
    Logic = 4,
}

impl TickOutputRole {
    pub const fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            1 => Self::Render,
            2 => Self::Asset,
            3 => Self::Audio,
            4 => Self::Logic,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickOutputPacket<'a> {
    pub role: TickOutputRole,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickOutputCodecError {
    PacketCount,
    PacketSize,
    TotalSize,
    RoleOrder,
    Truncated,
    TrailingBytes,
}

pub fn encode_tick_output<'a>(
    packets: impl IntoIterator<Item = TickOutputPacket<'a>>,
) -> Result<Vec<u8>, TickOutputCodecError> {
    let mut output = Vec::from(TICK_OUTPUT_MAGIC);
    output.extend_from_slice(&0_u32.to_le_bytes());
    let mut count = 0_usize;
    let mut previous_role = None;
    for packet in packets {
        count = count
            .checked_add(1)
            .filter(|count| *count <= MAX_TICK_OUTPUT_PACKETS)
            .ok_or(TickOutputCodecError::PacketCount)?;
        if packet.bytes.is_empty() || packet.bytes.len() > MAX_TICK_OUTPUT_PACKET_BYTES {
            return Err(TickOutputCodecError::PacketSize);
        }
        if previous_role.is_some_and(|previous| previous > packet.role) {
            return Err(TickOutputCodecError::RoleOrder);
        }
        let next_len = output
            .len()
            .checked_add(5)
            .and_then(|len| len.checked_add(packet.bytes.len()))
            .filter(|len| *len <= MAX_TICK_OUTPUT_BYTES)
            .ok_or(TickOutputCodecError::TotalSize)?;
        output.reserve(next_len - output.len());
        output.push(packet.role as u8);
        output.extend_from_slice(&(packet.bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(packet.bytes);
        previous_role = Some(packet.role);
    }
    output[TICK_OUTPUT_MAGIC.len()..TICK_OUTPUT_MAGIC.len() + 4]
        .copy_from_slice(&(count as u32).to_le_bytes());
    Ok(output)
}

pub fn decode_tick_output(bytes: &[u8]) -> Result<Vec<TickOutputPacket<'_>>, TickOutputCodecError> {
    if !bytes.starts_with(TICK_OUTPUT_MAGIC) {
        return Err(TickOutputCodecError::Truncated);
    }
    let mut cursor = TICK_OUTPUT_MAGIC.len();
    let count = read_tick_output_u32(bytes, cursor)? as usize;
    cursor += 4;
    if count > MAX_TICK_OUTPUT_PACKETS || bytes.len() > MAX_TICK_OUTPUT_BYTES {
        return Err(TickOutputCodecError::PacketCount);
    }
    let mut packets = Vec::with_capacity(count);
    let mut previous_role = None;
    for _ in 0..count {
        let role =
            TickOutputRole::from_tag(*bytes.get(cursor).ok_or(TickOutputCodecError::Truncated)?)
                .ok_or(TickOutputCodecError::RoleOrder)?;
        let packet_len = read_tick_output_u32(bytes, cursor + 1)? as usize;
        cursor = cursor
            .checked_add(5)
            .ok_or(TickOutputCodecError::TotalSize)?;
        let end = cursor
            .checked_add(packet_len)
            .ok_or(TickOutputCodecError::TotalSize)?;
        if packet_len == 0 || packet_len > MAX_TICK_OUTPUT_PACKET_BYTES {
            return Err(TickOutputCodecError::PacketSize);
        }
        if previous_role.is_some_and(|previous| previous > role) {
            return Err(TickOutputCodecError::RoleOrder);
        }
        let packet = bytes
            .get(cursor..end)
            .ok_or(TickOutputCodecError::Truncated)?;
        packets.push(TickOutputPacket {
            role,
            bytes: packet,
        });
        previous_role = Some(role);
        cursor = end;
    }
    if cursor != bytes.len() {
        return Err(TickOutputCodecError::TrailingBytes);
    }
    Ok(packets)
}

fn read_tick_output_u32(bytes: &[u8], offset: usize) -> Result<u32, TickOutputCodecError> {
    let field = bytes
        .get(offset..offset.saturating_add(4))
        .and_then(|field| field.try_into().ok())
        .ok_or(TickOutputCodecError::Truncated)?;
    Ok(u32::from_le_bytes(field))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    UnknownKind,
    InvalidEngine,
    PayloadTooLarge,
    LengthMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    InvalidEngine,
    PayloadTooLarge,
    LengthOverflow,
}

pub fn decode_packet(bytes: &[u8]) -> Result<(PacketHeader, &[u8]), DecodeError> {
    generated::decode_framework_packet(bytes).map_err(|error| match error {
        generated::FrameworkPacketCodecError::Truncated => DecodeError::Truncated,
        generated::FrameworkPacketCodecError::UnknownKind => DecodeError::UnknownKind,
        generated::FrameworkPacketCodecError::InvalidHandle => DecodeError::InvalidEngine,
        generated::FrameworkPacketCodecError::PayloadTooLarge => DecodeError::PayloadTooLarge,
        generated::FrameworkPacketCodecError::LengthMismatch
        | generated::FrameworkPacketCodecError::LengthOverflow => DecodeError::LengthMismatch,
    })
}

pub fn encode_packet(mut header: PacketHeader, payload: &[u8]) -> Result<Vec<u8>, EncodeError> {
    header.payload_len = u32::try_from(payload.len()).map_err(|_| EncodeError::LengthOverflow)?;
    generated::encode_framework_packet(header, payload).map_err(|error| match error {
        generated::FrameworkPacketCodecError::InvalidHandle => EncodeError::InvalidEngine,
        generated::FrameworkPacketCodecError::PayloadTooLarge => EncodeError::PayloadTooLarge,
        generated::FrameworkPacketCodecError::LengthOverflow
        | generated::FrameworkPacketCodecError::LengthMismatch
        | generated::FrameworkPacketCodecError::Truncated
        | generated::FrameworkPacketCodecError::UnknownKind => EncodeError::LengthOverflow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_bounds_and_engine_identity_precede_payload_exposure() {
        let mut bytes = vec![0_u8; HEADER_BYTES + 2];
        bytes[0..2].copy_from_slice(
            &(generated::MessageKind::RenderStateTransaction as u16).to_le_bytes(),
        );
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2_u32.to_le_bytes());
        bytes[12..20].copy_from_slice(&3_u64.to_le_bytes());
        bytes[20..28].copy_from_slice(&4_u64.to_le_bytes());
        bytes[28..36].copy_from_slice(&5_u64.to_le_bytes());
        bytes[36..44].copy_from_slice(&6_u64.to_le_bytes());
        bytes[44..52].copy_from_slice(&7_u64.to_le_bytes());
        bytes[52..60].copy_from_slice(&8_u64.to_le_bytes());
        bytes[60..68].copy_from_slice(&9_u64.to_le_bytes());
        bytes[76..80].copy_from_slice(&2_u32.to_le_bytes());
        bytes[80..].copy_from_slice(b"tx");

        let (header, payload) = decode_packet(&bytes).unwrap();
        assert_eq!(header.channel_epoch, 3);
        assert_eq!(header.commit_id, 4);
        assert_eq!(header.base_revision, 5);
        assert_eq!(header.new_revision, 6);
        assert_eq!(payload, b"tx");

        bytes[8..12].fill(0);
        assert_eq!(decode_packet(&bytes), Err(DecodeError::InvalidEngine));
    }
}
