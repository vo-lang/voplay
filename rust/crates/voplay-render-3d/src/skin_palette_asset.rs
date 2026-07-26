use voplay_protocol::{EngineId, Handle};
use voplay_runtime::RenderEntity;

const MAGIC: &[u8; 4] = b"VSP1";
const HEADER_BYTES: usize = 16;
const MATRIX_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct SkinPaletteArtifact3d {
    pub entity: RenderEntity,
    pub joint_matrices: Vec<[[f32; 4]; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkinPaletteArtifact3dConfig {
    pub max_joints: usize,
    pub max_bytes: usize,
}

impl Default for SkinPaletteArtifact3dConfig {
    fn default() -> Self {
        Self {
            max_joints: 256,
            max_bytes: HEADER_BYTES + 256 * MATRIX_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkinPaletteArtifact3dError {
    InvalidConfig,
    InvalidPalette,
    Capacity,
    Truncated,
    TrailingBytes,
}

pub fn encode_skin_palette_artifact_3d(
    artifact: &SkinPaletteArtifact3d,
    config: SkinPaletteArtifact3dConfig,
) -> Result<Vec<u8>, SkinPaletteArtifact3dError> {
    validate(artifact, config)?;
    let byte_len = HEADER_BYTES
        .checked_add(
            artifact
                .joint_matrices
                .len()
                .checked_mul(MATRIX_BYTES)
                .ok_or(SkinPaletteArtifact3dError::Capacity)?,
        )
        .filter(|length| *length <= config.max_bytes)
        .ok_or(SkinPaletteArtifact3dError::Capacity)?;
    let mut bytes = Vec::with_capacity(byte_len);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&artifact.entity.entity.index.to_le_bytes());
    bytes.extend_from_slice(&artifact.entity.entity.generation.to_le_bytes());
    bytes.extend_from_slice(
        &u16::try_from(artifact.joint_matrices.len())
            .map_err(|_| SkinPaletteArtifact3dError::Capacity)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    for matrix in &artifact.joint_matrices {
        for value in matrix.iter().flatten() {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    Ok(bytes)
}

pub fn decode_skin_palette_artifact_3d(
    bytes: &[u8],
    engine: EngineId,
    config: SkinPaletteArtifact3dConfig,
) -> Result<SkinPaletteArtifact3d, SkinPaletteArtifact3dError> {
    validate_config(config)?;
    if bytes.len() < HEADER_BYTES || bytes.len() > config.max_bytes || bytes.get(..4) != Some(MAGIC)
    {
        return Err(SkinPaletteArtifact3dError::Truncated);
    }
    let count = read_u16(bytes, 12)? as usize;
    if read_u16(bytes, 14)? != 0 || count == 0 || count > config.max_joints {
        return Err(SkinPaletteArtifact3dError::InvalidPalette);
    }
    let expected = HEADER_BYTES
        .checked_add(
            count
                .checked_mul(MATRIX_BYTES)
                .ok_or(SkinPaletteArtifact3dError::Capacity)?,
        )
        .ok_or(SkinPaletteArtifact3dError::Capacity)?;
    if bytes.len() != expected {
        return Err(if bytes.len() < expected {
            SkinPaletteArtifact3dError::Truncated
        } else {
            SkinPaletteArtifact3dError::TrailingBytes
        });
    }
    let entity = RenderEntity {
        engine,
        entity: Handle {
            index: read_u32(bytes, 4)?,
            generation: read_u32(bytes, 8)?,
        },
    };
    let mut offset = HEADER_BYTES;
    let mut joint_matrices = Vec::with_capacity(count);
    for _ in 0..count {
        let mut matrix = [[0.0; 4]; 4];
        for value in matrix.iter_mut().flatten() {
            *value = f32::from_bits(read_u32(bytes, offset)?);
            offset += 4;
        }
        joint_matrices.push(matrix);
    }
    let artifact = SkinPaletteArtifact3d {
        entity,
        joint_matrices,
    };
    validate(&artifact, config)?;
    Ok(artifact)
}

pub const fn packed_entity_handle(entity: RenderEntity) -> u64 {
    entity.entity.index as u64 | ((entity.entity.generation as u64) << 32)
}

fn validate(
    artifact: &SkinPaletteArtifact3d,
    config: SkinPaletteArtifact3dConfig,
) -> Result<(), SkinPaletteArtifact3dError> {
    validate_config(config)?;
    if !artifact.entity.engine.is_valid()
        || !artifact.entity.entity.is_valid()
        || artifact.joint_matrices.is_empty()
        || artifact.joint_matrices.len() > config.max_joints
        || artifact
            .joint_matrices
            .iter()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
    {
        return Err(SkinPaletteArtifact3dError::InvalidPalette);
    }
    Ok(())
}

fn validate_config(config: SkinPaletteArtifact3dConfig) -> Result<(), SkinPaletteArtifact3dError> {
    if config.max_joints == 0
        || config.max_joints > u16::MAX as usize
        || config.max_bytes < HEADER_BYTES + MATRIX_BYTES
    {
        return Err(SkinPaletteArtifact3dError::InvalidConfig);
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, SkinPaletteArtifact3dError> {
    bytes
        .get(offset..offset + 2)
        .ok_or(SkinPaletteArtifact3dError::Truncated)?
        .try_into()
        .map(u16::from_le_bytes)
        .map_err(|_| SkinPaletteArtifact3dError::Truncated)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SkinPaletteArtifact3dError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(SkinPaletteArtifact3dError::Truncated)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| SkinPaletteArtifact3dError::Truncated)
}
