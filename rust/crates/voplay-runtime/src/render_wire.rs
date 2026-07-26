use std::collections::BTreeMap;

use voplay_protocol::{
    decode_packet, encode_packet, generated::MessageKind, DecodeError, EncodeError, Handle,
    PacketHeader,
};

use crate::{
    outbox::{PresentationDomainId, RenderTransient},
    RenderCommitAck, RenderEntity, RenderOp, RenderStateSnapshot, RenderStateTransaction,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderWireError {
    Decode(DecodeError),
    Encode(EncodeError),
    WrongKind,
    InvalidHeader,
    InvalidPayload,
    Capacity,
    Truncated,
}

impl From<DecodeError> for RenderWireError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl From<EncodeError> for RenderWireError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

pub fn encode_render_transaction(
    transaction: &RenderStateTransaction,
) -> Result<Vec<u8>, RenderWireError> {
    if transaction.ops.len() > voplay_protocol::generated::MAX_TRANSACTION_OPS {
        return Err(RenderWireError::Capacity);
    }
    let mut payload = Vec::new();
    push_u32(&mut payload, transaction.ops.len())?;
    for op in &transaction.ops {
        encode_op(&mut payload, op)?;
    }
    encode_packet(
        PacketHeader {
            kind: MessageKind::RenderStateTransaction,
            engine: transaction.engine,
            channel_epoch: transaction.channel_epoch,
            commit_id: transaction.commit_id,
            base_revision: transaction.base_revision,
            new_revision: transaction.new_revision,
            required_control_revision: transaction.required_control_revision,
            source_simulation_revision: transaction.source_simulation_revision,
            sequence: 0,
            payload_len: 0,
        },
        &payload,
    )
    .map_err(Into::into)
}

pub fn decode_render_transaction(packet: &[u8]) -> Result<RenderStateTransaction, RenderWireError> {
    let (header, payload) = decode_packet(packet)?;
    decode_render_transaction_parts(header, payload)
}

pub fn decode_render_transaction_parts(
    header: PacketHeader,
    payload: &[u8],
) -> Result<RenderStateTransaction, RenderWireError> {
    if header.kind != MessageKind::RenderStateTransaction {
        return Err(RenderWireError::WrongKind);
    }
    if header.channel_epoch == 0
        || header.commit_id == 0
        || header.new_revision != header.base_revision.checked_add(1).unwrap_or(0)
        || header.sequence != 0
    {
        return Err(RenderWireError::InvalidHeader);
    }
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()? as usize;
    if count > voplay_protocol::generated::MAX_TRANSACTION_OPS || count > cursor.remaining() / 9 {
        return Err(RenderWireError::Capacity);
    }
    let mut ops = Vec::with_capacity(count);
    for _ in 0..count {
        ops.push(decode_op(header.engine, &mut cursor)?);
    }
    cursor.finish()?;
    Ok(RenderStateTransaction {
        engine: header.engine,
        channel_epoch: header.channel_epoch,
        commit_id: header.commit_id,
        base_revision: header.base_revision,
        new_revision: header.new_revision,
        required_control_revision: header.required_control_revision,
        source_simulation_revision: header.source_simulation_revision,
        ops,
    })
}

pub fn encode_render_snapshot(snapshot: &RenderStateSnapshot) -> Result<Vec<u8>, RenderWireError> {
    if snapshot.objects.len() > voplay_protocol::generated::MAX_SNAPSHOT_OBJECTS {
        return Err(RenderWireError::Capacity);
    }
    let mut payload = Vec::new();
    push_u32(&mut payload, snapshot.objects.len())?;
    for (entity, value) in &snapshot.objects {
        push_entity(&mut payload, *entity);
        push_bytes(&mut payload, value)?;
    }
    encode_packet(
        PacketHeader {
            kind: MessageKind::RenderStateSnapshot,
            engine: snapshot.engine,
            channel_epoch: snapshot.channel_epoch,
            commit_id: snapshot.commit_id,
            base_revision: 0,
            new_revision: snapshot.revision,
            required_control_revision: snapshot.required_control_revision,
            source_simulation_revision: snapshot.source_simulation_revision,
            sequence: 0,
            payload_len: 0,
        },
        &payload,
    )
    .map_err(Into::into)
}

pub fn decode_render_snapshot(packet: &[u8]) -> Result<RenderStateSnapshot, RenderWireError> {
    let (header, payload) = decode_packet(packet)?;
    decode_render_snapshot_parts(header, payload)
}

pub fn decode_render_snapshot_parts(
    header: PacketHeader,
    payload: &[u8],
) -> Result<RenderStateSnapshot, RenderWireError> {
    if header.kind != MessageKind::RenderStateSnapshot {
        return Err(RenderWireError::WrongKind);
    }
    if header.channel_epoch == 0
        || header.commit_id == 0
        || header.base_revision != 0
        || header.sequence != 0
    {
        return Err(RenderWireError::InvalidHeader);
    }
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()? as usize;
    if count > voplay_protocol::generated::MAX_SNAPSHOT_OBJECTS || count > cursor.remaining() / 12 {
        return Err(RenderWireError::Capacity);
    }
    let mut objects = BTreeMap::new();
    for _ in 0..count {
        let entity = cursor.entity(header.engine)?;
        let value = cursor.bytes()?.to_vec();
        if objects.insert(entity, value).is_some() {
            return Err(RenderWireError::InvalidPayload);
        }
    }
    cursor.finish()?;
    Ok(RenderStateSnapshot {
        engine: header.engine,
        channel_epoch: header.channel_epoch,
        commit_id: header.commit_id,
        revision: header.new_revision,
        required_control_revision: header.required_control_revision,
        source_simulation_revision: header.source_simulation_revision,
        objects,
    })
}

pub fn encode_render_ack(
    engine: Handle,
    channel_epoch: u64,
    ack: RenderCommitAck,
) -> Result<Vec<u8>, RenderWireError> {
    encode_packet(
        PacketHeader {
            kind: MessageKind::RenderStateAck,
            engine,
            channel_epoch,
            commit_id: ack.commit_id,
            base_revision: 0,
            new_revision: ack.revision,
            required_control_revision: 0,
            source_simulation_revision: 0,
            sequence: 0,
            payload_len: 0,
        },
        &[],
    )
    .map_err(Into::into)
}

pub fn decode_render_ack(packet: &[u8]) -> Result<(Handle, u64, RenderCommitAck), RenderWireError> {
    let (header, payload) = decode_packet(packet)?;
    decode_render_ack_parts(header, payload)
}

pub fn decode_render_ack_parts(
    header: PacketHeader,
    payload: &[u8],
) -> Result<(Handle, u64, RenderCommitAck), RenderWireError> {
    if header.kind != MessageKind::RenderStateAck {
        return Err(RenderWireError::WrongKind);
    }
    if !payload.is_empty()
        || header.channel_epoch == 0
        || header.commit_id == 0
        || header.base_revision != 0
        || header.required_control_revision != 0
        || header.source_simulation_revision != 0
        || header.sequence != 0
    {
        return Err(RenderWireError::InvalidHeader);
    }
    Ok((
        header.engine,
        header.channel_epoch,
        RenderCommitAck {
            commit_id: header.commit_id,
            revision: header.new_revision,
        },
    ))
}

pub fn encode_render_transient(
    transient: &RenderTransient,
    channel_epoch: u64,
) -> Result<Vec<u8>, RenderWireError> {
    if transient.bytes.len() > voplay_protocol::generated::MAX_EVENT_PAYLOAD_BYTES {
        return Err(RenderWireError::Capacity);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(&transient.domain.handle.index.to_le_bytes());
    payload.extend_from_slice(&transient.domain.handle.generation.to_le_bytes());
    push_bytes(&mut payload, &transient.bytes)?;
    encode_packet(
        PacketHeader {
            kind: MessageKind::RenderTransient,
            engine: transient.domain.engine,
            channel_epoch,
            commit_id: 0,
            base_revision: transient.required_render_revision,
            new_revision: 0,
            required_control_revision: transient.required_control_revision,
            source_simulation_revision: 0,
            sequence: transient.frame_id,
            payload_len: 0,
        },
        &payload,
    )
    .map_err(Into::into)
}

pub fn decode_render_transient(packet: &[u8]) -> Result<RenderTransient, RenderWireError> {
    let (header, payload) = decode_packet(packet)?;
    decode_render_transient_parts(header, payload)
}

pub fn decode_render_transient_parts(
    header: PacketHeader,
    payload: &[u8],
) -> Result<RenderTransient, RenderWireError> {
    if header.kind != MessageKind::RenderTransient {
        return Err(RenderWireError::WrongKind);
    }
    if header.channel_epoch == 0
        || header.commit_id != 0
        || header.new_revision != 0
        || header.source_simulation_revision != 0
        || header.sequence == 0
    {
        return Err(RenderWireError::InvalidHeader);
    }
    let mut cursor = Cursor::new(payload);
    let domain = PresentationDomainId {
        engine: header.engine,
        handle: Handle {
            index: cursor.u32()?,
            generation: cursor.u32()?,
        },
    };
    if !domain.handle.is_valid() {
        return Err(RenderWireError::InvalidPayload);
    }
    let bytes = cursor.bytes()?.to_vec();
    if bytes.len() > voplay_protocol::generated::MAX_EVENT_PAYLOAD_BYTES {
        return Err(RenderWireError::Capacity);
    }
    cursor.finish()?;
    Ok(RenderTransient {
        domain,
        frame_id: header.sequence,
        required_render_revision: header.base_revision,
        required_control_revision: header.required_control_revision,
        bytes,
    })
}

fn encode_op(payload: &mut Vec<u8>, op: &RenderOp) -> Result<(), RenderWireError> {
    match op {
        RenderOp::Spawn { entity, value } => {
            payload.push(1);
            push_entity(payload, *entity);
            push_bytes(payload, value)?;
        }
        RenderOp::Despawn { entity } => {
            payload.push(2);
            push_entity(payload, *entity);
        }
        RenderOp::Upsert { entity, value } => {
            payload.push(3);
            push_entity(payload, *entity);
            push_bytes(payload, value)?;
        }
    }
    Ok(())
}

fn decode_op(engine: Handle, cursor: &mut Cursor<'_>) -> Result<RenderOp, RenderWireError> {
    let tag = cursor.u8()?;
    let entity = cursor.entity(engine)?;
    match tag {
        1 => Ok(RenderOp::Spawn {
            entity,
            value: cursor.bytes()?.to_vec(),
        }),
        2 => Ok(RenderOp::Despawn { entity }),
        3 => Ok(RenderOp::Upsert {
            entity,
            value: cursor.bytes()?.to_vec(),
        }),
        _ => Err(RenderWireError::InvalidPayload),
    }
}

fn push_entity(payload: &mut Vec<u8>, entity: RenderEntity) {
    payload.extend_from_slice(&entity.entity.index.to_le_bytes());
    payload.extend_from_slice(&entity.entity.generation.to_le_bytes());
}

fn push_u32(payload: &mut Vec<u8>, value: usize) -> Result<(), RenderWireError> {
    let value = u32::try_from(value).map_err(|_| RenderWireError::Capacity)?;
    payload.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn push_bytes(payload: &mut Vec<u8>, value: &[u8]) -> Result<(), RenderWireError> {
    push_u32(payload, value.len())?;
    let new_len = payload
        .len()
        .checked_add(value.len())
        .filter(|length| *length <= voplay_protocol::generated::MAX_PACKET_BYTES)
        .ok_or(RenderWireError::Capacity)?;
    payload.reserve(new_len - payload.len());
    payload.extend_from_slice(value);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u8(&mut self) -> Result<u8, RenderWireError> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or(RenderWireError::Truncated)?;
        self.offset += 1;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, RenderWireError> {
        let end = self
            .offset
            .checked_add(4)
            .ok_or(RenderWireError::Truncated)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(RenderWireError::Truncated)?;
        self.offset = end;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], RenderWireError> {
        let length = self.u32()? as usize;
        let end = self
            .offset
            .checked_add(length)
            .ok_or(RenderWireError::Truncated)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(RenderWireError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    fn entity(&mut self, engine: Handle) -> Result<RenderEntity, RenderWireError> {
        let entity = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !entity.is_valid() {
            return Err(RenderWireError::InvalidPayload);
        }
        Ok(RenderEntity { engine, entity })
    }

    fn finish(self) -> Result<(), RenderWireError> {
        if self.offset != self.bytes.len() {
            return Err(RenderWireError::InvalidPayload);
        }
        Ok(())
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
}
