use voplay_protocol::{generated::MessageKind, Handle, PacketHeader};

use crate::{
    control::{
        ControlCommitted, ControlDependencyRef, ControlDomain, ControlError,
        ControlRefAllocatorLease, ControlTxnBuilder, DescriptorDependency, RetirementFence,
        RetirementProgress, StableControlRef,
    },
    endpoint::OwnedEndpointPacket,
};

const LEASE_REQUEST_MAGIC: [u8; 4] = *b"VCLR";
const LEASE_GRANTED_MAGIC: [u8; 4] = *b"VCLG";
const COMMIT_MAGIC: [u8; 4] = *b"VCT1";
const COMMITTED_MAGIC: [u8; 4] = *b"VCC1";
const OBSERVED_MAGIC: [u8; 4] = *b"VCO1";
const VERSION: u16 = 1;
const OBSERVED_VERSION: u16 = 2;
const COMMIT_PREFIX_BYTES: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlAuthorityWireError {
    WrongKind,
    WrongEngine,
    InvalidHeader,
    Malformed,
    UnsupportedVersion,
    Capacity,
    Control(ControlError),
}

impl From<ControlError> for ControlAuthorityWireError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

pub struct DecodedControlCommit {
    pub domain: ControlDomain,
    pub logic_generation: Handle,
    pub builder: ControlTxnBuilder,
}

pub struct DecodedControlRealizationResults {
    pub domain: ControlDomain,
    pub endpoint_generation: Handle,
    pub revision: u64,
    pub results: Vec<(StableControlRef, crate::control::RealizationOutcome)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedControlObserved {
    pub domain: ControlDomain,
    pub transaction_id: u64,
    pub revision: u64,
    pub progress: RetirementProgress,
}

pub fn decode_control_lease_request(
    header: PacketHeader,
    payload: &[u8],
) -> Result<(Handle, Handle, u32), ControlAuthorityWireError> {
    if header.kind != MessageKind::ControlLeaseRequest {
        return Err(ControlAuthorityWireError::WrongKind);
    }
    validate_header(header, payload)?;
    if payload.len() != 28 || payload[0..4] != LEASE_REQUEST_MAGIC {
        return Err(ControlAuthorityWireError::Malformed);
    }
    validate_version(payload)?;
    let producer = read_handle(payload, 8)?;
    let logic_generation = read_handle(payload, 16)?;
    let capacity = read_u32(payload, 24)?;
    if capacity == 0 || header.commit_id == 0 {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    Ok((producer, logic_generation, capacity))
}

pub fn encode_control_lease_granted(
    source: PacketHeader,
    lease: ControlRefAllocatorLease,
) -> Result<OwnedEndpointPacket, ControlAuthorityWireError> {
    if source.kind != MessageKind::ControlLeaseRequest
        || source.engine != lease.engine()
        || source.commit_id == 0
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut payload = Vec::with_capacity(36);
    payload.extend_from_slice(&LEASE_GRANTED_MAGIC);
    payload.extend_from_slice(&VERSION.to_le_bytes());
    payload.extend_from_slice(&0_u16.to_le_bytes());
    write_handle(&mut payload, lease.producer());
    write_handle(&mut payload, lease.logic_generation());
    write_handle(&mut payload, lease.lease());
    payload.extend_from_slice(&lease.capacity().to_le_bytes());
    Ok(OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::ControlLeaseGranted,
            engine: source.engine,
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
    })
}

pub fn decode_control_commit(
    header: PacketHeader,
    payload: &[u8],
) -> Result<DecodedControlCommit, ControlAuthorityWireError> {
    if header.kind != MessageKind::ControlCommit {
        return Err(ControlAuthorityWireError::WrongKind);
    }
    validate_header(header, payload)?;
    if payload.len() < COMMIT_PREFIX_BYTES || payload[0..4] != COMMIT_MAGIC {
        return Err(ControlAuthorityWireError::Malformed);
    }
    validate_version(payload)?;
    let domain = match payload[6] {
        1 => ControlDomain::Render,
        2 => ControlDomain::Audio,
        _ => return Err(ControlAuthorityWireError::Malformed),
    };
    if payload[7] != 0
        || header.commit_id == 0
        || header.base_revision.checked_add(1) != Some(header.new_revision)
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let producer = read_handle(payload, 8)?;
    let logic_generation = read_handle(payload, 16)?;
    let lease_handle = read_handle(payload, 24)?;
    let capacity = read_u32(payload, 32)?;
    let operation_count = read_u32(payload, 36)? as usize;
    if capacity == 0 || operation_count == 0 || operation_count > capacity as usize {
        return Err(ControlAuthorityWireError::Capacity);
    }
    let lease = ControlRefAllocatorLease::from_wire_parts(
        header.engine,
        producer,
        logic_generation,
        lease_handle,
        capacity,
    );
    let mut builder = lease.begin(domain, header.commit_id, header.base_revision);
    let mut offset = COMMIT_PREFIX_BYTES;
    for _ in 0..operation_count {
        let tag = *payload
            .get(offset)
            .ok_or(ControlAuthorityWireError::Malformed)?;
        let kind = decode_kind(
            *payload
                .get(offset + 1)
                .ok_or(ControlAuthorityWireError::Malformed)?,
        )?;
        if payload.get(offset + 2..offset + 4) != Some(&[0, 0]) || kind.domain() != domain {
            return Err(ControlAuthorityWireError::Malformed);
        }
        match tag {
            1 => {
                let token = read_u32(payload, offset + 4)?;
                let descriptor_len = read_u32(payload, offset + 8)? as usize;
                let dependency_count = read_u32(payload, offset + 12)? as usize;
                offset = offset
                    .checked_add(16)
                    .ok_or(ControlAuthorityWireError::Capacity)?;
                let descriptor_end = offset
                    .checked_add(descriptor_len)
                    .filter(|end| *end <= payload.len())
                    .ok_or(ControlAuthorityWireError::Malformed)?;
                let descriptor = payload[offset..descriptor_end].to_vec();
                offset = descriptor_end;
                let mut dependencies = Vec::with_capacity(dependency_count);
                for _ in 0..dependency_count {
                    let dependency_end = offset
                        .checked_add(16)
                        .filter(|end| *end <= payload.len())
                        .ok_or(ControlAuthorityWireError::Malformed)?;
                    let binding_offset = read_u32(payload, offset)? as usize;
                    let reference_kind = decode_kind(payload[offset + 5])?;
                    if payload[offset + 6..offset + 8] != [0, 0]
                        || reference_kind.domain() != domain
                    {
                        return Err(ControlAuthorityWireError::Malformed);
                    }
                    let identity = read_u32(payload, offset + 8)?;
                    let generation = read_u32(payload, offset + 12)?;
                    let reference = match payload[offset + 4] {
                        1 if generation == 0 => ControlDependencyRef::Provisional {
                            token: identity,
                            kind: reference_kind,
                        },
                        2 => ControlDependencyRef::Stable(StableControlRef {
                            engine: header.engine,
                            kind: reference_kind,
                            handle: Handle {
                                index: identity,
                                generation,
                            },
                        }),
                        _ => return Err(ControlAuthorityWireError::Malformed),
                    };
                    dependencies.push(DescriptorDependency {
                        offset: binding_offset,
                        reference,
                    });
                    offset = dependency_end;
                }
                let assigned = builder.create_with_dependencies(kind, descriptor, dependencies)?;
                if assigned != token {
                    return Err(ControlAuthorityWireError::Malformed);
                }
            }
            2 => {
                let end = offset
                    .checked_add(44)
                    .filter(|end| *end <= payload.len())
                    .ok_or(ControlAuthorityWireError::Malformed)?;
                let stable = StableControlRef {
                    engine: header.engine,
                    kind,
                    handle: Handle {
                        index: read_u32(payload, offset + 4)?,
                        generation: read_u32(payload, offset + 8)?,
                    },
                };
                builder.retire(
                    stable,
                    RetirementFence {
                        last_render_revision: read_u64(payload, offset + 12)?,
                        last_domain_frame: read_u64(payload, offset + 20)?,
                        last_event_sequence: read_u64(payload, offset + 28)?,
                        last_audio_event_sequence: read_u64(payload, offset + 36)?,
                    },
                )?;
                offset = end;
            }
            _ => return Err(ControlAuthorityWireError::Malformed),
        }
    }
    if offset != payload.len() {
        return Err(ControlAuthorityWireError::Malformed);
    }
    Ok(DecodedControlCommit {
        domain,
        logic_generation,
        builder,
    })
}

pub fn encode_control_committed(
    source: PacketHeader,
    domain: ControlDomain,
    committed: &ControlCommitted,
) -> Result<OwnedEndpointPacket, ControlAuthorityWireError> {
    if source.kind != MessageKind::ControlCommit
        || source.commit_id != committed.transaction_id
        || source.new_revision != committed.control_revision
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut payload = Vec::with_capacity(16 + committed.promotions.len() * 16);
    payload.extend_from_slice(&COMMITTED_MAGIC);
    payload.extend_from_slice(&VERSION.to_le_bytes());
    payload.push(match domain {
        ControlDomain::Render => 1,
        ControlDomain::Audio => 2,
    });
    payload.push(0);
    payload.extend_from_slice(&(committed.promotions.len() as u32).to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    for (token, stable) in &committed.promotions {
        payload.extend_from_slice(&token.to_le_bytes());
        payload.push(kind_tag(stable.kind));
        payload.extend_from_slice(&[0; 3]);
        write_handle(&mut payload, stable.handle);
    }
    Ok(OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::ControlCommitted,
            engine: source.engine,
            channel_epoch: source.channel_epoch,
            commit_id: committed.transaction_id,
            base_revision: source.base_revision,
            new_revision: committed.control_revision,
            required_control_revision: committed.control_revision,
            source_simulation_revision: source.source_simulation_revision,
            sequence: source.sequence,
            payload_len: payload.len() as u32,
        },
        payload,
    })
}

pub fn encode_control_realization_results(
    source: PacketHeader,
    domain: ControlDomain,
    endpoint_generation: Handle,
    results: &[(StableControlRef, crate::control::RealizationOutcome)],
) -> Result<OwnedEndpointPacket, ControlAuthorityWireError> {
    if !endpoint_generation.is_valid()
        || source.new_revision == 0
        || results
            .iter()
            .any(|(stable, _)| stable.engine != source.engine || stable.kind.domain() != domain)
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut payload = Vec::with_capacity(32 + results.len() * 12);
    payload.extend_from_slice(b"VCRR");
    payload.extend_from_slice(&VERSION.to_le_bytes());
    payload.push(match domain {
        ControlDomain::Render => 1,
        ControlDomain::Audio => 2,
    });
    payload.push(0);
    write_handle(&mut payload, endpoint_generation);
    payload.extend_from_slice(&source.new_revision.to_le_bytes());
    payload.extend_from_slice(
        &u32::try_from(results.len())
            .map_err(|_| ControlAuthorityWireError::Capacity)?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&0_u32.to_le_bytes());
    for (stable, outcome) in results {
        payload.push(kind_tag(stable.kind));
        payload.push(match outcome {
            crate::control::RealizationOutcome::Live => 1,
            crate::control::RealizationOutcome::TransientFailure => 2,
            crate::control::RealizationOutcome::PermanentFailure => 3,
        });
        payload.extend_from_slice(&0_u16.to_le_bytes());
        write_handle(&mut payload, stable.handle);
    }
    Ok(OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::ControlRealizationResult,
            engine: source.engine,
            channel_epoch: source.channel_epoch,
            commit_id: source.commit_id,
            base_revision: source.base_revision,
            new_revision: source.new_revision,
            required_control_revision: source.new_revision,
            source_simulation_revision: source.source_simulation_revision,
            sequence: source.sequence,
            payload_len: payload.len() as u32,
        },
        payload,
    })
}

pub fn decode_control_observed(
    header: PacketHeader,
    payload: &[u8],
) -> Result<DecodedControlObserved, ControlAuthorityWireError> {
    decode_control_observation_packet(header, payload, MessageKind::ControlObserved)
}

pub fn decode_control_observed_ack(
    header: PacketHeader,
    payload: &[u8],
) -> Result<DecodedControlObserved, ControlAuthorityWireError> {
    decode_control_observation_packet(header, payload, MessageKind::ControlObservedAck)
}

fn decode_control_observation_packet(
    header: PacketHeader,
    payload: &[u8],
    expected_kind: MessageKind,
) -> Result<DecodedControlObserved, ControlAuthorityWireError> {
    if header.kind != expected_kind {
        return Err(ControlAuthorityWireError::WrongKind);
    }
    validate_header(header, payload)?;
    if payload.len() != 56 || payload[0..4] != OBSERVED_MAGIC {
        return Err(ControlAuthorityWireError::Malformed);
    }
    if read_u16(payload, 4)? != OBSERVED_VERSION {
        return Err(ControlAuthorityWireError::UnsupportedVersion);
    }
    let domain = match payload[6] {
        1 => ControlDomain::Render,
        2 => ControlDomain::Audio,
        _ => return Err(ControlAuthorityWireError::Malformed),
    };
    let transaction_id = read_u64(payload, 8)?;
    let revision = read_u64(payload, 16)?;
    let progress = RetirementProgress {
        render_revision: read_u64(payload, 24)?,
        domain_frame: read_u64(payload, 32)?,
        event_sequence: read_u64(payload, 40)?,
        audio_event_sequence: read_u64(payload, 48)?,
    };
    if payload[7] != 0
        || transaction_id == 0
        || revision == 0
        || header.commit_id != transaction_id
        || header.new_revision != revision
        || header.required_control_revision != revision
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    Ok(DecodedControlObserved {
        domain,
        transaction_id,
        revision,
        progress,
    })
}

pub fn encode_control_observed(
    source: PacketHeader,
    domain: ControlDomain,
    progress: RetirementProgress,
) -> Result<OwnedEndpointPacket, ControlAuthorityWireError> {
    let expected_kind = match domain {
        ControlDomain::Render => MessageKind::RenderControlTransaction,
        ControlDomain::Audio => MessageKind::AudioControlTransaction,
    };
    if source.kind != expected_kind
        || !source.engine.is_valid()
        || source.channel_epoch == 0
        || source.commit_id == 0
        || source.new_revision == 0
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut payload = Vec::with_capacity(56);
    payload.extend_from_slice(&OBSERVED_MAGIC);
    payload.extend_from_slice(&OBSERVED_VERSION.to_le_bytes());
    payload.push(match domain {
        ControlDomain::Render => 1,
        ControlDomain::Audio => 2,
    });
    payload.push(0);
    payload.extend_from_slice(&source.commit_id.to_le_bytes());
    payload.extend_from_slice(&source.new_revision.to_le_bytes());
    payload.extend_from_slice(&progress.render_revision.to_le_bytes());
    payload.extend_from_slice(&progress.domain_frame.to_le_bytes());
    payload.extend_from_slice(&progress.event_sequence.to_le_bytes());
    payload.extend_from_slice(&progress.audio_event_sequence.to_le_bytes());
    Ok(OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::ControlObserved,
            engine: source.engine,
            channel_epoch: source.channel_epoch,
            commit_id: source.commit_id,
            base_revision: source.base_revision,
            new_revision: source.new_revision,
            required_control_revision: source.new_revision,
            source_simulation_revision: source.source_simulation_revision,
            sequence: source.sequence,
            payload_len: payload.len() as u32,
        },
        payload,
    })
}

pub fn encode_control_observed_ack(
    source: PacketHeader,
    observed: DecodedControlObserved,
) -> Result<OwnedEndpointPacket, ControlAuthorityWireError> {
    if source.kind != MessageKind::ControlObserved
        || source.commit_id != observed.transaction_id
        || source.new_revision != observed.revision
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut payload = Vec::with_capacity(56);
    payload.extend_from_slice(&OBSERVED_MAGIC);
    payload.extend_from_slice(&OBSERVED_VERSION.to_le_bytes());
    payload.push(match observed.domain {
        ControlDomain::Render => 1,
        ControlDomain::Audio => 2,
    });
    payload.push(0);
    payload.extend_from_slice(&observed.transaction_id.to_le_bytes());
    payload.extend_from_slice(&observed.revision.to_le_bytes());
    payload.extend_from_slice(&observed.progress.render_revision.to_le_bytes());
    payload.extend_from_slice(&observed.progress.domain_frame.to_le_bytes());
    payload.extend_from_slice(&observed.progress.event_sequence.to_le_bytes());
    payload.extend_from_slice(&observed.progress.audio_event_sequence.to_le_bytes());
    Ok(OwnedEndpointPacket {
        header: PacketHeader {
            kind: MessageKind::ControlObservedAck,
            engine: source.engine,
            channel_epoch: source.channel_epoch,
            commit_id: observed.transaction_id,
            base_revision: source.base_revision,
            new_revision: observed.revision,
            required_control_revision: observed.revision,
            source_simulation_revision: source.source_simulation_revision,
            sequence: source.sequence,
            payload_len: payload.len() as u32,
        },
        payload,
    })
}

pub fn decode_control_realization_results(
    header: PacketHeader,
    payload: &[u8],
) -> Result<DecodedControlRealizationResults, ControlAuthorityWireError> {
    if header.kind != MessageKind::ControlRealizationResult {
        return Err(ControlAuthorityWireError::WrongKind);
    }
    validate_header(header, payload)?;
    if payload.len() < 32 || payload[0..4] != *b"VCRR" {
        return Err(ControlAuthorityWireError::Malformed);
    }
    validate_version(payload)?;
    let domain = match payload[6] {
        1 => ControlDomain::Render,
        2 => ControlDomain::Audio,
        _ => return Err(ControlAuthorityWireError::Malformed),
    };
    if payload[7] != 0 || read_u32(payload, 28)? != 0 {
        return Err(ControlAuthorityWireError::Malformed);
    }
    let endpoint_generation = read_handle(payload, 8)?;
    let revision = read_u64(payload, 16)?;
    let count = read_u32(payload, 24)? as usize;
    if revision == 0
        || revision != header.new_revision
        || payload.len() != 32_usize.saturating_add(count.saturating_mul(12))
    {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    let mut results = Vec::with_capacity(count);
    let mut offset = 32;
    for _ in 0..count {
        let kind = decode_kind(payload[offset])?;
        let outcome = match payload[offset + 1] {
            1 => crate::control::RealizationOutcome::Live,
            2 => crate::control::RealizationOutcome::TransientFailure,
            3 => crate::control::RealizationOutcome::PermanentFailure,
            _ => return Err(ControlAuthorityWireError::Malformed),
        };
        if payload[offset + 2..offset + 4] != [0, 0] || kind.domain() != domain {
            return Err(ControlAuthorityWireError::Malformed);
        }
        results.push((
            StableControlRef {
                engine: header.engine,
                kind,
                handle: read_handle(payload, offset + 4)?,
            },
            outcome,
        ));
        offset += 12;
    }
    Ok(DecodedControlRealizationResults {
        domain,
        endpoint_generation,
        revision,
        results,
    })
}

fn validate_header(header: PacketHeader, payload: &[u8]) -> Result<(), ControlAuthorityWireError> {
    if !header.engine.is_valid() {
        return Err(ControlAuthorityWireError::WrongEngine);
    }
    if header.channel_epoch == 0 || header.payload_len as usize != payload.len() {
        return Err(ControlAuthorityWireError::InvalidHeader);
    }
    Ok(())
}

fn validate_version(payload: &[u8]) -> Result<(), ControlAuthorityWireError> {
    if read_u16(payload, 4)? != VERSION {
        return Err(ControlAuthorityWireError::UnsupportedVersion);
    }
    Ok(())
}

fn decode_kind(tag: u8) -> Result<crate::control::ControlKind, ControlAuthorityWireError> {
    use crate::control::ControlKind;
    match tag {
        1 => Ok(ControlKind::RenderView),
        2 => Ok(ControlKind::RenderTarget),
        3 => Ok(ControlKind::AudioBus),
        4 => Ok(ControlKind::PersistentAudioSource),
        5 => Ok(ControlKind::RenderGraph),
        _ => Err(ControlAuthorityWireError::Malformed),
    }
}

const fn kind_tag(kind: crate::control::ControlKind) -> u8 {
    use crate::control::ControlKind;
    match kind {
        ControlKind::RenderView => 1,
        ControlKind::RenderTarget => 2,
        ControlKind::AudioBus => 3,
        ControlKind::PersistentAudioSource => 4,
        ControlKind::RenderGraph => 5,
    }
}

fn write_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn read_handle(bytes: &[u8], offset: usize) -> Result<Handle, ControlAuthorityWireError> {
    let handle = Handle {
        index: read_u32(bytes, offset)?,
        generation: read_u32(bytes, offset + 4)?,
    };
    if !handle.is_valid() {
        return Err(ControlAuthorityWireError::Malformed);
    }
    Ok(handle)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ControlAuthorityWireError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(ControlAuthorityWireError::Malformed)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ControlAuthorityWireError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ControlAuthorityWireError::Malformed)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ControlAuthorityWireError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ControlAuthorityWireError::Malformed)
}
