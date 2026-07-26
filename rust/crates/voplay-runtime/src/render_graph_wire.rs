use crate::render_graph::{
    compile, CompiledRenderGraph, GraphNodeId, GraphNodeSpec, GraphQueue, GraphResourceDesc,
    GraphResourceId, RenderGraphConfig, RenderGraphError, ResourceLifetime, ResourceRead,
    ResourceUsage, ResourceWrite, TextureFormat,
};

const MAGIC: [u8; 4] = *b"VGR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 16;
const RESOURCE_BYTES: usize = 12;
const NODE_PREFIX_BYTES: usize = 32;
const ACCESS_BYTES: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderGraphWireError {
    Capacity,
    Malformed,
    UnsupportedVersion,
    Compile(RenderGraphError),
}

pub fn encode_render_graph_program(
    resources: &[GraphResourceDesc],
    nodes: &[GraphNodeSpec],
) -> Result<Vec<u8>, RenderGraphWireError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(resources.len())
            .map_err(|_| RenderGraphWireError::Capacity)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u32::try_from(nodes.len())
            .map_err(|_| RenderGraphWireError::Capacity)?
            .to_le_bytes(),
    );
    for resource in resources {
        bytes.extend_from_slice(&resource.id.0.to_le_bytes());
        bytes.push(texture_format_tag(resource.format));
        bytes.push(resource.sample_count);
        bytes.push(resource_lifetime_tag(resource.lifetime));
        bytes.push(0);
        bytes.extend_from_slice(&resource.allowed_usage.0.to_le_bytes());
    }
    for node in nodes {
        let label = node.label.as_bytes();
        bytes.extend_from_slice(&node.id.0.to_le_bytes());
        bytes.extend_from_slice(&node.stable_type.to_le_bytes());
        bytes.push(queue_tag(node.queue));
        bytes.extend_from_slice(&[0; 3]);
        for count in [
            label.len(),
            node.dependencies.len(),
            node.reads.len(),
            node.writes.len(),
        ] {
            bytes.extend_from_slice(
                &u32::try_from(count)
                    .map_err(|_| RenderGraphWireError::Capacity)?
                    .to_le_bytes(),
            );
        }
        bytes.extend_from_slice(label);
        for dependency in &node.dependencies {
            bytes.extend_from_slice(&dependency.0.to_le_bytes());
        }
        for read in &node.reads {
            bytes.extend_from_slice(&read.resource.0.to_le_bytes());
            bytes.extend_from_slice(&read.version.to_le_bytes());
            bytes.extend_from_slice(&read.usage.0.to_le_bytes());
        }
        for write in &node.writes {
            bytes.extend_from_slice(&write.resource.0.to_le_bytes());
            bytes.extend_from_slice(&write.base_version.to_le_bytes());
            bytes.extend_from_slice(&write.usage.0.to_le_bytes());
        }
    }
    Ok(bytes)
}

pub fn decode_render_graph_program(
    bytes: &[u8],
    config: RenderGraphConfig,
) -> Result<CompiledRenderGraph, RenderGraphWireError> {
    let (resources, nodes) = decode_render_graph_specs(bytes, config)?;
    compile(resources, nodes, config).map_err(RenderGraphWireError::Compile)
}

pub fn decode_render_graph_specs(
    bytes: &[u8],
    config: RenderGraphConfig,
) -> Result<(Vec<GraphResourceDesc>, Vec<GraphNodeSpec>), RenderGraphWireError> {
    if bytes.len() < HEADER_BYTES || bytes[0..4] != MAGIC {
        return Err(RenderGraphWireError::Malformed);
    }
    if read_u16(bytes, 4)? != VERSION {
        return Err(RenderGraphWireError::UnsupportedVersion);
    }
    if read_u16(bytes, 6)? != 0 {
        return Err(RenderGraphWireError::Malformed);
    }
    let resource_count = read_u32(bytes, 8)? as usize;
    let node_count = read_u32(bytes, 12)? as usize;
    if resource_count > config.max_resources || node_count > config.max_nodes {
        return Err(RenderGraphWireError::Capacity);
    }
    let resource_end = HEADER_BYTES
        .checked_add(
            resource_count
                .checked_mul(RESOURCE_BYTES)
                .ok_or(RenderGraphWireError::Capacity)?,
        )
        .filter(|end| *end <= bytes.len())
        .ok_or(RenderGraphWireError::Malformed)?;
    let mut resources = Vec::with_capacity(resource_count);
    let mut offset = HEADER_BYTES;
    for _ in 0..resource_count {
        resources.push(GraphResourceDesc {
            id: GraphResourceId(read_u32(bytes, offset)?),
            format: decode_texture_format(bytes[offset + 4])?,
            sample_count: bytes[offset + 5],
            lifetime: decode_resource_lifetime(bytes[offset + 6])?,
            allowed_usage: ResourceUsage(read_u32(bytes, offset + 8)?),
        });
        if bytes[offset + 7] != 0 {
            return Err(RenderGraphWireError::Malformed);
        }
        offset += RESOURCE_BYTES;
    }
    debug_assert_eq!(offset, resource_end);
    let mut nodes = Vec::with_capacity(node_count);
    let mut accesses = 0_usize;
    for _ in 0..node_count {
        let prefix_end = offset
            .checked_add(NODE_PREFIX_BYTES)
            .filter(|end| *end <= bytes.len())
            .ok_or(RenderGraphWireError::Malformed)?;
        let id = GraphNodeId(read_u32(bytes, offset)?);
        let stable_type = read_u64(bytes, offset + 4)?;
        let queue = decode_queue(bytes[offset + 12])?;
        if bytes[offset + 13..offset + 16] != [0; 3] {
            return Err(RenderGraphWireError::Malformed);
        }
        let label_len = read_u32(bytes, offset + 16)? as usize;
        let dependency_count = read_u32(bytes, offset + 20)? as usize;
        let read_count = read_u32(bytes, offset + 24)? as usize;
        let write_count = read_u32(bytes, offset + 28)? as usize;
        if label_len == 0 || label_len > config.max_label_bytes {
            return Err(RenderGraphWireError::Malformed);
        }
        accesses = accesses
            .checked_add(read_count)
            .and_then(|count| count.checked_add(write_count))
            .filter(|count| *count <= config.max_accesses)
            .ok_or(RenderGraphWireError::Capacity)?;
        offset = prefix_end;
        let label_end = offset
            .checked_add(label_len)
            .filter(|end| *end <= bytes.len())
            .ok_or(RenderGraphWireError::Malformed)?;
        let label = core::str::from_utf8(&bytes[offset..label_end])
            .map_err(|_| RenderGraphWireError::Malformed)?
            .to_owned();
        offset = label_end;
        let mut dependencies = Vec::with_capacity(dependency_count);
        for _ in 0..dependency_count {
            dependencies.push(GraphNodeId(read_u32(bytes, offset)?));
            offset = offset
                .checked_add(4)
                .filter(|end| *end <= bytes.len())
                .ok_or(RenderGraphWireError::Malformed)?;
        }
        let mut reads = Vec::with_capacity(read_count);
        for _ in 0..read_count {
            reads.push(ResourceRead {
                resource: GraphResourceId(read_u32(bytes, offset)?),
                version: read_u32(bytes, offset + 4)?,
                usage: ResourceUsage(read_u32(bytes, offset + 8)?),
            });
            offset = offset
                .checked_add(ACCESS_BYTES)
                .filter(|end| *end <= bytes.len())
                .ok_or(RenderGraphWireError::Malformed)?;
        }
        let mut writes = Vec::with_capacity(write_count);
        for _ in 0..write_count {
            writes.push(ResourceWrite {
                resource: GraphResourceId(read_u32(bytes, offset)?),
                base_version: read_u32(bytes, offset + 4)?,
                usage: ResourceUsage(read_u32(bytes, offset + 8)?),
            });
            offset = offset
                .checked_add(ACCESS_BYTES)
                .filter(|end| *end <= bytes.len())
                .ok_or(RenderGraphWireError::Malformed)?;
        }
        nodes.push(GraphNodeSpec {
            id,
            stable_type,
            label,
            queue,
            dependencies,
            reads,
            writes,
        });
    }
    if offset != bytes.len() {
        return Err(RenderGraphWireError::Malformed);
    }
    Ok((resources, nodes))
}

const fn texture_format_tag(format: TextureFormat) -> u8 {
    match format {
        TextureFormat::Rgba8 => 1,
        TextureFormat::Rgba16Float => 2,
        TextureFormat::Depth24 => 3,
        TextureFormat::Depth32Float => 4,
    }
}

fn decode_texture_format(tag: u8) -> Result<TextureFormat, RenderGraphWireError> {
    match tag {
        1 => Ok(TextureFormat::Rgba8),
        2 => Ok(TextureFormat::Rgba16Float),
        3 => Ok(TextureFormat::Depth24),
        4 => Ok(TextureFormat::Depth32Float),
        _ => Err(RenderGraphWireError::Malformed),
    }
}

const fn resource_lifetime_tag(lifetime: ResourceLifetime) -> u8 {
    match lifetime {
        ResourceLifetime::Transient => 1,
        ResourceLifetime::Persistent => 2,
        ResourceLifetime::ExternalSurface => 3,
    }
}

fn decode_resource_lifetime(tag: u8) -> Result<ResourceLifetime, RenderGraphWireError> {
    match tag {
        1 => Ok(ResourceLifetime::Transient),
        2 => Ok(ResourceLifetime::Persistent),
        3 => Ok(ResourceLifetime::ExternalSurface),
        _ => Err(RenderGraphWireError::Malformed),
    }
}

const fn queue_tag(queue: GraphQueue) -> u8 {
    match queue {
        GraphQueue::Graphics => 1,
        GraphQueue::Compute => 2,
        GraphQueue::Copy => 3,
    }
}

fn decode_queue(tag: u8) -> Result<GraphQueue, RenderGraphWireError> {
    match tag {
        1 => Ok(GraphQueue::Graphics),
        2 => Ok(GraphQueue::Compute),
        3 => Ok(GraphQueue::Copy),
        _ => Err(RenderGraphWireError::Malformed),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, RenderGraphWireError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(RenderGraphWireError::Malformed)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, RenderGraphWireError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(RenderGraphWireError::Malformed)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, RenderGraphWireError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(RenderGraphWireError::Malformed)
}
