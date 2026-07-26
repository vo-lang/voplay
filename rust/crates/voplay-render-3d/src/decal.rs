use crate::{Wire3dError, WireDecal3d};

#[derive(Clone, Copy, Debug)]
pub struct DecalTargetMesh3d<'a> {
    pub id: u64,
    pub model_milli: [i64; 12],
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub indices: &'a [u32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedDecalMesh3d {
    pub mesh: u64,
    pub model_milli: [i64; 12],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub texcoords: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy)]
struct ClipVertex {
    world: [f32; 3],
    normal: [f32; 3],
    local: [f32; 3],
}

pub fn project_decal_mesh_3d(
    decal: &WireDecal3d,
    target: DecalTargetMesh3d<'_>,
) -> Result<ProjectedDecalMesh3d, Wire3dError> {
    if target.id == 0
        || target.positions.is_empty()
        || target.positions.len() != target.normals.len()
        || target.indices.len() % 3 != 0
        || target
            .indices
            .iter()
            .any(|index| *index as usize >= target.positions.len())
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    let target_matrix = affine_matrix(target.model_milli);
    let decal_matrix = matrix4_milli(decal.transform_milli);
    let inverse_decal = inverse_matrix(decal_matrix).ok_or(Wire3dError::InvalidDescriptor)?;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut texcoords = Vec::new();
    let mut indices = Vec::new();
    for triangle in target.indices.chunks_exact(3) {
        let mut polygon = triangle
            .iter()
            .map(|index| {
                let index = *index as usize;
                let world = transform_point(target_matrix, target.positions[index]);
                ClipVertex {
                    world,
                    normal: normalize(transform_vector(target_matrix, target.normals[index])),
                    local: transform_point(inverse_decal, world),
                }
            })
            .collect::<Vec<_>>();
        for (axis, sign) in [
            (0, 1.0),
            (0, -1.0),
            (1, 1.0),
            (1, -1.0),
            (2, 1.0),
            (2, -1.0),
        ] {
            polygon = clip_polygon(&polygon, axis, sign);
            if polygon.len() < 3 {
                break;
            }
        }
        if polygon.len() < 3 {
            continue;
        }
        let base = u32::try_from(positions.len()).map_err(|_| Wire3dError::Capacity)?;
        for vertex in &polygon {
            positions.push(vertex.world);
            normals.push(normalize(vertex.normal));
            texcoords.push([vertex.local[0] + 0.5, 0.5 - vertex.local[2]]);
        }
        for index in 1..polygon.len() - 1 {
            indices.extend_from_slice(&[base, base + index as u32, base + index as u32 + 1]);
        }
    }
    Ok(ProjectedDecalMesh3d {
        mesh: projected_decal_mesh_id(decal, target.id),
        model_milli: identity_affine_milli(),
        positions,
        normals,
        texcoords,
        indices,
    })
}

pub fn projected_decal_mesh_id(decal: &WireDecal3d, target: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in [decal.id, target, decal.material] {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    for value in decal.transform_milli {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash.max(1)
}

fn clip_polygon(vertices: &[ClipVertex], axis: usize, sign: f32) -> Vec<ClipVertex> {
    let mut output = Vec::new();
    let Some(mut previous) = vertices.last().copied() else {
        return output;
    };
    let mut previous_distance = 0.5 - sign * previous.local[axis];
    for current in vertices.iter().copied() {
        let current_distance = 0.5 - sign * current.local[axis];
        let previous_inside = previous_distance >= 0.0;
        let current_inside = current_distance >= 0.0;
        if previous_inside != current_inside {
            let denominator = previous_distance - current_distance;
            let amount = if denominator.abs() <= f32::EPSILON {
                0.0
            } else {
                (previous_distance / denominator).clamp(0.0, 1.0)
            };
            output.push(interpolate(previous, current, amount));
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_distance = current_distance;
    }
    output
}

fn interpolate(left: ClipVertex, right: ClipVertex, amount: f32) -> ClipVertex {
    ClipVertex {
        world: mix3(left.world, right.world, amount),
        normal: mix3(left.normal, right.normal, amount),
        local: mix3(left.local, right.local, amount),
    }
}

fn mix3(left: [f32; 3], right: [f32; 3], amount: f32) -> [f32; 3] {
    [
        left[0] + (right[0] - left[0]) * amount,
        left[1] + (right[1] - left[1]) * amount,
        left[2] + (right[2] - left[2]) * amount,
    ]
}

fn affine_matrix(value: [i64; 12]) -> [[f32; 4]; 4] {
    let scale = |value: i64| value as f32 / 1_000.0;
    [
        [
            scale(value[0]),
            scale(value[1]),
            scale(value[2]),
            scale(value[3]),
        ],
        [
            scale(value[4]),
            scale(value[5]),
            scale(value[6]),
            scale(value[7]),
        ],
        [
            scale(value[8]),
            scale(value[9]),
            scale(value[10]),
            scale(value[11]),
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn matrix4_milli(value: [i64; 16]) -> [[f32; 4]; 4] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| value[row * 4 + column] as f32 / 1_000.0)
    })
}

fn transform_point(matrix: [[f32; 4]; 4], value: [f32; 3]) -> [f32; 3] {
    let result = transform(matrix, [value[0], value[1], value[2], 1.0]);
    if result[3].abs() <= f32::EPSILON {
        [result[0], result[1], result[2]]
    } else {
        [
            result[0] / result[3],
            result[1] / result[3],
            result[2] / result[3],
        ]
    }
}

fn transform_vector(matrix: [[f32; 4]; 4], value: [f32; 3]) -> [f32; 3] {
    let result = transform(matrix, [value[0], value[1], value[2], 0.0]);
    [result[0], result[1], result[2]]
}

fn transform(matrix: [[f32; 4]; 4], value: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|row| {
        matrix[row]
            .iter()
            .zip(value)
            .map(|(left, right)| left * right)
            .sum()
    })
}

fn inverse_matrix(matrix: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    let mut augmented = [[0.0_f32; 8]; 4];
    for row in 0..4 {
        augmented[row][..4].copy_from_slice(&matrix[row]);
        augmented[row][row + 4] = 1.0;
    }
    for column in 0..4 {
        let pivot = (column..4).max_by(|left, right| {
            augmented[*left][column]
                .abs()
                .total_cmp(&augmented[*right][column].abs())
        })?;
        if augmented[pivot][column].abs() <= 0.000001 {
            return None;
        }
        augmented.swap(column, pivot);
        let divisor = augmented[column][column];
        for value in &mut augmented[column] {
            *value /= divisor;
        }
        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = augmented[row][column];
            for index in 0..8 {
                augmented[row][index] -= factor * augmented[column][index];
            }
        }
    }
    Some(std::array::from_fn(|row| {
        std::array::from_fn(|column| augmented[row][column + 4])
    }))
}

fn identity_affine_milli() -> [i64; 12] {
    [1_000, 0, 0, 0, 0, 1_000, 0, 0, 0, 0, 1_000, 0]
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 1.0, 0.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}
