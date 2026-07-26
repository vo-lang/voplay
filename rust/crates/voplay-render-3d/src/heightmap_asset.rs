const MAGIC: &[u8; 4] = b"VHM1";
const HEADER_BYTES: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeightmapArtifact3d {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub samples: Vec<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeightmapArtifact3dConfig {
    pub max_width: u32,
    pub max_height: u32,
    pub max_samples: usize,
    pub max_bytes: usize,
}

impl Default for HeightmapArtifact3dConfig {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_samples: 268_435_456,
            max_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeightmapArtifact3dError {
    InvalidConfig,
    InvalidHeightmap,
    Capacity,
    Truncated,
    TrailingBytes,
}

pub fn encode_heightmap_artifact_3d(
    artifact: &HeightmapArtifact3d,
    config: HeightmapArtifact3dConfig,
) -> Result<Vec<u8>, HeightmapArtifact3dError> {
    validate(artifact, config)?;
    let byte_len = HEADER_BYTES
        .checked_add(
            artifact
                .samples
                .len()
                .checked_mul(2)
                .ok_or(HeightmapArtifact3dError::Capacity)?,
        )
        .filter(|bytes| *bytes <= config.max_bytes)
        .ok_or(HeightmapArtifact3dError::Capacity)?;
    let mut bytes = Vec::with_capacity(byte_len);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&artifact.id.to_le_bytes());
    bytes.extend_from_slice(&artifact.width.to_le_bytes());
    bytes.extend_from_slice(&artifact.height.to_le_bytes());
    for sample in &artifact.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

pub fn decode_heightmap_artifact_3d(
    bytes: &[u8],
    config: HeightmapArtifact3dConfig,
) -> Result<HeightmapArtifact3d, HeightmapArtifact3dError> {
    validate_config(config)?;
    if bytes.len() < HEADER_BYTES || bytes.get(..4) != Some(MAGIC) {
        return Err(HeightmapArtifact3dError::Truncated);
    }
    if bytes.len() > config.max_bytes {
        return Err(HeightmapArtifact3dError::Capacity);
    }
    let id = read_u64(bytes, 4)?;
    let width = read_u32(bytes, 12)?;
    let height = read_u32(bytes, 16)?;
    let sample_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(HeightmapArtifact3dError::Capacity)?;
    let expected = HEADER_BYTES
        .checked_add(
            sample_count
                .checked_mul(2)
                .ok_or(HeightmapArtifact3dError::Capacity)?,
        )
        .ok_or(HeightmapArtifact3dError::Capacity)?;
    if bytes.len() != expected {
        return Err(if bytes.len() < expected {
            HeightmapArtifact3dError::Truncated
        } else {
            HeightmapArtifact3dError::TrailingBytes
        });
    }
    let mut samples = Vec::with_capacity(sample_count);
    for chunk in bytes[HEADER_BYTES..].chunks_exact(2) {
        samples.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    let artifact = HeightmapArtifact3d {
        id,
        width,
        height,
        samples,
    };
    validate(&artifact, config)?;
    Ok(artifact)
}

fn validate(
    artifact: &HeightmapArtifact3d,
    config: HeightmapArtifact3dConfig,
) -> Result<(), HeightmapArtifact3dError> {
    validate_config(config)?;
    let sample_count = (artifact.width as usize)
        .checked_mul(artifact.height as usize)
        .ok_or(HeightmapArtifact3dError::Capacity)?;
    if artifact.id == 0
        || artifact.width < 2
        || artifact.height < 2
        || artifact.width > config.max_width
        || artifact.height > config.max_height
        || sample_count > config.max_samples
        || artifact.samples.len() != sample_count
    {
        return Err(HeightmapArtifact3dError::InvalidHeightmap);
    }
    Ok(())
}

fn validate_config(config: HeightmapArtifact3dConfig) -> Result<(), HeightmapArtifact3dError> {
    if config.max_width < 2
        || config.max_height < 2
        || config.max_samples < 4
        || config.max_bytes < HEADER_BYTES + 8
    {
        return Err(HeightmapArtifact3dError::InvalidConfig);
    }
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, HeightmapArtifact3dError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(HeightmapArtifact3dError::Truncated)?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| HeightmapArtifact3dError::Truncated)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, HeightmapArtifact3dError> {
    bytes
        .get(offset..offset + 8)
        .ok_or(HeightmapArtifact3dError::Truncated)?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| HeightmapArtifact3dError::Truncated)
}
