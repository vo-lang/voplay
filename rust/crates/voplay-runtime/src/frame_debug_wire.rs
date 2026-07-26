use voplay_protocol::{EngineId, Handle, PacketHeader};

use crate::frame_debug_capture::{
    CapturedAttachment, CapturedNode, CapturedShaderDiagnostic, FrameCaptureId,
    FrameCaptureOutcome, FrameCaptureRequest, FrameCaptureResult,
};
use crate::{
    asset::ArtifactId,
    buffer_lease::BufferLease,
    render_graph::{GraphNodeId, GraphResourceId},
};

const REQUEST_MAGIC: &[u8; 4] = b"VFC1";
const RESULT_MAGIC: &[u8; 4] = b"VFC2";
pub const MAX_FRAME_CAPTURE_WIRE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameCaptureWireCommand {
    Submit(FrameCaptureRequest),
    Cancel(FrameCaptureId),
    ReadAttachment {
        request: u64,
        lease: BufferLease,
        offset: usize,
        len: usize,
        now_millis: u64,
    },
    ReleaseAttachment {
        request: u64,
        lease: BufferLease,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameCaptureWireError {
    Invalid,
    Capacity,
}

pub fn decode_frame_capture_command(
    header: PacketHeader,
    bytes: &[u8],
    endpoint_generation: Handle,
) -> Result<FrameCaptureWireCommand, FrameCaptureWireError> {
    if bytes.len() < 16
        || bytes.len() > MAX_FRAME_CAPTURE_WIRE_BYTES
        || bytes.get(..4) != Some(REQUEST_MAGIC)
        || bytes[6..8] != [0; 2]
    {
        return Err(FrameCaptureWireError::Invalid);
    }
    let id = FrameCaptureId(read_u64(bytes, 8)?);
    if id.0 == 0 || header.commit_id != id.0 {
        return Err(FrameCaptureWireError::Invalid);
    }
    match bytes[4] {
        1 if bytes.len() == 56 => {
            let encoded_generation = read_handle(bytes, 16)?;
            let flags = bytes[5];
            let request = FrameCaptureRequest {
                id,
                engine: header.engine,
                endpoint_generation: encoded_generation,
                frame_id: read_u64(bytes, 24)?,
                expected_graph_signature: read_u64(bytes, 32)?,
                include_attachments: flags & 1 != 0,
                include_shader_diagnostics: flags & 2 != 0,
                deadline_millis: read_u64(bytes, 40)?,
                max_bytes: usize::try_from(read_u64(bytes, 48)?)
                    .map_err(|_| FrameCaptureWireError::Capacity)?,
            };
            if flags & !3 != 0
                || request.endpoint_generation != endpoint_generation
                || request.frame_id == 0
                || request.expected_graph_signature == 0
                || request.deadline_millis == 0
                || request.max_bytes == 0
            {
                return Err(FrameCaptureWireError::Invalid);
            }
            Ok(FrameCaptureWireCommand::Submit(request))
        }
        2 if bytes.len() == 16 && bytes[5] == 0 => Ok(FrameCaptureWireCommand::Cancel(id)),
        3 if bytes.len() == 96 && bytes[5] == 0 => {
            let lease = read_wire_lease(header.engine, bytes, 16)?;
            let offset = usize::try_from(read_u64(bytes, 72)?)
                .map_err(|_| FrameCaptureWireError::Capacity)?;
            let len = usize::try_from(read_u32(bytes, 80)?)
                .map_err(|_| FrameCaptureWireError::Capacity)?;
            if bytes[84..88] != [0; 4] || len == 0 {
                return Err(FrameCaptureWireError::Invalid);
            }
            Ok(FrameCaptureWireCommand::ReadAttachment {
                request: id.0,
                lease,
                offset,
                len,
                now_millis: read_u64(bytes, 88)?,
            })
        }
        4 if bytes.len() == 72 && bytes[5] == 0 => Ok(FrameCaptureWireCommand::ReleaseAttachment {
            request: id.0,
            lease: read_wire_lease(header.engine, bytes, 16)?,
        }),
        _ => Err(FrameCaptureWireError::Invalid),
    }
}

pub fn encode_frame_capture_command(
    command: FrameCaptureWireCommand,
) -> Result<Vec<u8>, FrameCaptureWireError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(REQUEST_MAGIC);
    match command {
        FrameCaptureWireCommand::Submit(request) => {
            if request.id.0 == 0
                || !request.engine.is_valid()
                || !request.endpoint_generation.is_valid()
                || request.frame_id == 0
                || request.expected_graph_signature == 0
                || request.deadline_millis == 0
                || request.max_bytes == 0
            {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(1);
            bytes.push(
                u8::from(request.include_attachments)
                    | (u8::from(request.include_shader_diagnostics) << 1),
            );
            bytes.extend_from_slice(&[0; 2]);
            bytes.extend_from_slice(&request.id.0.to_le_bytes());
            put_handle(&mut bytes, request.endpoint_generation);
            bytes.extend_from_slice(&request.frame_id.to_le_bytes());
            bytes.extend_from_slice(&request.expected_graph_signature.to_le_bytes());
            bytes.extend_from_slice(&request.deadline_millis.to_le_bytes());
            bytes.extend_from_slice(
                &u64::try_from(request.max_bytes)
                    .map_err(|_| FrameCaptureWireError::Capacity)?
                    .to_le_bytes(),
            );
        }
        FrameCaptureWireCommand::Cancel(id) => {
            if id.0 == 0 {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(2);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&id.0.to_le_bytes());
        }
        FrameCaptureWireCommand::ReadAttachment {
            request,
            lease,
            offset,
            len,
            now_millis,
        } => {
            if request == 0 || len == 0 {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(3);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&request.to_le_bytes());
            put_wire_lease(&mut bytes, lease)?;
            bytes.extend_from_slice(
                &u64::try_from(offset)
                    .map_err(|_| FrameCaptureWireError::Capacity)?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(
                &u32::try_from(len)
                    .map_err(|_| FrameCaptureWireError::Capacity)?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&now_millis.to_le_bytes());
        }
        FrameCaptureWireCommand::ReleaseAttachment { request, lease } => {
            if request == 0 {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(4);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&request.to_le_bytes());
            put_wire_lease(&mut bytes, lease)?;
        }
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameCaptureAttachmentResponse {
    Chunk {
        request: u64,
        lease: BufferLease,
        offset: usize,
        bytes: Vec<u8>,
    },
    Released {
        request: u64,
        lease: BufferLease,
    },
}

pub fn encode_frame_capture_attachment_response(
    response: &FrameCaptureAttachmentResponse,
) -> Result<Vec<u8>, FrameCaptureWireError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"VFL2");
    match response {
        FrameCaptureAttachmentResponse::Chunk {
            request,
            lease,
            offset,
            bytes: chunk,
        } => {
            if *request == 0 || chunk.is_empty() {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(1);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&request.to_le_bytes());
            put_wire_lease(&mut bytes, *lease)?;
            bytes.extend_from_slice(
                &u64::try_from(*offset)
                    .map_err(|_| FrameCaptureWireError::Capacity)?
                    .to_le_bytes(),
            );
            put_blob(&mut bytes, chunk)?;
        }
        FrameCaptureAttachmentResponse::Released { request, lease } => {
            if *request == 0 {
                return Err(FrameCaptureWireError::Invalid);
            }
            bytes.push(2);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&request.to_le_bytes());
            put_wire_lease(&mut bytes, *lease)?;
        }
    }
    Ok(bytes)
}

pub fn decode_frame_capture_attachment_response(
    engine: EngineId,
    bytes: &[u8],
) -> Result<FrameCaptureAttachmentResponse, FrameCaptureWireError> {
    if !engine.is_valid()
        || bytes.len() < 72
        || bytes.len() > MAX_FRAME_CAPTURE_WIRE_BYTES
        || bytes.get(..4) != Some(b"VFL2")
        || bytes[5..8] != [0; 3]
    {
        return Err(FrameCaptureWireError::Invalid);
    }
    let request = read_u64(bytes, 8)?;
    let lease = read_wire_lease(engine, bytes, 16)?;
    if request == 0 {
        return Err(FrameCaptureWireError::Invalid);
    }
    match bytes[4] {
        1 if bytes.len() >= 84 => {
            let offset = usize::try_from(read_u64(bytes, 72)?)
                .map_err(|_| FrameCaptureWireError::Capacity)?;
            let len = read_u32(bytes, 80)? as usize;
            let chunk = bytes
                .get(84..)
                .filter(|chunk| chunk.len() == len && !chunk.is_empty())
                .ok_or(FrameCaptureWireError::Invalid)?
                .to_vec();
            Ok(FrameCaptureAttachmentResponse::Chunk {
                request,
                lease,
                offset,
                bytes: chunk,
            })
        }
        2 if bytes.len() == 72 => Ok(FrameCaptureAttachmentResponse::Released { request, lease }),
        _ => Err(FrameCaptureWireError::Invalid),
    }
}

pub fn encode_frame_capture_result(
    result: &FrameCaptureResult,
) -> Result<Vec<u8>, FrameCaptureWireError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(RESULT_MAGIC);
    bytes.push(2);
    bytes.push(outcome_tag(result.outcome));
    bytes.push(
        u8::from(result.request.include_attachments)
            | (u8::from(result.request.include_shader_diagnostics) << 1),
    );
    bytes.push(0);
    bytes.extend_from_slice(&result.request.id.0.to_le_bytes());
    bytes.extend_from_slice(&result.request.frame_id.to_le_bytes());
    bytes.extend_from_slice(&result.graph_signature.to_le_bytes());
    bytes.extend_from_slice(&result.request.expected_graph_signature.to_le_bytes());
    put_handle(&mut bytes, result.request.endpoint_generation);
    bytes.extend_from_slice(&result.request.deadline_millis.to_le_bytes());
    bytes.extend_from_slice(
        &u64::try_from(result.request.max_bytes)
            .map_err(|_| FrameCaptureWireError::Capacity)?
            .to_le_bytes(),
    );
    put_u32(&mut bytes, result.nodes.len())?;
    put_u32(&mut bytes, result.attachments.len())?;
    put_u32(&mut bytes, result.shader_diagnostics.len())?;
    put_blob(&mut bytes, result.diagnostic.as_bytes())?;
    for node in &result.nodes {
        bytes.extend_from_slice(&node.node.0.to_le_bytes());
        bytes.push(node.queue);
        bytes.extend_from_slice(&[0; 3]);
        bytes.extend_from_slice(&node.started_nanos.to_le_bytes());
        bytes.extend_from_slice(&node.finished_nanos.to_le_bytes());
        bytes.extend_from_slice(&node.pipeline_hash.to_le_bytes());
        bytes.extend_from_slice(&node.draw_calls.to_le_bytes());
        bytes.extend_from_slice(&node.dispatch_calls.to_le_bytes());
        put_blob(&mut bytes, node.label.as_bytes())?;
    }
    for attachment in &result.attachments {
        bytes.extend_from_slice(&attachment.resource.0.to_le_bytes());
        bytes.extend_from_slice(&attachment.version.to_le_bytes());
        bytes.extend_from_slice(&attachment.width.to_le_bytes());
        bytes.extend_from_slice(&attachment.height.to_le_bytes());
        bytes.extend_from_slice(&attachment.format.to_le_bytes());
        bytes.extend_from_slice(&attachment.row_bytes.to_le_bytes());
        put_handle(&mut bytes, attachment.lease.engine);
        put_handle(&mut bytes, attachment.lease.provider_generation);
        put_handle(&mut bytes, attachment.lease.consumer);
        put_handle(&mut bytes, attachment.lease.handle);
        bytes.extend_from_slice(&attachment.lease.artifact_id.0);
        bytes.extend_from_slice(
            &u64::try_from(attachment.lease.len)
                .map_err(|_| FrameCaptureWireError::Capacity)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&attachment.lease.deadline_millis.to_le_bytes());
    }
    for diagnostic in &result.shader_diagnostics {
        bytes.extend_from_slice(&diagnostic.pipeline_hash.to_le_bytes());
        bytes.push(diagnostic.severity);
        bytes.extend_from_slice(&[0; 3]);
        bytes.extend_from_slice(&diagnostic.source_start.to_le_bytes());
        bytes.extend_from_slice(&diagnostic.source_end.to_le_bytes());
        bytes.extend_from_slice(&diagnostic.graph_node.0.to_le_bytes());
        put_blob(&mut bytes, diagnostic.message.as_bytes())?;
    }
    if bytes.len() > MAX_FRAME_CAPTURE_WIRE_BYTES {
        return Err(FrameCaptureWireError::Capacity);
    }
    Ok(bytes)
}

pub fn decode_frame_capture_result(
    engine: EngineId,
    bytes: &[u8],
) -> Result<FrameCaptureResult, FrameCaptureWireError> {
    if !engine.is_valid()
        || bytes.len() < 80
        || bytes.len() > MAX_FRAME_CAPTURE_WIRE_BYTES
        || bytes.get(..4) != Some(RESULT_MAGIC)
        || bytes[4] != 2
        || bytes[7] != 0
        || bytes[6] & !3 != 0
    {
        return Err(FrameCaptureWireError::Invalid);
    }
    let mut reader = ResultReader { bytes, offset: 8 };
    let id = FrameCaptureId(reader.u64()?);
    let frame_id = reader.u64()?;
    let graph_signature = reader.u64()?;
    let expected_graph_signature = reader.u64()?;
    let endpoint_generation = reader.handle()?;
    let deadline_millis = reader.u64()?;
    let max_bytes = usize::try_from(reader.u64()?).map_err(|_| FrameCaptureWireError::Capacity)?;
    let node_count = reader.len(4096)?;
    let attachment_count = reader.len(128)?;
    let diagnostic_count = reader.len(1024)?;
    let diagnostic = reader.text()?.to_owned();
    if id.0 == 0
        || frame_id == 0
        || graph_signature == 0
        || expected_graph_signature == 0
        || deadline_millis == 0
        || max_bytes == 0
    {
        return Err(FrameCaptureWireError::Invalid);
    }
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let node = GraphNodeId(reader.u32()?);
        let queue = reader.u8()?;
        if reader.take(3)? != [0; 3] {
            return Err(FrameCaptureWireError::Invalid);
        }
        let started_nanos = reader.u64()?;
        let finished_nanos = reader.u64()?;
        let pipeline_hash = reader.u64()?;
        let draw_calls = reader.u32()?;
        let dispatch_calls = reader.u32()?;
        let label = reader.text()?.to_owned();
        if node.0 == 0
            || queue == 0
            || finished_nanos < started_nanos
            || pipeline_hash == 0
            || label.is_empty()
        {
            return Err(FrameCaptureWireError::Invalid);
        }
        nodes.push(CapturedNode {
            node,
            label,
            queue,
            started_nanos,
            finished_nanos,
            pipeline_hash,
            draw_calls,
            dispatch_calls,
        });
    }
    let mut attachments = Vec::with_capacity(attachment_count);
    for _ in 0..attachment_count {
        let resource = GraphResourceId(reader.u32()?);
        let version = reader.u32()?;
        let width = reader.u32()?;
        let height = reader.u32()?;
        let format = reader.u32()?;
        let row_bytes = reader.u32()?;
        let lease = BufferLease {
            engine: reader.handle()?,
            provider_generation: reader.handle()?,
            consumer: reader.handle()?,
            handle: reader.handle()?,
            artifact_id: ArtifactId(reader.array_16()?),
            len: usize::try_from(reader.u64()?).map_err(|_| FrameCaptureWireError::Capacity)?,
            deadline_millis: reader.u64()?,
        };
        if resource.0 == 0
            || version == 0
            || width == 0
            || height == 0
            || format == 0
            || row_bytes == 0
            || lease.artifact_id.0 == [0; 16]
            || lease.len == 0
            || lease.deadline_millis == 0
        {
            return Err(FrameCaptureWireError::Invalid);
        }
        attachments.push(CapturedAttachment {
            resource,
            version,
            width,
            height,
            format,
            row_bytes,
            lease,
        });
    }
    let mut shader_diagnostics = Vec::with_capacity(diagnostic_count);
    for _ in 0..diagnostic_count {
        let pipeline_hash = reader.u64()?;
        let severity = reader.u8()?;
        if reader.take(3)? != [0; 3] {
            return Err(FrameCaptureWireError::Invalid);
        }
        let source_start = reader.u32()?;
        let source_end = reader.u32()?;
        let graph_node = GraphNodeId(reader.u32()?);
        let message = reader.text()?.to_owned();
        if pipeline_hash == 0
            || severity == 0
            || source_end < source_start
            || graph_node.0 == 0
            || message.is_empty()
        {
            return Err(FrameCaptureWireError::Invalid);
        }
        shader_diagnostics.push(CapturedShaderDiagnostic {
            pipeline_hash,
            severity,
            source_start,
            source_end,
            graph_node,
            message,
        });
    }
    if reader.offset != bytes.len() {
        return Err(FrameCaptureWireError::Invalid);
    }
    Ok(FrameCaptureResult {
        request: FrameCaptureRequest {
            id,
            engine,
            endpoint_generation,
            frame_id,
            expected_graph_signature,
            include_attachments: bytes[6] & 1 != 0,
            include_shader_diagnostics: bytes[6] & 2 != 0,
            deadline_millis,
            max_bytes,
        },
        graph_signature,
        outcome: decode_outcome(bytes[5])?,
        nodes,
        attachments,
        shader_diagnostics,
        diagnostic,
    })
}

fn outcome_tag(outcome: FrameCaptureOutcome) -> u8 {
    match outcome {
        FrameCaptureOutcome::Completed => 1,
        FrameCaptureOutcome::Cancelled => 2,
        FrameCaptureOutcome::DeadlineExceeded => 3,
        FrameCaptureOutcome::GraphChanged => 4,
        FrameCaptureOutcome::EndpointRestartedBeforeCapture => 5,
        FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart => 6,
        FrameCaptureOutcome::Failed => 7,
    }
}

fn decode_outcome(value: u8) -> Result<FrameCaptureOutcome, FrameCaptureWireError> {
    match value {
        1 => Ok(FrameCaptureOutcome::Completed),
        2 => Ok(FrameCaptureOutcome::Cancelled),
        3 => Ok(FrameCaptureOutcome::DeadlineExceeded),
        4 => Ok(FrameCaptureOutcome::GraphChanged),
        5 => Ok(FrameCaptureOutcome::EndpointRestartedBeforeCapture),
        6 => Ok(FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart),
        7 => Ok(FrameCaptureOutcome::Failed),
        _ => Err(FrameCaptureWireError::Invalid),
    }
}

fn put_u32(bytes: &mut Vec<u8>, value: usize) -> Result<(), FrameCaptureWireError> {
    bytes.extend_from_slice(
        &u32::try_from(value)
            .map_err(|_| FrameCaptureWireError::Capacity)?
            .to_le_bytes(),
    );
    Ok(())
}

fn put_blob(bytes: &mut Vec<u8>, value: &[u8]) -> Result<(), FrameCaptureWireError> {
    put_u32(bytes, value.len())?;
    bytes.extend_from_slice(value);
    Ok(())
}

fn put_handle(bytes: &mut Vec<u8>, handle: Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, FrameCaptureWireError> {
    bytes
        .get(offset..offset + 8)
        .ok_or(FrameCaptureWireError::Invalid)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| FrameCaptureWireError::Invalid)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FrameCaptureWireError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(FrameCaptureWireError::Invalid)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| FrameCaptureWireError::Invalid)
}

fn read_handle(bytes: &[u8], offset: usize) -> Result<Handle, FrameCaptureWireError> {
    let handle = Handle {
        index: u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or(FrameCaptureWireError::Invalid)?
                .try_into()
                .map_err(|_| FrameCaptureWireError::Invalid)?,
        ),
        generation: u32::from_le_bytes(
            bytes
                .get(offset + 4..offset + 8)
                .ok_or(FrameCaptureWireError::Invalid)?
                .try_into()
                .map_err(|_| FrameCaptureWireError::Invalid)?,
        ),
    };
    handle
        .is_valid()
        .then_some(handle)
        .ok_or(FrameCaptureWireError::Invalid)
}

fn put_wire_lease(bytes: &mut Vec<u8>, lease: BufferLease) -> Result<(), FrameCaptureWireError> {
    if !lease.engine.is_valid()
        || !lease.provider_generation.is_valid()
        || !lease.consumer.is_valid()
        || !lease.handle.is_valid()
        || lease.artifact_id.0 == [0; 16]
        || lease.len == 0
        || lease.deadline_millis == 0
    {
        return Err(FrameCaptureWireError::Invalid);
    }
    put_handle(bytes, lease.provider_generation);
    put_handle(bytes, lease.consumer);
    put_handle(bytes, lease.handle);
    bytes.extend_from_slice(&lease.artifact_id.0);
    bytes.extend_from_slice(
        &u64::try_from(lease.len)
            .map_err(|_| FrameCaptureWireError::Capacity)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&lease.deadline_millis.to_le_bytes());
    Ok(())
}

fn read_wire_lease(
    engine: EngineId,
    bytes: &[u8],
    offset: usize,
) -> Result<BufferLease, FrameCaptureWireError> {
    let lease = BufferLease {
        engine,
        provider_generation: read_handle(bytes, offset)?,
        consumer: read_handle(bytes, offset + 8)?,
        handle: read_handle(bytes, offset + 16)?,
        artifact_id: ArtifactId(
            bytes
                .get(offset + 24..offset + 40)
                .ok_or(FrameCaptureWireError::Invalid)?
                .try_into()
                .map_err(|_| FrameCaptureWireError::Invalid)?,
        ),
        len: usize::try_from(read_u64(bytes, offset + 40)?)
            .map_err(|_| FrameCaptureWireError::Capacity)?,
        deadline_millis: read_u64(bytes, offset + 48)?,
    };
    if lease.artifact_id.0 == [0; 16] || lease.len == 0 || lease.deadline_millis == 0 {
        return Err(FrameCaptureWireError::Invalid);
    }
    Ok(lease)
}

pub fn frame_capture_engine(result: &FrameCaptureResult) -> EngineId {
    result.request.engine
}

struct ResultReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ResultReader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], FrameCaptureWireError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(FrameCaptureWireError::Capacity)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(FrameCaptureWireError::Invalid)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, FrameCaptureWireError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, FrameCaptureWireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, FrameCaptureWireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn handle(&mut self) -> Result<Handle, FrameCaptureWireError> {
        let handle = Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        handle
            .is_valid()
            .then_some(handle)
            .ok_or(FrameCaptureWireError::Invalid)
    }

    fn len(&mut self, max: usize) -> Result<usize, FrameCaptureWireError> {
        let len = self.u32()? as usize;
        (len <= max)
            .then_some(len)
            .ok_or(FrameCaptureWireError::Capacity)
    }

    fn text(&mut self) -> Result<&'a str, FrameCaptureWireError> {
        let len = self.u32()? as usize;
        std::str::from_utf8(self.take(len)?).map_err(|_| FrameCaptureWireError::Invalid)
    }

    fn array_16(&mut self) -> Result<[u8; 16], FrameCaptureWireError> {
        Ok(self.take(16)?.try_into().unwrap())
    }
}
