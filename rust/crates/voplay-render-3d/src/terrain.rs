use crate::{Wire3dError, WireTerrainPatch3d};

#[derive(Clone, Copy, Debug)]
pub struct TerrainHeightmap3d<'a> {
    pub width: u32,
    pub height: u32,
    pub samples: &'a [u16],
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMeshArtifact3d {
    pub mesh: u64,
    pub model_milli: [i64; 12],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub texcoords: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

pub fn build_terrain_mesh_3d(
    patch: &WireTerrainPatch3d,
    heightmap: TerrainHeightmap3d<'_>,
) -> Result<TerrainMeshArtifact3d, Wire3dError> {
    let sample_count = (heightmap.width as usize)
        .checked_mul(heightmap.height as usize)
        .ok_or(Wire3dError::Capacity)?;
    if heightmap.width < 2
        || heightmap.height < 2
        || heightmap.samples.len() != sample_count
        || patch.size.contains(&0)
        || patch.height_scale_milli == 0
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    let step = 1_u32
        .checked_shl(patch.lod.min(30))
        .ok_or(Wire3dError::Capacity)?;
    let xs = sample_axis(heightmap.width, step);
    let zs = sample_axis(heightmap.height, step);
    let vertex_count = xs
        .len()
        .checked_mul(zs.len())
        .ok_or(Wire3dError::Capacity)?;
    let index_count = xs
        .len()
        .saturating_sub(1)
        .checked_mul(zs.len().saturating_sub(1))
        .and_then(|quads| quads.checked_mul(6))
        .ok_or(Wire3dError::Capacity)?;
    if vertex_count > u32::MAX as usize || index_count > u32::MAX as usize {
        return Err(Wire3dError::Capacity);
    }

    let width_units = patch.size[0] as f32 / 1_000.0;
    let depth_units = patch.size[1] as f32 / 1_000.0;
    let height_units = patch.height_scale_milli as f32 / 1_000.0;
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut texcoords = Vec::with_capacity(vertex_count);
    for &z in &zs {
        let v = z as f32 / (heightmap.height - 1) as f32;
        for &x in &xs {
            let u = x as f32 / (heightmap.width - 1) as f32;
            positions.push([
                u * width_units,
                sample_height(heightmap, x, z) * height_units,
                v * depth_units,
            ]);
            normals.push(terrain_normal(
                heightmap,
                x,
                z,
                width_units,
                depth_units,
                height_units,
            ));
            texcoords.push([u, v]);
        }
    }

    let row = xs.len() as u32;
    let mut indices = Vec::with_capacity(index_count);
    for z in 0..zs.len() - 1 {
        for x in 0..xs.len() - 1 {
            let top_left = z as u32 * row + x as u32;
            let top_right = top_left + 1;
            let bottom_left = top_left + row;
            let bottom_right = bottom_left + 1;
            indices.extend_from_slice(&[
                top_left,
                bottom_left,
                top_right,
                top_right,
                bottom_left,
                bottom_right,
            ]);
        }
    }
    let row = xs.len();
    let column = zs.len();
    let edges = [
        (0..row).map(|index| index as u32).collect::<Vec<_>>(),
        (0..column)
            .map(|index| (index * row + row - 1) as u32)
            .collect(),
        (0..row)
            .rev()
            .map(|index| ((column - 1) * row + index) as u32)
            .collect(),
        (0..column)
            .rev()
            .map(|index| (index * row) as u32)
            .collect(),
    ];
    let skirt_depth = (height_units * 0.05).max(0.01);
    for (edge, vertices) in edges.iter().enumerate() {
        if patch.neighbors[edge] > patch.lod {
            add_skirt(
                vertices,
                skirt_depth,
                &mut positions,
                &mut normals,
                &mut texcoords,
                &mut indices,
            )?;
        }
    }
    Ok(TerrainMeshArtifact3d {
        mesh: terrain_mesh_id(patch),
        model_milli: [
            1_000,
            0,
            0,
            patch.origin_milli[0],
            0,
            1_000,
            0,
            patch.origin_milli[1],
            0,
            0,
            1_000,
            patch.origin_milli[2],
        ],
        positions,
        normals,
        texcoords,
        indices,
    })
}

pub fn terrain_mesh_id(patch: &WireTerrainPatch3d) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in [
        patch.id,
        patch.heightmap,
        u64::from(patch.lod),
        u64::from(patch.size[0]),
        u64::from(patch.size[1]),
        u64::from(patch.height_scale_milli),
    ] {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    for neighbor in patch.neighbors {
        for byte in neighbor.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash.max(1)
}

fn add_skirt(
    edge: &[u32],
    depth: f32,
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    texcoords: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
) -> Result<(), Wire3dError> {
    let duplicate_start = u32::try_from(positions.len()).map_err(|_| Wire3dError::Capacity)?;
    for index in edge {
        let index = *index as usize;
        let mut position = positions[index];
        position[1] -= depth;
        positions.push(position);
        normals.push(normals[index]);
        texcoords.push(texcoords[index]);
    }
    for segment in 0..edge.len().saturating_sub(1) {
        let top_left = edge[segment];
        let top_right = edge[segment + 1];
        let bottom_left = duplicate_start + segment as u32;
        let bottom_right = bottom_left + 1;
        indices.extend_from_slice(&[
            top_left,
            bottom_left,
            top_right,
            top_right,
            bottom_left,
            bottom_right,
            top_right,
            bottom_left,
            top_left,
            bottom_right,
            bottom_left,
            top_right,
        ]);
    }
    Ok(())
}

fn sample_axis(length: u32, step: u32) -> Vec<u32> {
    let last = length - 1;
    let mut samples = (0..=last).step_by(step as usize).collect::<Vec<_>>();
    if samples.last().copied() != Some(last) {
        samples.push(last);
    }
    samples
}

fn sample_height(heightmap: TerrainHeightmap3d<'_>, x: u32, z: u32) -> f32 {
    let index = z as usize * heightmap.width as usize + x as usize;
    f32::from(heightmap.samples[index]) / f32::from(u16::MAX)
}

fn terrain_normal(
    heightmap: TerrainHeightmap3d<'_>,
    x: u32,
    z: u32,
    width_units: f32,
    depth_units: f32,
    height_units: f32,
) -> [f32; 3] {
    let left = x.saturating_sub(1);
    let right = (x + 1).min(heightmap.width - 1);
    let back = z.saturating_sub(1);
    let front = (z + 1).min(heightmap.height - 1);
    let dx = ((right - left) as f32 * width_units / (heightmap.width - 1) as f32).max(f32::EPSILON);
    let dz =
        ((front - back) as f32 * depth_units / (heightmap.height - 1) as f32).max(f32::EPSILON);
    let slope_x = (sample_height(heightmap, right, z) - sample_height(heightmap, left, z))
        * height_units
        / dx;
    let slope_z = (sample_height(heightmap, x, front) - sample_height(heightmap, x, back))
        * height_units
        / dz;
    normalize([-slope_x, 1.0, -slope_z])
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 1.0, 0.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}
