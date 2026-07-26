use std::{collections::BTreeMap, sync::Arc};

use voplay_import_gltf::{
    sample_animation_channel, GltfAnimationArtifact, GltfImportPlan, GltfSampledAnimationValue,
    GltfSkinArtifact,
};
use voplay_runtime::{
    animation_presentation::{
        BoneTransform, PoseProviderError, PoseProviderState, PoseSamplingProvider,
    },
    asset::AssetRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GltfPoseProviderConfig {
    pub ticks_per_second: u32,
    pub max_skeletons: usize,
    pub max_clips: usize,
    pub max_nodes_per_skeleton: usize,
    pub max_bones_per_skeleton: usize,
}

impl Default for GltfPoseProviderConfig {
    fn default() -> Self {
        Self {
            ticks_per_second: 60,
            max_skeletons: 65_536,
            max_clips: 65_536,
            max_nodes_per_skeleton: 65_536,
            max_bones_per_skeleton: 1_024,
        }
    }
}

#[derive(Clone)]
struct SkeletonRecord {
    plan: Arc<GltfImportPlan>,
    skin: GltfSkinArtifact,
    mesh_node: usize,
}

pub struct GltfPoseProvider {
    engine: voplay_protocol::EngineId,
    endpoint_generation: voplay_protocol::Handle,
    config: GltfPoseProviderConfig,
    state: PoseProviderState,
    skeletons: BTreeMap<AssetRef, SkeletonRecord>,
    clips: BTreeMap<AssetRef, GltfAnimationArtifact>,
}

impl GltfPoseProvider {
    pub fn new(
        engine: voplay_protocol::EngineId,
        endpoint_generation: voplay_protocol::Handle,
        config: GltfPoseProviderConfig,
    ) -> Result<Self, PoseProviderError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.ticks_per_second == 0
            || config.max_skeletons == 0
            || config.max_clips == 0
            || config.max_nodes_per_skeleton == 0
            || config.max_bones_per_skeleton == 0
        {
            return Err(PoseProviderError::Capacity);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            state: PoseProviderState::Ready,
            skeletons: BTreeMap::new(),
            clips: BTreeMap::new(),
        })
    }

    pub fn register_skeleton(
        &mut self,
        asset: AssetRef,
        plan: Arc<GltfImportPlan>,
        skin: GltfSkinArtifact,
        mesh_node: usize,
    ) -> Result<(), PoseProviderError> {
        if self.state != PoseProviderState::Ready
            || asset.engine != self.engine
            || !asset.handle.is_valid()
            || plan.nodes.len() > self.config.max_nodes_per_skeleton
            || skin.joints.is_empty()
            || skin.joints.len() > self.config.max_bones_per_skeleton
            || skin.joints.len() != skin.inverse_bind_matrices.len()
            || skin.joints.iter().any(|joint| *joint >= plan.nodes.len())
            || plan
                .nodes
                .get(mesh_node)
                .is_none_or(|node| node.skin != Some(skin.skin))
            || self.skeletons.contains_key(&asset)
        {
            return Err(PoseProviderError::InvalidArtifact);
        }
        if self.skeletons.len() == self.config.max_skeletons {
            return Err(PoseProviderError::Capacity);
        }
        build_parent_table(&plan)?;
        self.skeletons.insert(
            asset,
            SkeletonRecord {
                plan,
                skin,
                mesh_node,
            },
        );
        Ok(())
    }

    pub fn register_clip(
        &mut self,
        asset: AssetRef,
        clip: GltfAnimationArtifact,
    ) -> Result<(), PoseProviderError> {
        if self.state != PoseProviderState::Ready
            || asset.engine != self.engine
            || !asset.handle.is_valid()
            || clip.channels.is_empty()
            || !clip.duration_seconds.is_finite()
            || clip.duration_seconds < 0.0
            || self.clips.contains_key(&asset)
        {
            return Err(PoseProviderError::InvalidArtifact);
        }
        if self.clips.len() == self.config.max_clips {
            return Err(PoseProviderError::Capacity);
        }
        self.clips.insert(asset, clip);
        Ok(())
    }

    pub fn remove_skeleton(&mut self, asset: AssetRef) -> bool {
        self.skeletons.remove(&asset).is_some()
    }

    pub fn remove_clip(&mut self, asset: AssetRef) -> bool {
        self.clips.remove(&asset).is_some()
    }

    pub fn begin_close(&mut self) {
        if self.state == PoseProviderState::Ready {
            self.state = PoseProviderState::Closing;
        }
    }

    pub fn close(&mut self) {
        self.state = PoseProviderState::Closed;
        self.skeletons.clear();
        self.clips.clear();
    }

    fn sample(
        &self,
        skeleton: &SkeletonRecord,
        clip: &GltfAnimationArtifact,
        tick: u64,
    ) -> Result<Vec<BoneTransform>, PoseProviderError> {
        let time = tick as f64 / f64::from(self.config.ticks_per_second);
        if !time.is_finite() || time > f64::from(f32::MAX) {
            return Err(PoseProviderError::DecodeFailed);
        }
        let mut locals = skeleton
            .plan
            .nodes
            .iter()
            .map(|node| fixed_matrix(node.local_matrix_milli))
            .collect::<Vec<_>>();
        let mut sampled_trs = skeleton
            .plan
            .nodes
            .iter()
            .map(|node| node.rest_trs.as_ref().map(rest_trs))
            .collect::<Vec<_>>();
        for channel in &clip.channels {
            let sampled = sample_animation_channel(channel, time as f32, true)
                .map_err(|_| PoseProviderError::DecodeFailed)?;
            let trs = sampled_trs
                .get_mut(channel.node)
                .ok_or(PoseProviderError::InvalidArtifact)?;
            match sampled {
                GltfSampledAnimationValue::Translation(value) => {
                    trs.as_mut()
                        .ok_or(PoseProviderError::InvalidArtifact)?
                        .translation = value;
                }
                GltfSampledAnimationValue::Rotation(value) => {
                    trs.as_mut()
                        .ok_or(PoseProviderError::InvalidArtifact)?
                        .rotation = normalize_quaternion(value)?;
                }
                GltfSampledAnimationValue::Scale(value) => {
                    trs.as_mut()
                        .ok_or(PoseProviderError::InvalidArtifact)?
                        .scale = value;
                }
                GltfSampledAnimationValue::Weights(_) => {}
            }
        }
        for (index, trs) in sampled_trs.into_iter().enumerate() {
            if let Some(trs) = trs {
                locals[index] = trs_matrix(trs);
            }
        }
        let parents = build_parent_table(&skeleton.plan)?;
        let globals = build_globals(&parents, &locals)?;
        let inverse_mesh =
            inverse_matrix(globals[skeleton.mesh_node]).ok_or(PoseProviderError::DecodeFailed)?;
        let joint_lookup = skeleton
            .skin
            .joints
            .iter()
            .enumerate()
            .map(|(bone, node)| (*node, bone))
            .collect::<BTreeMap<_, _>>();
        skeleton
            .skin
            .joints
            .iter()
            .map(|node| {
                let parent_joint = nearest_joint_parent(*node, &parents, &joint_lookup);
                let relative = if let Some(parent_joint) = parent_joint {
                    let parent_node = skeleton.skin.joints[parent_joint];
                    let inverse = inverse_matrix(globals[parent_node])
                        .ok_or(PoseProviderError::DecodeFailed)?;
                    multiply(inverse, globals[*node])
                } else {
                    multiply(inverse_mesh, globals[*node])
                };
                decompose_bone(relative)
            })
            .collect()
    }
}

impl PoseSamplingProvider for GltfPoseProvider {
    fn engine(&self) -> voplay_protocol::EngineId {
        self.engine
    }

    fn endpoint_generation(&self) -> voplay_protocol::Handle {
        self.endpoint_generation
    }

    fn state(&self) -> PoseProviderState {
        self.state
    }

    fn sample_pose(
        &self,
        skeleton: AssetRef,
        clip: AssetRef,
        tick: u64,
        bone_count: u32,
    ) -> Result<Vec<BoneTransform>, PoseProviderError> {
        if self.state != PoseProviderState::Ready {
            return Err(PoseProviderError::MissingArtifact);
        }
        let skeleton = self
            .skeletons
            .get(&skeleton)
            .ok_or(PoseProviderError::MissingArtifact)?;
        let clip = self
            .clips
            .get(&clip)
            .ok_or(PoseProviderError::MissingArtifact)?;
        if skeleton.skin.joints.len() != bone_count as usize {
            return Err(PoseProviderError::InvalidArtifact);
        }
        self.sample(skeleton, clip, tick)
    }
}

#[derive(Clone, Copy)]
struct Trs {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

fn rest_trs(value: &voplay_import_gltf::GltfNodeTrsPlan) -> Trs {
    Trs {
        translation: value.translation_milli.map(|value| value as f32 / 1_000.0),
        rotation: value
            .rotation_q30
            .map(|value| value as f32 / 1_000_000_000.0),
        scale: value.scale_milli.map(|value| value as f32 / 1_000.0),
    }
}

fn normalize_quaternion(mut value: [f32; 4]) -> Result<[f32; 4], PoseProviderError> {
    let length = value.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(PoseProviderError::DecodeFailed);
    }
    value.iter_mut().for_each(|value| *value /= length);
    Ok(value)
}

fn trs_matrix(value: Trs) -> [[f32; 4]; 4] {
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

fn build_parent_table(plan: &GltfImportPlan) -> Result<Vec<Option<usize>>, PoseProviderError> {
    let mut parents = vec![None; plan.nodes.len()];
    for (parent, node) in plan.nodes.iter().enumerate() {
        for child in &node.children {
            let slot = parents
                .get_mut(*child)
                .ok_or(PoseProviderError::InvalidArtifact)?;
            if slot.replace(parent).is_some() {
                return Err(PoseProviderError::InvalidArtifact);
            }
        }
    }
    Ok(parents)
}

fn build_globals(
    parents: &[Option<usize>],
    locals: &[[[f32; 4]; 4]],
) -> Result<Vec<[[f32; 4]; 4]>, PoseProviderError> {
    let mut globals = vec![identity_matrix(); locals.len()];
    let mut state = vec![0_u8; locals.len()];
    for node in 0..locals.len() {
        evaluate_global(node, parents, locals, &mut globals, &mut state)?;
    }
    Ok(globals)
}

fn evaluate_global(
    node: usize,
    parents: &[Option<usize>],
    locals: &[[[f32; 4]; 4]],
    globals: &mut [[[f32; 4]; 4]],
    state: &mut [u8],
) -> Result<(), PoseProviderError> {
    match state.get(node).copied() {
        Some(2) => return Ok(()),
        Some(0) => {}
        _ => return Err(PoseProviderError::InvalidArtifact),
    }
    state[node] = 1;
    globals[node] = if let Some(parent) = parents[node] {
        evaluate_global(parent, parents, locals, globals, state)?;
        multiply(globals[parent], locals[node])
    } else {
        locals[node]
    };
    state[node] = 2;
    Ok(())
}

fn nearest_joint_parent(
    node: usize,
    parents: &[Option<usize>],
    joints: &BTreeMap<usize, usize>,
) -> Option<usize> {
    let mut parent = parents[node];
    while let Some(node) = parent {
        if let Some(joint) = joints.get(&node) {
            return Some(*joint);
        }
        parent = parents[node];
    }
    None
}

fn decompose_bone(matrix: [[f32; 4]; 4]) -> Result<BoneTransform, PoseProviderError> {
    let translation = [matrix[0][3], matrix[1][3], matrix[2][3]];
    let mut scale = [0.0; 3];
    for column in 0..3 {
        scale[column] = (0..3)
            .map(|row| matrix[row][column] * matrix[row][column])
            .sum::<f32>()
            .sqrt();
        if !scale[column].is_finite() || scale[column] <= f32::EPSILON {
            return Err(PoseProviderError::DecodeFailed);
        }
    }
    let rotation = std::array::from_fn(|row| {
        std::array::from_fn(|column| matrix[row][column] / scale[column])
    });
    let quaternion = matrix_quaternion(rotation)?;
    Ok(BoneTransform {
        translation: translation
            .map(|value| fixed(value, 1_000.0))
            .transpose_result()?,
        rotation: quaternion
            .map(|value| fixed(value, 1_000_000_000.0))
            .transpose_result()?,
        scale: scale
            .map(|value| fixed(value, 1_000.0))
            .transpose_result()?,
    })
}

trait ArrayResultExt<T, const N: usize> {
    fn transpose_result(self) -> Result<[T; N], PoseProviderError>;
}

impl<T: Copy + Default, const N: usize> ArrayResultExt<T, N> for [Result<T, PoseProviderError>; N] {
    fn transpose_result(self) -> Result<[T; N], PoseProviderError> {
        let mut output = [T::default(); N];
        for (index, value) in self.into_iter().enumerate() {
            output[index] = value?;
        }
        Ok(output)
    }
}

fn fixed(value: f32, scale: f32) -> Result<i64, PoseProviderError> {
    let value = value as f64 * scale as f64;
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(PoseProviderError::DecodeFailed);
    }
    Ok(value.round() as i64)
}

fn matrix_quaternion(matrix: [[f32; 3]; 3]) -> Result<[f32; 4], PoseProviderError> {
    let trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
    let quaternion = if trace > 0.0 {
        let scale = (trace + 1.0).sqrt() * 2.0;
        [
            (matrix[2][1] - matrix[1][2]) / scale,
            (matrix[0][2] - matrix[2][0]) / scale,
            (matrix[1][0] - matrix[0][1]) / scale,
            0.25 * scale,
        ]
    } else if matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2] {
        let scale = (1.0 + matrix[0][0] - matrix[1][1] - matrix[2][2]).sqrt() * 2.0;
        [
            0.25 * scale,
            (matrix[0][1] + matrix[1][0]) / scale,
            (matrix[0][2] + matrix[2][0]) / scale,
            (matrix[2][1] - matrix[1][2]) / scale,
        ]
    } else if matrix[1][1] > matrix[2][2] {
        let scale = (1.0 + matrix[1][1] - matrix[0][0] - matrix[2][2]).sqrt() * 2.0;
        [
            (matrix[0][1] + matrix[1][0]) / scale,
            0.25 * scale,
            (matrix[1][2] + matrix[2][1]) / scale,
            (matrix[0][2] - matrix[2][0]) / scale,
        ]
    } else {
        let scale = (1.0 + matrix[2][2] - matrix[0][0] - matrix[1][1]).sqrt() * 2.0;
        [
            (matrix[0][2] + matrix[2][0]) / scale,
            (matrix[1][2] + matrix[2][1]) / scale,
            0.25 * scale,
            (matrix[1][0] - matrix[0][1]) / scale,
        ]
    };
    normalize_quaternion(quaternion)
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

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}
