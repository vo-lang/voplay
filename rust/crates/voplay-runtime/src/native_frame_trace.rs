use crate::{
    frame_debug_capture::{CapturedNode, CapturedShaderDiagnostic},
    render_graph::{GraphNodeId, GraphResourceId},
};

const MAGIC: &[u8; 4] = b"VGT1";
const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
const MAX_VIEWS: usize = 256;
const MAX_RECORDS: usize = 65_536;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCapturedAttachment {
    pub resource: GraphResourceId,
    pub version: u32,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFrameTrace {
    pub frame_id: u64,
    pub graph_signature: u64,
    pub nodes: Vec<CapturedNode>,
    pub attachments: Vec<NativeCapturedAttachment>,
    pub shader_diagnostics: Vec<CapturedShaderDiagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFrameTraceError {
    Invalid,
    Capacity,
}

pub fn decode_native_frame_trace(bytes: &[u8]) -> Result<NativeFrameTrace, NativeFrameTraceError> {
    if bytes.len() < 28 || bytes.len() > MAX_TRACE_BYTES || bytes.get(..4) != Some(MAGIC) {
        return Err(NativeFrameTraceError::Invalid);
    }
    let mut reader = Reader { bytes, offset: 4 };
    if reader.u8()? != 1 || reader.take(3)? != [0; 3] {
        return Err(NativeFrameTraceError::Invalid);
    }
    let frame_id = reader.u64()?;
    let graph_signature = reader.u64()?;
    let view_count = reader.len(MAX_VIEWS)?;
    if frame_id == 0 || graph_signature == 0 || view_count == 0 {
        return Err(NativeFrameTraceError::Invalid);
    }
    let mut nodes = Vec::new();
    let mut attachments = Vec::new();
    let mut shader_diagnostics = Vec::new();
    for view_index in 0..view_count {
        let engine = reader.handle()?;
        if reader.u8()? != 2 || reader.take(3)? != [0; 3] {
            return Err(NativeFrameTraceError::Invalid);
        }
        let target = reader.handle()?;
        let viewport = [reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?];
        let fence = reader.u64()?;
        let node_count = reader.len(MAX_RECORDS)?;
        let attachment_count = reader.len(MAX_RECORDS)?;
        let diagnostic_count = reader.len(MAX_RECORDS)?;
        if !engine.is_valid()
            || !target.is_valid()
            || viewport[2] == 0
            || viewport[3] == 0
            || fence == 0
            || nodes.len().saturating_add(node_count) > MAX_RECORDS
            || attachments.len().saturating_add(attachment_count) > MAX_RECORDS
            || shader_diagnostics.len().saturating_add(diagnostic_count) > MAX_RECORDS
        {
            return Err(NativeFrameTraceError::Capacity);
        }
        let node_base = u32::try_from(view_index)
            .map_err(|_| NativeFrameTraceError::Capacity)?
            .checked_mul(1_000)
            .ok_or(NativeFrameTraceError::Capacity)?;
        for _ in 0..node_count {
            let source_node = reader.u32()?;
            let node = GraphNodeId(
                node_base
                    .checked_add(source_node)
                    .ok_or(NativeFrameTraceError::Capacity)?,
            );
            let queue = reader.u8()?;
            if reader.take(3)? != [0; 3] {
                return Err(NativeFrameTraceError::Invalid);
            }
            let started_nanos = reader.u64()?;
            let finished_nanos = reader.u64()?;
            let pipeline_hash = reader.u64()?;
            let draw_calls = reader.u32()?;
            let dispatch_calls = reader.u32()?;
            let label = format!("view-{view_index}/{}", reader.text()?);
            if node.0 == 0 || queue == 0 || finished_nanos < started_nanos || pipeline_hash == 0 {
                return Err(NativeFrameTraceError::Invalid);
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
        for _ in 0..attachment_count {
            let source_resource = reader.u32()?;
            let resource = GraphResourceId(
                node_base
                    .checked_add(source_resource)
                    .ok_or(NativeFrameTraceError::Capacity)?,
            );
            let version = reader.u32()?;
            let width = reader.u32()?;
            let height = reader.u32()?;
            let format = reader.u32()?;
            let row_bytes = reader.u32()?;
            let label = format!("view-{view_index}/{}", reader.text()?);
            let byte_len = reader.len(MAX_TRACE_BYTES)?;
            let attachment_bytes = reader.take(byte_len)?.to_vec();
            if resource.0 == 0
                || version == 0
                || width == 0
                || height == 0
                || format == 0
                || row_bytes == 0
            {
                return Err(NativeFrameTraceError::Invalid);
            }
            attachments.push(NativeCapturedAttachment {
                resource,
                version,
                label,
                width,
                height,
                format,
                row_bytes,
                bytes: attachment_bytes,
            });
        }
        for _ in 0..diagnostic_count {
            let pipeline_hash = reader.u64()?;
            let severity = reader.u8()?;
            if reader.take(3)? != [0; 3] {
                return Err(NativeFrameTraceError::Invalid);
            }
            let source_start = reader.u32()?;
            let source_end = reader.u32()?;
            let graph_node = GraphNodeId(
                node_base
                    .checked_add(reader.u32()?)
                    .ok_or(NativeFrameTraceError::Capacity)?,
            );
            let message = reader.text()?.to_owned();
            if pipeline_hash == 0
                || severity == 0
                || source_end < source_start
                || graph_node.0 == 0
                || message.is_empty()
            {
                return Err(NativeFrameTraceError::Invalid);
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
    }
    if reader.offset != bytes.len() {
        return Err(NativeFrameTraceError::Invalid);
    }
    Ok(NativeFrameTrace {
        frame_id,
        graph_signature,
        nodes,
        attachments,
        shader_diagnostics,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], NativeFrameTraceError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(NativeFrameTraceError::Capacity)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(NativeFrameTraceError::Invalid)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, NativeFrameTraceError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, NativeFrameTraceError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, NativeFrameTraceError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn handle(&mut self) -> Result<voplay_protocol::Handle, NativeFrameTraceError> {
        let handle = voplay_protocol::Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !handle.is_valid() {
            return Err(NativeFrameTraceError::Invalid);
        }
        Ok(handle)
    }

    fn len(&mut self, max: usize) -> Result<usize, NativeFrameTraceError> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(NativeFrameTraceError::Capacity);
        }
        Ok(len)
    }

    fn text(&mut self) -> Result<&'a str, NativeFrameTraceError> {
        let len = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        if len == 0 {
            return Err(NativeFrameTraceError::Invalid);
        }
        std::str::from_utf8(self.take(len)?).map_err(|_| NativeFrameTraceError::Invalid)
    }
}
