use std::collections::BTreeMap;

use voplay_import_gltf::{
    sample_animation_channel, GltfAnimationArtifact, GltfImportError, GltfImportPlan,
    GltfNodeTrsPlan, GltfSampledAnimationValue, GltfSkinArtifact,
};

#[derive(Clone, Debug, PartialEq)]
pub struct GltfSkinPose3d {
    pub skin: usize,
    pub mesh_node: usize,
    pub joint_matrices: Vec<[[f32; 4]; 4]>,
    pub morph_weights: BTreeMap<usize, Vec<f32>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfPoseError {
    InvalidPlan,
    MissingSkin,
    MissingMeshNode,
    AnimatedMatrixNode,
    JointCapacity,
    SingularMeshTransform,
    ArithmeticOverflow,
    Sample(GltfImportError),
}

impl From<GltfImportError> for GltfPoseError {
    fn from(value: GltfImportError) -> Self {
        Self::Sample(value)
    }
}

#[derive(Clone, Copy)]
struct PoseTrs {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

pub fn build_gltf_skin_pose_3d(
    plan: &GltfImportPlan,
    skins: &[GltfSkinArtifact],
    animation: Option<&GltfAnimationArtifact>,
    skin_index: usize,
    mesh_node: usize,
    time_seconds: f32,
    looping: bool,
) -> Result<GltfSkinPose3d, GltfPoseError> {
    if mesh_node >= plan.nodes.len() || !time_seconds.is_finite() {
        return Err(GltfPoseError::MissingMeshNode);
    }
    let skin = skins
        .iter()
        .find(|skin| skin.skin == skin_index)
        .ok_or(GltfPoseError::MissingSkin)?;
    if skin.joints.is_empty()
        || skin.joints.len() != skin.inverse_bind_matrices.len()
        || skin.joints.len() > 256
        || skin.joints.iter().any(|joint| *joint >= plan.nodes.len())
    {
        return Err(GltfPoseError::JointCapacity);
    }

    let mut animated_trs = plan
        .nodes
        .iter()
        .map(|node| node.rest_trs.as_ref().map(pose_trs))
        .collect::<Vec<_>>();
    let mut morph_weights = BTreeMap::new();
    if let Some(animation) = animation {
        for channel in &animation.channels {
            let sampled = sample_animation_channel(channel, time_seconds, looping)?;
            let target = animated_trs
                .get_mut(channel.node)
                .ok_or(GltfPoseError::InvalidPlan)?;
            match sampled {
                GltfSampledAnimationValue::Translation(value) => {
                    target
                        .as_mut()
                        .ok_or(GltfPoseError::AnimatedMatrixNode)?
                        .translation = value;
                }
                GltfSampledAnimationValue::Rotation(value) => {
                    target
                        .as_mut()
                        .ok_or(GltfPoseError::AnimatedMatrixNode)?
                        .rotation = normalize_quaternion(value)?;
                }
                GltfSampledAnimationValue::Scale(value) => {
                    target
                        .as_mut()
                        .ok_or(GltfPoseError::AnimatedMatrixNode)?
                        .scale = value;
                }
                GltfSampledAnimationValue::Weights(value) => {
                    morph_weights.insert(channel.node, value);
                }
            }
        }
    }

    let locals = plan
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            animated_trs[index]
                .map(trs_matrix)
                .unwrap_or_else(|| fixed_matrix(node.local_matrix_milli))
        })
        .collect::<Vec<_>>();
    let parents = build_parent_table(plan)?;
    let globals = build_global_matrices(&parents, &locals)?;
    let inverse_mesh =
        inverse_matrix(globals[mesh_node]).ok_or(GltfPoseError::SingularMeshTransform)?;
    let mut joint_matrices = Vec::with_capacity(skin.joints.len());
    for (joint, inverse_bind) in skin.joints.iter().zip(&skin.inverse_bind_matrices) {
        let inverse_bind = columns_to_rows(*inverse_bind);
        let palette = multiply(multiply(inverse_mesh, globals[*joint]), inverse_bind);
        if palette.iter().flatten().any(|value| !value.is_finite()) {
            return Err(GltfPoseError::ArithmeticOverflow);
        }
        joint_matrices.push(rows_to_columns(palette));
    }
    Ok(GltfSkinPose3d {
        skin: skin_index,
        mesh_node,
        joint_matrices,
        morph_weights,
    })
}

#[cfg(feature = "native-wgpu")]
pub fn upload_gltf_skin_pose_3d(
    renderer: &mut crate::WgpuSceneRenderer,
    entity: voplay_runtime::RenderEntity,
    pose: &GltfSkinPose3d,
) -> Result<(), crate::WgpuSceneRendererError> {
    renderer.upload_skin_palette(crate::SkinPaletteUpload3d {
        entity,
        joint_matrices: &pose.joint_matrices,
    })
}

fn pose_trs(value: &GltfNodeTrsPlan) -> PoseTrs {
    PoseTrs {
        translation: value.translation_milli.map(|value| value as f32 / 1_000.0),
        rotation: value
            .rotation_q30
            .map(|value| value as f32 / 1_000_000_000.0),
        scale: value.scale_milli.map(|value| value as f32 / 1_000.0),
    }
}

fn normalize_quaternion(mut value: [f32; 4]) -> Result<[f32; 4], GltfPoseError> {
    let length = value.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(GltfPoseError::ArithmeticOverflow);
    }
    value.iter_mut().for_each(|value| *value /= length);
    Ok(value)
}

fn trs_matrix(value: PoseTrs) -> [[f32; 4]; 4] {
    let [x, y, z, w] = value.rotation;
    let rotation = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut matrix = identity_matrix();
    for row in 0..3 {
        for column in 0..3 {
            matrix[row][column] = rotation[row][column] * value.scale[column];
        }
        matrix[row][3] = value.translation[row];
    }
    matrix
}

fn fixed_matrix(value: [i64; 16]) -> [[f32; 4]; 4] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| value[row * 4 + column] as f32 / 1_000.0)
    })
}

fn build_parent_table(plan: &GltfImportPlan) -> Result<Vec<Option<usize>>, GltfPoseError> {
    let mut parents = vec![None; plan.nodes.len()];
    for (parent, node) in plan.nodes.iter().enumerate() {
        for child in &node.children {
            let slot = parents.get_mut(*child).ok_or(GltfPoseError::InvalidPlan)?;
            if slot.replace(parent).is_some() {
                return Err(GltfPoseError::InvalidPlan);
            }
        }
    }
    Ok(parents)
}

fn build_global_matrices(
    parents: &[Option<usize>],
    locals: &[[[f32; 4]; 4]],
) -> Result<Vec<[[f32; 4]; 4]>, GltfPoseError> {
    let mut globals = vec![identity_matrix(); locals.len()];
    let mut states = vec![0_u8; locals.len()];
    for node in 0..locals.len() {
        evaluate_global(node, parents, locals, &mut globals, &mut states)?;
    }
    Ok(globals)
}

fn evaluate_global(
    node: usize,
    parents: &[Option<usize>],
    locals: &[[[f32; 4]; 4]],
    globals: &mut [[[f32; 4]; 4]],
    states: &mut [u8],
) -> Result<(), GltfPoseError> {
    match states.get(node).copied() {
        Some(2) => return Ok(()),
        Some(1) | None => return Err(GltfPoseError::InvalidPlan),
        Some(0) => {}
        Some(_) => return Err(GltfPoseError::InvalidPlan),
    }
    states[node] = 1;
    globals[node] = if let Some(parent) = parents[node] {
        evaluate_global(parent, parents, locals, globals, states)?;
        multiply(globals[parent], locals[node])
    } else {
        locals[node]
    };
    states[node] = 2;
    Ok(())
}

fn multiply(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            (0..4)
                .map(|index| left[row][index] * right[index][column])
                .sum()
        })
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

fn columns_to_rows(matrix: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    std::array::from_fn(|row| std::array::from_fn(|column| matrix[column][row]))
}

fn rows_to_columns(matrix: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    columns_to_rows(matrix)
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}
