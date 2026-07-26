use crate::WgpuNativeFrameTrace3d;

const MAGIC: &[u8; 4] = b"VGT1";
const VERSION: u8 = 1;
pub const MAX_NATIVE_FRAME_TRACE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFrameTraceWireError {
    Invalid,
    Capacity,
}

pub fn encode_native_frame_trace_3d(
    trace: &WgpuNativeFrameTrace3d,
) -> Result<Vec<u8>, NativeFrameTraceWireError> {
    if trace.frame_id == 0
        || trace.graph_signature == 0
        || trace.views.is_empty()
        || trace.views.len() > u32::MAX as usize
    {
        return Err(NativeFrameTraceWireError::Invalid);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&trace.frame_id.to_le_bytes());
    bytes.extend_from_slice(&trace.graph_signature.to_le_bytes());
    put_len(&mut bytes, trace.views.len())?;
    for view in &trace.views {
        if !view.target.engine.is_valid()
            || !view.target.handle.is_valid()
            || view.viewport[2] == 0
            || view.viewport[3] == 0
            || view.trace.fence == 0
        {
            return Err(NativeFrameTraceWireError::Invalid);
        }
        put_handle(&mut bytes, view.target.engine);
        bytes.push(2);
        bytes.extend_from_slice(&[0; 3]);
        put_handle(&mut bytes, view.target.handle);
        for value in view.viewport {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&view.trace.fence.to_le_bytes());
        put_len(&mut bytes, view.trace.nodes.len())?;
        put_len(&mut bytes, view.trace.attachments.len())?;
        put_len(&mut bytes, view.trace.shader_diagnostics.len())?;
        for node in &view.trace.nodes {
            if node.node == 0
                || node.label.is_empty()
                || node.label.len() > u16::MAX as usize
                || node.queue == 0
                || node.finished_nanos < node.started_nanos
                || node.pipeline_hash == 0
            {
                return Err(NativeFrameTraceWireError::Invalid);
            }
            bytes.extend_from_slice(&node.node.to_le_bytes());
            bytes.push(node.queue);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&node.started_nanos.to_le_bytes());
            bytes.extend_from_slice(&node.finished_nanos.to_le_bytes());
            bytes.extend_from_slice(&node.pipeline_hash.to_le_bytes());
            bytes.extend_from_slice(&node.draw_calls.to_le_bytes());
            bytes.extend_from_slice(&node.dispatch_calls.to_le_bytes());
            put_text(&mut bytes, &node.label)?;
        }
        for attachment in &view.trace.attachments {
            if attachment.resource == 0
                || attachment.version == 0
                || attachment.label.is_empty()
                || attachment.label.len() > u16::MAX as usize
                || attachment.width == 0
                || attachment.height == 0
                || attachment.format == 0
                || attachment.row_bytes == 0
            {
                return Err(NativeFrameTraceWireError::Invalid);
            }
            bytes.extend_from_slice(&attachment.resource.to_le_bytes());
            bytes.extend_from_slice(&attachment.version.to_le_bytes());
            bytes.extend_from_slice(&attachment.width.to_le_bytes());
            bytes.extend_from_slice(&attachment.height.to_le_bytes());
            bytes.extend_from_slice(&attachment.format.to_le_bytes());
            bytes.extend_from_slice(&attachment.row_bytes.to_le_bytes());
            put_text(&mut bytes, &attachment.label)?;
            put_len(&mut bytes, attachment.bytes.len())?;
            bytes.extend_from_slice(&attachment.bytes);
        }
        for diagnostic in &view.trace.shader_diagnostics {
            if diagnostic.pipeline_hash == 0
                || diagnostic.severity == 0
                || diagnostic.source_end < diagnostic.source_start
                || diagnostic.graph_node == 0
                || diagnostic.message.is_empty()
                || diagnostic.message.len() > u16::MAX as usize
            {
                return Err(NativeFrameTraceWireError::Invalid);
            }
            bytes.extend_from_slice(&diagnostic.pipeline_hash.to_le_bytes());
            bytes.push(diagnostic.severity);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&diagnostic.source_start.to_le_bytes());
            bytes.extend_from_slice(&diagnostic.source_end.to_le_bytes());
            bytes.extend_from_slice(&diagnostic.graph_node.to_le_bytes());
            put_text(&mut bytes, &diagnostic.message)?;
        }
        if bytes.len() > MAX_NATIVE_FRAME_TRACE_BYTES {
            return Err(NativeFrameTraceWireError::Capacity);
        }
    }
    Ok(bytes)
}

fn put_handle(bytes: &mut Vec<u8>, handle: voplay_protocol::Handle) {
    bytes.extend_from_slice(&handle.index.to_le_bytes());
    bytes.extend_from_slice(&handle.generation.to_le_bytes());
}

fn put_len(bytes: &mut Vec<u8>, len: usize) -> Result<(), NativeFrameTraceWireError> {
    bytes.extend_from_slice(
        &u32::try_from(len)
            .map_err(|_| NativeFrameTraceWireError::Capacity)?
            .to_le_bytes(),
    );
    Ok(())
}

fn put_text(bytes: &mut Vec<u8>, value: &str) -> Result<(), NativeFrameTraceWireError> {
    bytes.extend_from_slice(
        &u16::try_from(value.len())
            .map_err(|_| NativeFrameTraceWireError::Capacity)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}
