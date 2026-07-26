use crate::MeshDescriptor;

const MAGIC: &[u8; 4] = b"VMG1";

#[derive(Clone, Debug, PartialEq)]
pub struct MeshArtifact3d {
    pub descriptor: MeshDescriptor,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub texcoords: Vec<[f32; 2]>,
    pub joints: Option<Vec<[u16; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshArtifact3dConfig {
    pub max_vertices: usize,
    pub max_indices: usize,
    pub max_bytes: usize,
}

impl Default for MeshArtifact3dConfig {
    fn default() -> Self {
        Self {
            max_vertices: 16_777_216,
            max_indices: 50_331_648,
            max_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshArtifact3dError {
    InvalidConfig,
    InvalidMesh,
    Capacity,
    Truncated,
    TrailingBytes,
}

pub fn encode_mesh_artifact_3d(
    artifact: &MeshArtifact3d,
    config: MeshArtifact3dConfig,
) -> Result<Vec<u8>, MeshArtifact3dError> {
    validate(artifact, config)?;
    let vertex_stride = if artifact.descriptor.skinned { 56 } else { 32 };
    let capacity = 21_usize
        .checked_add(
            artifact
                .positions
                .len()
                .checked_mul(vertex_stride)
                .ok_or(MeshArtifact3dError::Capacity)?,
        )
        .and_then(|bytes| bytes.checked_add(artifact.indices.len().checked_mul(4)?))
        .filter(|bytes| *bytes <= config.max_bytes)
        .ok_or(MeshArtifact3dError::Capacity)?;
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&artifact.descriptor.id.to_le_bytes());
    bytes.extend_from_slice(&artifact.descriptor.vertex_count.to_le_bytes());
    bytes.extend_from_slice(&artifact.descriptor.index_count.to_le_bytes());
    bytes.push(u8::from(artifact.descriptor.skinned));
    for index in 0..artifact.positions.len() {
        for value in artifact.positions[index]
            .iter()
            .chain(artifact.normals[index].iter())
            .chain(artifact.texcoords[index].iter())
        {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        if let (Some(joints), Some(weights)) = (&artifact.joints, &artifact.weights) {
            for joint in joints[index] {
                bytes.extend_from_slice(&joint.to_le_bytes());
            }
            for weight in weights[index] {
                bytes.extend_from_slice(&weight.to_bits().to_le_bytes());
            }
        }
    }
    for index in &artifact.indices {
        bytes.extend_from_slice(&index.to_le_bytes());
    }
    Ok(bytes)
}

pub fn decode_mesh_artifact_3d(
    bytes: &[u8],
    config: MeshArtifact3dConfig,
) -> Result<MeshArtifact3d, MeshArtifact3dError> {
    if config.max_vertices == 0 || config.max_indices == 0 || config.max_bytes < 21 {
        return Err(MeshArtifact3dError::InvalidConfig);
    }
    if bytes.len() < 21 || bytes.len() > config.max_bytes || bytes.get(..4) != Some(MAGIC) {
        return Err(MeshArtifact3dError::Truncated);
    }
    let descriptor = MeshDescriptor {
        id: read_u64(bytes, 4)?,
        vertex_count: read_u32(bytes, 12)?,
        index_count: read_u32(bytes, 16)?,
        skinned: match bytes[20] {
            0 => false,
            1 => true,
            _ => return Err(MeshArtifact3dError::InvalidMesh),
        },
    };
    let vertices = descriptor.vertex_count as usize;
    let indices = descriptor.index_count as usize;
    if vertices > config.max_vertices || indices > config.max_indices {
        return Err(MeshArtifact3dError::Capacity);
    }
    let stride = if descriptor.skinned { 56 } else { 32 };
    let expected = 21_usize
        .checked_add(
            vertices
                .checked_mul(stride)
                .ok_or(MeshArtifact3dError::Capacity)?,
        )
        .and_then(|size| size.checked_add(indices.checked_mul(4)?))
        .ok_or(MeshArtifact3dError::Capacity)?;
    if bytes.len() != expected {
        return Err(if bytes.len() < expected {
            MeshArtifact3dError::Truncated
        } else {
            MeshArtifact3dError::TrailingBytes
        });
    }
    let mut offset = 21;
    let mut positions = Vec::with_capacity(vertices);
    let mut normals = Vec::with_capacity(vertices);
    let mut texcoords = Vec::with_capacity(vertices);
    let mut joints = descriptor.skinned.then(|| Vec::with_capacity(vertices));
    let mut weights = descriptor.skinned.then(|| Vec::with_capacity(vertices));
    for _ in 0..vertices {
        positions.push(read_f32_array(bytes, &mut offset)?);
        normals.push(read_f32_array(bytes, &mut offset)?);
        texcoords.push(read_f32_array(bytes, &mut offset)?);
        if let (Some(joints), Some(weights)) = (&mut joints, &mut weights) {
            joints.push([
                read_u16_advance(bytes, &mut offset)?,
                read_u16_advance(bytes, &mut offset)?,
                read_u16_advance(bytes, &mut offset)?,
                read_u16_advance(bytes, &mut offset)?,
            ]);
            weights.push(read_f32_array(bytes, &mut offset)?);
        }
    }
    let mut decoded_indices = Vec::with_capacity(indices);
    for _ in 0..indices {
        decoded_indices.push(read_u32_advance(bytes, &mut offset)?);
    }
    if offset != bytes.len() {
        return Err(MeshArtifact3dError::TrailingBytes);
    }
    let artifact = MeshArtifact3d {
        descriptor,
        positions,
        normals,
        texcoords,
        joints,
        weights,
        indices: decoded_indices,
    };
    validate(&artifact, config)?;
    Ok(artifact)
}

fn validate(
    artifact: &MeshArtifact3d,
    config: MeshArtifact3dConfig,
) -> Result<(), MeshArtifact3dError> {
    if config.max_vertices == 0
        || config.max_indices == 0
        || config.max_bytes < 21
        || artifact.descriptor.id == 0
        || artifact.positions.is_empty()
        || artifact.positions.len() > config.max_vertices
        || artifact.positions.len() != artifact.normals.len()
        || artifact.positions.len() != artifact.texcoords.len()
        || artifact.descriptor.vertex_count as usize != artifact.positions.len()
        || artifact.indices.is_empty()
        || artifact.indices.len() > config.max_indices
        || artifact.descriptor.index_count as usize != artifact.indices.len()
        || artifact.descriptor.skinned != artifact.joints.is_some()
        || artifact.descriptor.skinned != artifact.weights.is_some()
        || artifact
            .joints
            .as_ref()
            .is_some_and(|joints| joints.len() != artifact.positions.len())
        || artifact
            .weights
            .as_ref()
            .is_some_and(|weights| weights.len() != artifact.positions.len())
        || artifact
            .positions
            .iter()
            .flatten()
            .chain(artifact.normals.iter().flatten())
            .chain(artifact.texcoords.iter().flatten())
            .any(|value| !value.is_finite())
        || artifact
            .indices
            .iter()
            .any(|index| *index as usize >= artifact.positions.len())
        || artifact.weights.as_ref().is_some_and(|weights| {
            weights.iter().any(|weights| {
                !weights
                    .iter()
                    .all(|weight| weight.is_finite() && *weight >= 0.0)
                    || weights.iter().sum::<f32>() <= f32::EPSILON
            })
        })
    {
        return Err(MeshArtifact3dError::InvalidMesh);
    }
    Ok(())
}

fn read_u16_advance(bytes: &[u8], offset: &mut usize) -> Result<u16, MeshArtifact3dError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(MeshArtifact3dError::Truncated)?;
    *offset += 2;
    Ok(u16::from_le_bytes(value.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, MeshArtifact3dError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(MeshArtifact3dError::Truncated)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| MeshArtifact3dError::Truncated)
}

fn read_u32_advance(bytes: &[u8], offset: &mut usize) -> Result<u32, MeshArtifact3dError> {
    let value = read_u32(bytes, *offset)?;
    *offset += 4;
    Ok(value)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, MeshArtifact3dError> {
    bytes
        .get(offset..offset + 8)
        .ok_or(MeshArtifact3dError::Truncated)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| MeshArtifact3dError::Truncated)
}

fn read_f32_array<const N: usize>(
    bytes: &[u8],
    offset: &mut usize,
) -> Result<[f32; N], MeshArtifact3dError> {
    let mut values = [0.0; N];
    for value in &mut values {
        *value = f32::from_bits(read_u32_advance(bytes, offset)?);
    }
    Ok(values)
}
