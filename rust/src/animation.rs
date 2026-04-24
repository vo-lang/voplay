use std::collections::HashMap;
use std::sync::Mutex;

use crate::math3d::{self, Mat4, Quat, Vec3, MAT4_IDENTITY};
use crate::model_loader::{ModelId, ModelManager};
use crate::physics_registry::{with_world_in, with_world_ref_in, WorldRegistry};

pub const MAX_JOINTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
}

#[derive(Clone, Debug)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
    pub local_transform: Transform,
}

#[derive(Clone, Debug)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    pub inverse_bind_matrices: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationInterpolation {
    Step,
    Linear,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub struct AnimationChannel {
    pub joint_index: usize,
    pub property: AnimationProperty,
    pub interpolation: AnimationInterpolation,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<AnimationChannel>,
}

#[derive(Clone, Debug)]
pub struct AnimationClipInfo {
    pub name: String,
    pub duration: f32,
}

#[derive(Clone, Debug)]
pub struct ModelAnimationInfo {
    pub has_skeleton: bool,
    pub joint_count: usize,
    pub clips: Vec<AnimationClipInfo>,
}

#[derive(Clone, Debug)]
pub struct BlendState {
    pub clip_index: usize,
    pub time: f32,
    pub progress: f32,
    pub duration: f32,
}

#[derive(Clone, Debug)]
pub struct AnimationState {
    pub clip_index: usize,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    pub playing: bool,
    pub blend_from: Option<BlendState>,
}

#[derive(Default)]
pub struct AnimationWorld {
    states: HashMap<u32, AnimationState>,
    palettes: HashMap<u32, Vec<Mat4>>,
}

static WORLDS: Mutex<Option<WorldRegistry<AnimationWorld>>> = Mutex::new(None);

pub fn create_world() -> u32 {
    let mut reg = WORLDS.lock().unwrap();
    let reg = reg.get_or_insert_with(WorldRegistry::new);
    reg.insert(AnimationWorld::default())
}

pub fn destroy_world(world_id: u32) {
    let mut reg = WORLDS.lock().unwrap();
    if let Some(reg) = reg.as_mut() {
        reg.remove(world_id);
    }
}

pub fn with_world<R>(world_id: u32, f: impl FnOnce(&mut AnimationWorld) -> R) -> R {
    with_world_in(&WORLDS, world_id, f)
}

pub fn get_palette(world_id: u32, target_id: u32) -> Option<Vec<Mat4>> {
    with_world_ref_in(&WORLDS, world_id, |world| {
        world.palettes.get(&target_id).cloned()
    })
    .flatten()
}

pub fn compute_rest_joint_palette(skeleton: &Skeleton) -> Vec<Mat4> {
    let local_poses: Vec<Transform> = skeleton
        .joints
        .iter()
        .map(|joint| joint.local_transform)
        .collect();
    compute_joint_matrices(skeleton, &local_poses)
}

pub fn compute_joint_matrices(skeleton: &Skeleton, local_poses: &[Transform]) -> Vec<Mat4> {
    assert_eq!(
        skeleton.joints.len(),
        local_poses.len(),
        "voplay: skeleton pose length mismatch ({} joints, {} poses)",
        skeleton.joints.len(),
        local_poses.len()
    );
    let mut world_matrices = vec![MAT4_IDENTITY; skeleton.joints.len()];
    for (index, joint) in skeleton.joints.iter().enumerate() {
        let local = transform_to_matrix(&local_poses[index]);
        world_matrices[index] = match joint.parent {
            Some(parent) => math3d::mat4_mul(&world_matrices[parent], &local),
            None => local,
        };
    }
    let mut palette = vec![MAT4_IDENTITY; skeleton.joints.len()];
    for index in 0..skeleton.joints.len() {
        palette[index] = math3d::mat4_mul(
            &world_matrices[index],
            &skeleton.inverse_bind_matrices[index],
        );
    }
    palette
}

impl AnimationWorld {
    pub fn play(&mut self, target_id: u32, clip_index: usize, looping: bool, speed: f32) {
        self.states.insert(
            target_id,
            AnimationState {
                clip_index,
                time: 0.0,
                speed,
                looping,
                playing: true,
                blend_from: None,
            },
        );
    }

    pub fn stop(&mut self, target_id: u32) {
        let state = self
            .states
            .get_mut(&target_id)
            .unwrap_or_else(|| panic!("voplay: animation target not found: {}", target_id));
        state.playing = false;
        state.blend_from = None;
    }

    pub fn crossfade(&mut self, target_id: u32, to_clip: usize, duration: f32) {
        let state = self
            .states
            .get_mut(&target_id)
            .unwrap_or_else(|| panic!("voplay: animation target not found: {}", target_id));
        let duration = duration.max(0.000001);
        let from_clip = state.clip_index;
        let from_time = state.time;
        state.clip_index = to_clip;
        state.time = 0.0;
        state.playing = true;
        state.blend_from = Some(BlendState {
            clip_index: from_clip,
            time: from_time,
            progress: 0.0,
            duration,
        });
    }

    pub fn set_speed(&mut self, target_id: u32, speed: f32) {
        let state = self
            .states
            .get_mut(&target_id)
            .unwrap_or_else(|| panic!("voplay: animation target not found: {}", target_id));
        state.speed = speed;
    }

    pub fn remove(&mut self, target_id: u32) {
        self.states.remove(&target_id);
        self.palettes.remove(&target_id);
    }

    pub fn progress(&self, target_id: u32, models: &ModelManager, model_id: ModelId) -> f32 {
        let Some(state) = self.states.get(&target_id) else {
            return 0.0;
        };
        let model = models
            .get(model_id)
            .unwrap_or_else(|| panic!("voplay: animation model not found: {}", model_id));
        let clip = model.clips.get(state.clip_index).unwrap_or_else(|| {
            panic!(
                "voplay: clip {} out of range for model {}",
                state.clip_index, model_id
            )
        });
        if clip.duration <= 0.0 {
            return 0.0;
        }
        (state.time / clip.duration).clamp(0.0, 1.0)
    }

    pub fn tick(&mut self, dt: f32, models: &ModelManager, entity_models: &HashMap<u32, ModelId>) {
        self.palettes.clear();
        for (&target_id, state) in self.states.iter_mut() {
            let Some(&model_id) = entity_models.get(&target_id) else {
                continue;
            };
            let model = models
                .get(model_id)
                .unwrap_or_else(|| panic!("voplay: animation model not found: {}", model_id));
            let skeleton = model
                .skeleton
                .as_ref()
                .unwrap_or_else(|| panic!("voplay: model {} has no skeleton", model_id));
            let clip = model.clips.get(state.clip_index).unwrap_or_else(|| {
                panic!(
                    "voplay: clip {} out of range for model {}",
                    state.clip_index, model_id
                )
            });

            if state.playing {
                advance_time(
                    &mut state.time,
                    clip.duration,
                    dt * state.speed,
                    state.looping,
                    &mut state.playing,
                );
            }

            let local_pose = if let Some(blend) = state.blend_from.as_mut() {
                let from_clip = model.clips.get(blend.clip_index).unwrap_or_else(|| {
                    panic!(
                        "voplay: clip {} out of range for model {}",
                        blend.clip_index, model_id
                    )
                });
                let mut blend_playing = true;
                advance_time(
                    &mut blend.time,
                    from_clip.duration,
                    dt * state.speed,
                    state.looping,
                    &mut blend_playing,
                );
                blend.progress += dt;
                let factor = (blend.progress / blend.duration).clamp(0.0, 1.0);
                let from_pose = evaluate_clip(skeleton, from_clip, blend.time);
                let to_pose = evaluate_clip(skeleton, clip, state.time);
                if factor >= 1.0 {
                    state.blend_from = None;
                }
                blend_poses(&from_pose, &to_pose, factor)
            } else {
                evaluate_clip(skeleton, clip, state.time)
            };

            let palette = compute_joint_matrices(skeleton, &local_pose);
            self.palettes.insert(target_id, palette);
        }
    }
}

pub fn evaluate_clip(skeleton: &Skeleton, clip: &AnimationClip, time: f32) -> Vec<Transform> {
    let mut local_poses: Vec<Transform> = skeleton
        .joints
        .iter()
        .map(|joint| joint.local_transform)
        .collect();
    for channel in &clip.channels {
        match channel.property {
            AnimationProperty::Translation => {
                local_poses[channel.joint_index].translation = sample_vec3(channel, time);
            }
            AnimationProperty::Rotation => {
                local_poses[channel.joint_index].rotation = sample_quat(channel, time);
            }
            AnimationProperty::Scale => {
                local_poses[channel.joint_index].scale = sample_vec3(channel, time);
            }
        }
    }
    local_poses
}

fn transform_to_matrix(transform: &Transform) -> Mat4 {
    math3d::model_matrix(transform.translation, transform.rotation, transform.scale)
}

fn advance_time(time: &mut f32, duration: f32, delta: f32, looping: bool, playing: &mut bool) {
    if duration <= 0.0 {
        *time = 0.0;
        *playing = false;
        return;
    }
    *time += delta;
    if looping {
        *time = (*time).rem_euclid(duration);
        return;
    }
    if *time >= duration {
        *time = duration;
        *playing = false;
    } else if *time < 0.0 {
        *time = 0.0;
        *playing = false;
    }
}

fn blend_poses(from_pose: &[Transform], to_pose: &[Transform], factor: f32) -> Vec<Transform> {
    assert_eq!(
        from_pose.len(),
        to_pose.len(),
        "voplay: animation blend pose length mismatch"
    );
    let mut blended = Vec::with_capacity(from_pose.len());
    for index in 0..from_pose.len() {
        blended.push(Transform {
            translation: lerp_vec3(
                from_pose[index].translation,
                to_pose[index].translation,
                factor,
            ),
            rotation: quat_slerp(from_pose[index].rotation, to_pose[index].rotation, factor),
            scale: lerp_vec3(from_pose[index].scale, to_pose[index].scale, factor),
        });
    }
    blended
}

fn sample_vec3(channel: &AnimationChannel, time: f32) -> Vec3 {
    assert!(matches!(
        channel.property,
        AnimationProperty::Translation | AnimationProperty::Scale
    ));
    let key = find_keyframe(&channel.times, time);
    match key {
        Keyframe::Single(index) => {
            vec3_from_values(channel, index, value_group_index(channel.interpolation))
        }
        Keyframe::Between(index, next_index, factor) => match channel.interpolation {
            AnimationInterpolation::Step => vec3_from_values(channel, index, 0),
            AnimationInterpolation::Linear => {
                let a = vec3_from_values(channel, index, 0);
                let b = vec3_from_values(channel, next_index, 0);
                lerp_vec3(a, b, factor)
            }
            AnimationInterpolation::CubicSpline => {
                sample_cubic_vec3(channel, index, next_index, factor)
            }
        },
    }
}

fn sample_quat(channel: &AnimationChannel, time: f32) -> Quat {
    assert_eq!(channel.property, AnimationProperty::Rotation);
    let key = find_keyframe(&channel.times, time);
    match key {
        Keyframe::Single(index) => {
            quat_from_values(channel, index, value_group_index(channel.interpolation))
        }
        Keyframe::Between(index, next_index, factor) => match channel.interpolation {
            AnimationInterpolation::Step => quat_from_values(channel, index, 0),
            AnimationInterpolation::Linear => {
                let a = quat_from_values(channel, index, 0);
                let b = quat_from_values(channel, next_index, 0);
                quat_slerp(a, b, factor)
            }
            AnimationInterpolation::CubicSpline => {
                sample_cubic_quat(channel, index, next_index, factor)
            }
        },
    }
}

enum Keyframe {
    Single(usize),
    Between(usize, usize, f32),
}

fn find_keyframe(times: &[f32], time: f32) -> Keyframe {
    assert!(
        !times.is_empty(),
        "voplay: animation channel has no keyframes"
    );
    if times.len() == 1 || time <= times[0] {
        return Keyframe::Single(0);
    }
    let last_index = times.len() - 1;
    if time >= times[last_index] {
        return Keyframe::Single(last_index);
    }
    for index in 0..last_index {
        let t0 = times[index];
        let t1 = times[index + 1];
        if time >= t0 && time <= t1 {
            let span = t1 - t0;
            let factor = if span <= 0.0 { 0.0 } else { (time - t0) / span };
            return Keyframe::Between(index, index + 1, factor.clamp(0.0, 1.0));
        }
    }
    Keyframe::Single(last_index)
}

fn channel_width(property: AnimationProperty) -> usize {
    match property {
        AnimationProperty::Translation | AnimationProperty::Scale => 3,
        AnimationProperty::Rotation => 4,
    }
}

fn value_group_stride(interpolation: AnimationInterpolation) -> usize {
    match interpolation {
        AnimationInterpolation::CubicSpline => 3,
        AnimationInterpolation::Step | AnimationInterpolation::Linear => 1,
    }
}

fn value_group_index(interpolation: AnimationInterpolation) -> usize {
    match interpolation {
        AnimationInterpolation::CubicSpline => 1,
        AnimationInterpolation::Step | AnimationInterpolation::Linear => 0,
    }
}

fn value_offset(
    channel: &AnimationChannel,
    key_index: usize,
    component_index: usize,
    group_index: usize,
) -> usize {
    let width = channel_width(channel.property);
    (key_index * value_group_stride(channel.interpolation) + group_index) * width + component_index
}

fn vec3_from_values(channel: &AnimationChannel, key_index: usize, group_index: usize) -> Vec3 {
    Vec3::new(
        channel.values[value_offset(channel, key_index, 0, group_index)],
        channel.values[value_offset(channel, key_index, 1, group_index)],
        channel.values[value_offset(channel, key_index, 2, group_index)],
    )
}

fn quat_from_values(channel: &AnimationChannel, key_index: usize, group_index: usize) -> Quat {
    quat_normalize(quat_from_raw_values(channel, key_index, group_index))
}

fn quat_from_raw_values(channel: &AnimationChannel, key_index: usize, group_index: usize) -> Quat {
    Quat::new(
        channel.values[value_offset(channel, key_index, 0, group_index)],
        channel.values[value_offset(channel, key_index, 1, group_index)],
        channel.values[value_offset(channel, key_index, 2, group_index)],
        channel.values[value_offset(channel, key_index, 3, group_index)],
    )
}

fn sample_cubic_vec3(
    channel: &AnimationChannel,
    index: usize,
    next_index: usize,
    factor: f32,
) -> Vec3 {
    let t0 = channel.times[index];
    let t1 = channel.times[next_index];
    let dt = t1 - t0;
    let p0 = vec3_from_values(channel, index, 1);
    let m0 = vec3_from_values(channel, index, 2);
    let p1 = vec3_from_values(channel, next_index, 1);
    let m1 = vec3_from_values(channel, next_index, 0);
    cubic_hermite_vec3(p0, m0, p1, m1, factor, dt)
}

fn sample_cubic_quat(
    channel: &AnimationChannel,
    index: usize,
    next_index: usize,
    factor: f32,
) -> Quat {
    let t0 = channel.times[index];
    let t1 = channel.times[next_index];
    let dt = t1 - t0;
    let p0 = quat_from_values(channel, index, 1);
    let m0 = quat_from_raw_values(channel, index, 2);
    let p1 = quat_from_values(channel, next_index, 1);
    let m1 = quat_from_raw_values(channel, next_index, 0);
    quat_normalize(cubic_hermite_quat(p0, m0, p1, m1, factor, dt))
}

fn cubic_hermite_scalar(p0: f32, m0: f32, p1: f32, m1: f32, t: f32, dt: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * p0
        + (t3 - 2.0 * t2 + t) * m0 * dt
        + (-2.0 * t3 + 3.0 * t2) * p1
        + (t3 - t2) * m1 * dt
}

fn cubic_hermite_vec3(p0: Vec3, m0: Vec3, p1: Vec3, m1: Vec3, t: f32, dt: f32) -> Vec3 {
    Vec3::new(
        cubic_hermite_scalar(p0.x, m0.x, p1.x, m1.x, t, dt),
        cubic_hermite_scalar(p0.y, m0.y, p1.y, m1.y, t, dt),
        cubic_hermite_scalar(p0.z, m0.z, p1.z, m1.z, t, dt),
    )
}

fn cubic_hermite_quat(p0: Quat, m0: Quat, p1: Quat, m1: Quat, t: f32, dt: f32) -> Quat {
    Quat::new(
        cubic_hermite_scalar(p0.x, m0.x, p1.x, m1.x, t, dt),
        cubic_hermite_scalar(p0.y, m0.y, p1.y, m1.y, t, dt),
        cubic_hermite_scalar(p0.z, m0.z, p1.z, m1.z, t, dt),
        cubic_hermite_scalar(p0.w, m0.w, p1.w, m1.w, t, dt),
    )
}

fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

fn quat_dot(a: Quat, b: Quat) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w
}

fn quat_normalize(q: Quat) -> Quat {
    let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if len == 0.0 {
        return Quat::IDENTITY;
    }
    Quat::new(q.x / len, q.y / len, q.z / len, q.w / len)
}

fn quat_slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut end = b;
    let mut cos_theta = quat_dot(a, b);
    if cos_theta < 0.0 {
        end = Quat::new(-b.x, -b.y, -b.z, -b.w);
        cos_theta = -cos_theta;
    }
    if cos_theta > 0.9995 {
        return quat_normalize(Quat::new(
            a.x + (end.x - a.x) * t,
            a.y + (end.y - a.y) * t,
            a.z + (end.z - a.z) * t,
            a.w + (end.w - a.w) * t,
        ));
    }
    let theta = cos_theta.acos();
    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;
    quat_normalize(Quat::new(
        a.x * wa + end.x * wb,
        a.y * wa + end.y * wb,
        a.z * wa + end.z * wb,
        a.w * wa + end.w * wb,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_skeleton() -> Skeleton {
        Skeleton {
            joints: vec![
                Joint {
                    name: "root".to_string(),
                    parent: None,
                    local_transform: Transform::IDENTITY,
                },
                Joint {
                    name: "child".to_string(),
                    parent: Some(0),
                    local_transform: Transform {
                        translation: Vec3::new(0.0, 1.0, 0.0),
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                },
            ],
            inverse_bind_matrices: vec![MAT4_IDENTITY, MAT4_IDENTITY],
        }
    }

    #[test]
    fn computes_rest_palette() {
        let skeleton = test_skeleton();
        let palette = compute_rest_joint_palette(&skeleton);
        assert_eq!(palette.len(), 2);
        assert!((palette[1][3][1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn samples_linear_translation() {
        let skeleton = test_skeleton();
        let clip = AnimationClip {
            name: "move".to_string(),
            duration: 1.0,
            channels: vec![AnimationChannel {
                joint_index: 1,
                property: AnimationProperty::Translation,
                interpolation: AnimationInterpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 1.0, 0.0, 2.0, 1.0, 0.0],
            }],
        };
        let pose = evaluate_clip(&skeleton, &clip, 0.5);
        assert!((pose[1].translation.x - 1.0).abs() < 1e-6);
        assert!((pose[1].translation.y - 1.0).abs() < 1e-6);
    }
}
