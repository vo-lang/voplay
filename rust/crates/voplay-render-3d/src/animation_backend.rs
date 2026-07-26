use std::collections::BTreeMap;

use voplay_runtime::{
    animation_gpu::{
        verify_encoding, AnimationGpuAck, AnimationGpuBackend, AnimationGpuBackendFailure,
        AnimationGpuBatch, AnimationGpuCommand,
    },
    animation_presentation::BoneTransform,
    asset::AssetRef,
    RenderEntity,
};

use crate::{SkinPaletteUpload3d, WgpuSceneRenderer};

#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonPaletteDescriptor3d {
    pub skeleton: AssetRef,
    pub parents: Vec<Option<usize>>,
    pub inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkeletonPaletteError3d {
    Closed,
    InvalidConfig,
    InvalidDescriptor,
    DuplicateSkeleton,
    MissingSkeleton,
    Capacity,
    Cycle,
    InvalidPose,
}

pub struct SkeletonPaletteRegistry3d {
    max_skeletons: usize,
    max_joints_per_skeleton: usize,
    skeletons: BTreeMap<AssetRef, SkeletonPaletteDescriptor3d>,
    closed: bool,
}

impl SkeletonPaletteRegistry3d {
    pub fn new(
        max_skeletons: usize,
        max_joints_per_skeleton: usize,
    ) -> Result<Self, SkeletonPaletteError3d> {
        if max_skeletons == 0 || max_joints_per_skeleton == 0 {
            return Err(SkeletonPaletteError3d::InvalidConfig);
        }
        Ok(Self {
            max_skeletons,
            max_joints_per_skeleton,
            skeletons: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> usize {
        let released = self.skeletons.len();
        self.closed = true;
        self.skeletons.clear();
        released
    }

    pub fn register(
        &mut self,
        descriptor: SkeletonPaletteDescriptor3d,
    ) -> Result<(), SkeletonPaletteError3d> {
        self.ensure_open()?;
        if !descriptor.skeleton.engine.is_valid()
            || !descriptor.skeleton.handle.is_valid()
            || descriptor.parents.is_empty()
            || descriptor.parents.len() != descriptor.inverse_bind_matrices.len()
            || descriptor.parents.len() > self.max_joints_per_skeleton
            || descriptor
                .parents
                .iter()
                .enumerate()
                .any(|(joint, parent)| parent.is_some_and(|parent| parent >= joint))
            || descriptor
                .inverse_bind_matrices
                .iter()
                .flatten()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(SkeletonPaletteError3d::InvalidDescriptor);
        }
        if self.skeletons.contains_key(&descriptor.skeleton) {
            return Err(SkeletonPaletteError3d::DuplicateSkeleton);
        }
        if self.skeletons.len() == self.max_skeletons {
            return Err(SkeletonPaletteError3d::Capacity);
        }
        self.skeletons.insert(descriptor.skeleton, descriptor);
        Ok(())
    }

    pub fn remove(&mut self, skeleton: AssetRef) -> bool {
        if self.closed {
            return false;
        }
        self.skeletons.remove(&skeleton).is_some()
    }

    pub fn build_palette(
        &self,
        skeleton: AssetRef,
        bones: &[BoneTransform],
    ) -> Result<Vec<[[f32; 4]; 4]>, SkeletonPaletteError3d> {
        self.ensure_open()?;
        let descriptor = self
            .skeletons
            .get(&skeleton)
            .ok_or(SkeletonPaletteError3d::MissingSkeleton)?;
        if bones.len() != descriptor.parents.len() {
            return Err(SkeletonPaletteError3d::InvalidPose);
        }
        let locals = bones
            .iter()
            .map(bone_matrix)
            .collect::<Result<Vec<_>, _>>()?;
        let mut globals = Vec::with_capacity(locals.len());
        for (joint, local) in locals.into_iter().enumerate() {
            let global =
                descriptor.parents[joint].map_or(local, |parent| multiply(globals[parent], local));
            globals.push(global);
        }
        globals
            .into_iter()
            .zip(&descriptor.inverse_bind_matrices)
            .map(|(global, inverse_bind)| {
                let palette = multiply(global, columns_to_rows(*inverse_bind));
                if palette.iter().flatten().any(|value| !value.is_finite()) {
                    return Err(SkeletonPaletteError3d::InvalidPose);
                }
                Ok(rows_to_columns(palette))
            })
            .collect()
    }

    fn ensure_open(&self) -> Result<(), SkeletonPaletteError3d> {
        if self.closed {
            Err(SkeletonPaletteError3d::Closed)
        } else {
            Ok(())
        }
    }
}

pub struct WgpuSkinningBackend3d<'a> {
    pub renderer: &'a mut WgpuSceneRenderer,
    pub skeletons: &'a SkeletonPaletteRegistry3d,
    pub engine: voplay_protocol::EngineId,
    pub endpoint_generation: voplay_protocol::Handle,
    pub actor: voplay_protocol::Handle,
}

impl AnimationGpuBackend for WgpuSkinningBackend3d<'_> {
    fn engine(&self) -> voplay_protocol::EngineId {
        self.engine
    }

    fn endpoint_generation(&self) -> voplay_protocol::Handle {
        self.endpoint_generation
    }

    fn actor(&self) -> voplay_protocol::Handle {
        self.actor
    }

    fn submit(
        &mut self,
        batch: &AnimationGpuBatch,
    ) -> Result<AnimationGpuAck, AnimationGpuBackendFailure> {
        if batch.engine != self.engine
            || batch.endpoint_generation != self.endpoint_generation
            || batch.actor != self.actor
            || verify_encoding(&batch.encoded, batch.commands.len()).is_err()
            || batch
                .commands
                .iter()
                .any(|command| matches!(command, AnimationGpuCommand::Sprite { .. }))
        {
            return Err(AnimationGpuBackendFailure::RejectedBeforeSubmit);
        }
        let prepared = batch
            .commands
            .iter()
            .map(|command| match command {
                AnimationGpuCommand::Skinning {
                    entity,
                    skeleton,
                    palette,
                    ..
                } => {
                    let palette = self
                        .skeletons
                        .build_palette(*skeleton, palette)
                        .map_err(|_| AnimationGpuBackendFailure::RejectedBeforeSubmit)?;
                    Ok((
                        RenderEntity {
                            engine: entity.world.engine,
                            entity: entity.entity,
                        },
                        palette,
                    ))
                }
                AnimationGpuCommand::Sprite { .. } => {
                    Err(AnimationGpuBackendFailure::RejectedBeforeSubmit)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (entity, joint_matrices) in &prepared {
            self.renderer
                .upload_skin_palette(SkinPaletteUpload3d {
                    entity: *entity,
                    joint_matrices,
                })
                .map_err(|_| AnimationGpuBackendFailure::OutcomeUnknown)?;
        }
        Ok(AnimationGpuAck {
            submission_id: batch.submission_id,
            presentation_revision: batch.presentation_revision,
            command_count: batch.commands.len(),
            encoded_bytes: batch.encoded.len(),
            digest: batch.digest,
        })
    }
}

fn bone_matrix(bone: &BoneTransform) -> Result<[[f32; 4]; 4], SkeletonPaletteError3d> {
    let translation = bone.translation.map(|value| value as f32 / 1_000.0);
    let mut rotation = bone.rotation.map(|value| value as f32 / 1_000_000_000.0);
    let scale = bone.scale.map(|value| value as f32 / 1_000.0);
    let length = rotation
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if !length.is_finite()
        || length <= f32::EPSILON
        || translation.iter().any(|value| !value.is_finite())
        || scale.iter().any(|value| !value.is_finite())
    {
        return Err(SkeletonPaletteError3d::InvalidPose);
    }
    rotation.iter_mut().for_each(|value| *value /= length);
    let [x, y, z, w] = rotation;
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
            matrix[row][column] = rotation[row][column] * scale[column];
        }
        matrix[row][3] = translation[row];
    }
    Ok(matrix)
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
