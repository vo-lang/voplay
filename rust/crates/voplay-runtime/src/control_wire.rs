use voplay_protocol::{generated::MessageKind, Handle, PacketHeader};

use crate::control::{
    ControlDomain, ControlKind, ControlSnapshotEntry, ControlSnapshotState, ControlStateSnapshot,
    RetirementFence, StableControlRef,
};

const AUDIO_MAGIC: [u8; 4] = *b"VACS";
const RENDER_MAGIC: [u8; 4] = *b"VRCS";
const VERSION: u16 = 1;
const PREFIX_BYTES: usize = 20;
const ENTRY_PREFIX_BYTES: usize = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioWireError {
    WrongKind,
    WrongEngine,
    InvalidHeader,
    Malformed,
    UnsupportedVersion,
    EntryCapacity,
    DescriptorCapacity,
    DuplicateEntry,
}

pub fn encode_audio_control_snapshot(
    snapshot: &ControlStateSnapshot,
    channel_epoch: u64,
) -> Result<Vec<u8>, AudioWireError> {
    encode_control_snapshot(
        snapshot,
        channel_epoch,
        ControlDomain::Audio,
        MessageKind::AudioControlTransaction,
        AUDIO_MAGIC,
    )
}

pub fn encode_render_control_snapshot(
    snapshot: &ControlStateSnapshot,
    channel_epoch: u64,
) -> Result<Vec<u8>, AudioWireError> {
    encode_control_snapshot(
        snapshot,
        channel_epoch,
        ControlDomain::Render,
        MessageKind::RenderControlTransaction,
        RENDER_MAGIC,
    )
}

fn encode_control_snapshot(
    snapshot: &ControlStateSnapshot,
    channel_epoch: u64,
    domain: ControlDomain,
    message_kind: MessageKind,
    magic: [u8; 4],
) -> Result<Vec<u8>, AudioWireError> {
    if !snapshot.engine.is_valid() || snapshot.domain != domain || snapshot.revision == 0 {
        return Err(AudioWireError::InvalidHeader);
    }
    let count = u32::try_from(snapshot.entries.len()).map_err(|_| AudioWireError::EntryCapacity)?;
    let descriptor_bytes = snapshot.entries.iter().try_fold(0_usize, |total, entry| {
        total
            .checked_add(entry.descriptor.len())
            .ok_or(AudioWireError::DescriptorCapacity)
    })?;
    let payload_len = PREFIX_BYTES
        .checked_add(
            ENTRY_PREFIX_BYTES
                .checked_mul(snapshot.entries.len())
                .ok_or(AudioWireError::DescriptorCapacity)?,
        )
        .and_then(|bytes| bytes.checked_add(descriptor_bytes))
        .ok_or(AudioWireError::DescriptorCapacity)?;
    if payload_len
        > voplay_protocol::generated::MAX_PACKET_BYTES.saturating_sub(voplay_protocol::HEADER_BYTES)
    {
        return Err(AudioWireError::DescriptorCapacity);
    }
    let mut payload = Vec::with_capacity(payload_len);
    payload.extend_from_slice(&magic);
    payload.extend_from_slice(&VERSION.to_le_bytes());
    payload.extend_from_slice(&0_u16.to_le_bytes());
    payload.extend_from_slice(&snapshot.last_transaction_id.to_le_bytes());
    payload.extend_from_slice(&count.to_le_bytes());
    for entry in &snapshot.entries {
        if entry.stable.engine != snapshot.engine
            || entry.stable.kind.domain() != domain
            || !entry.stable.handle.is_valid()
        {
            return Err(AudioWireError::InvalidHeader);
        }
        payload.push(kind_tag(entry.stable.kind)?);
        payload.push(match entry.state {
            ControlSnapshotState::Desired => 1,
            ControlSnapshotState::Tombstone { .. } => 2,
        });
        payload.extend_from_slice(&0_u16.to_le_bytes());
        payload.extend_from_slice(&entry.stable.handle.index.to_le_bytes());
        payload.extend_from_slice(&entry.stable.handle.generation.to_le_bytes());
        let descriptor_len = u32::try_from(entry.descriptor.len())
            .map_err(|_| AudioWireError::DescriptorCapacity)?;
        payload.extend_from_slice(&descriptor_len.to_le_bytes());
        let fence = match entry.state {
            ControlSnapshotState::Desired => RetirementFence {
                last_render_revision: 0,
                last_domain_frame: 0,
                last_event_sequence: 0,
                last_audio_event_sequence: 0,
            },
            ControlSnapshotState::Tombstone { fence } => fence,
        };
        payload.extend_from_slice(&fence.last_render_revision.to_le_bytes());
        payload.extend_from_slice(&fence.last_domain_frame.to_le_bytes());
        payload.extend_from_slice(&fence.last_event_sequence.to_le_bytes());
        payload.extend_from_slice(&fence.last_audio_event_sequence.to_le_bytes());
        payload.extend_from_slice(&entry.descriptor);
    }
    voplay_protocol::encode_packet(
        PacketHeader {
            kind: message_kind,
            engine: snapshot.engine,
            channel_epoch,
            commit_id: snapshot.last_transaction_id,
            base_revision: snapshot.revision.saturating_sub(1),
            new_revision: snapshot.revision,
            required_control_revision: 0,
            source_simulation_revision: 0,
            sequence: snapshot.last_transaction_id,
            payload_len: payload.len() as u32,
        },
        &payload,
    )
    .map_err(|_| AudioWireError::DescriptorCapacity)
}

pub fn decode_audio_control_snapshot_parts(
    header: PacketHeader,
    payload: &[u8],
    max_entries: usize,
    max_descriptor_bytes: usize,
) -> Result<ControlStateSnapshot, AudioWireError> {
    decode_control_snapshot_parts(
        header,
        payload,
        max_entries,
        max_descriptor_bytes,
        ControlDomain::Audio,
        MessageKind::AudioControlTransaction,
        AUDIO_MAGIC,
    )
}

pub fn decode_render_control_snapshot_parts(
    header: PacketHeader,
    payload: &[u8],
    max_entries: usize,
    max_descriptor_bytes: usize,
) -> Result<ControlStateSnapshot, AudioWireError> {
    decode_control_snapshot_parts(
        header,
        payload,
        max_entries,
        max_descriptor_bytes,
        ControlDomain::Render,
        MessageKind::RenderControlTransaction,
        RENDER_MAGIC,
    )
}

fn decode_control_snapshot_parts(
    header: PacketHeader,
    payload: &[u8],
    max_entries: usize,
    max_descriptor_bytes: usize,
    domain: ControlDomain,
    message_kind: MessageKind,
    magic: [u8; 4],
) -> Result<ControlStateSnapshot, AudioWireError> {
    if header.kind != message_kind {
        return Err(AudioWireError::WrongKind);
    }
    if !header.engine.is_valid() {
        return Err(AudioWireError::WrongEngine);
    }
    if header.new_revision == 0
        || header.commit_id == 0
        || header.base_revision.checked_add(1) != Some(header.new_revision)
        || header.payload_len as usize != payload.len()
    {
        return Err(AudioWireError::InvalidHeader);
    }
    if payload.len() < PREFIX_BYTES || payload[0..4] != magic {
        return Err(AudioWireError::Malformed);
    }
    if read_u16(payload, 4) != VERSION {
        return Err(AudioWireError::UnsupportedVersion);
    }
    if read_u16(payload, 6) != 0 || read_u64(payload, 8) != header.commit_id {
        return Err(AudioWireError::InvalidHeader);
    }
    let count = read_u32(payload, 16) as usize;
    if count > max_entries {
        return Err(AudioWireError::EntryCapacity);
    }
    let minimum = PREFIX_BYTES
        .checked_add(
            ENTRY_PREFIX_BYTES
                .checked_mul(count)
                .ok_or(AudioWireError::Malformed)?,
        )
        .ok_or(AudioWireError::Malformed)?;
    if minimum > payload.len() {
        return Err(AudioWireError::Malformed);
    }
    let mut entries = Vec::with_capacity(count);
    let mut offset = PREFIX_BYTES;
    let mut descriptor_bytes = 0_usize;
    let mut identities = std::collections::BTreeSet::new();
    for _ in 0..count {
        if payload.len().saturating_sub(offset) < ENTRY_PREFIX_BYTES {
            return Err(AudioWireError::Malformed);
        }
        let kind = decode_kind(payload[offset])?;
        if kind.domain() != domain {
            return Err(AudioWireError::Malformed);
        }
        let state_tag = payload[offset + 1];
        if read_u16(payload, offset + 2) != 0 {
            return Err(AudioWireError::Malformed);
        }
        let handle = Handle {
            index: read_u32(payload, offset + 4),
            generation: read_u32(payload, offset + 8),
        };
        if !handle.is_valid() || !identities.insert((kind, handle)) {
            return Err(AudioWireError::DuplicateEntry);
        }
        let descriptor_len = read_u32(payload, offset + 12) as usize;
        descriptor_bytes = descriptor_bytes
            .checked_add(descriptor_len)
            .filter(|bytes| *bytes <= max_descriptor_bytes)
            .ok_or(AudioWireError::DescriptorCapacity)?;
        let fence = RetirementFence {
            last_render_revision: read_u64(payload, offset + 16),
            last_domain_frame: read_u64(payload, offset + 24),
            last_event_sequence: read_u64(payload, offset + 32),
            last_audio_event_sequence: read_u64(payload, offset + 40),
        };
        offset += ENTRY_PREFIX_BYTES;
        let end = offset
            .checked_add(descriptor_len)
            .filter(|end| *end <= payload.len())
            .ok_or(AudioWireError::Malformed)?;
        let state = match state_tag {
            1 if fence
                == (RetirementFence {
                    last_render_revision: 0,
                    last_domain_frame: 0,
                    last_event_sequence: 0,
                    last_audio_event_sequence: 0,
                }) =>
            {
                ControlSnapshotState::Desired
            }
            2 => ControlSnapshotState::Tombstone { fence },
            _ => return Err(AudioWireError::Malformed),
        };
        entries.push(ControlSnapshotEntry {
            stable: StableControlRef {
                engine: header.engine,
                kind,
                handle,
            },
            descriptor: payload[offset..end].to_vec(),
            state,
        });
        offset = end;
    }
    if offset != payload.len() {
        return Err(AudioWireError::Malformed);
    }
    Ok(ControlStateSnapshot {
        engine: header.engine,
        domain,
        revision: header.new_revision,
        last_transaction_id: header.commit_id,
        entries,
    })
}

fn kind_tag(kind: ControlKind) -> Result<u8, AudioWireError> {
    match kind {
        ControlKind::RenderView => Ok(1),
        ControlKind::RenderTarget => Ok(2),
        ControlKind::AudioBus => Ok(3),
        ControlKind::PersistentAudioSource => Ok(4),
        ControlKind::RenderGraph => Ok(5),
    }
}

fn decode_kind(tag: u8) -> Result<ControlKind, AudioWireError> {
    match tag {
        1 => Ok(ControlKind::RenderView),
        2 => Ok(ControlKind::RenderTarget),
        3 => Ok(ControlKind::AudioBus),
        4 => Ok(ControlKind::PersistentAudioSource),
        5 => Ok(ControlKind::RenderGraph),
        _ => Err(AudioWireError::Malformed),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
