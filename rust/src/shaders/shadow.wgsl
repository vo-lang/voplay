struct LightVPUniform {
    light_vp: mat4x4<f32>,
};

struct ShadowModelUniform {
    model: mat4x4<f32>,
    joint_count: vec4<u32>,
    joints: array<mat4x4<f32>, 128>,
};

@group(0) @binding(0) var<uniform> light: LightVPUniform;
@group(1) @binding(0) var<uniform> shadow_model: ShadowModelUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
};

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) joint_indices: vec4<u32>,
    @location(6) joint_weights: vec4<f32>,
};

fn skin_matrix(joint_indices: vec4<u32>, joint_weights: vec4<f32>) -> mat4x4<f32> {
    return joint_weights.x * shadow_model.joints[joint_indices.x]
        + joint_weights.y * shadow_model.joints[joint_indices.y]
        + joint_weights.z * shadow_model.joints[joint_indices.z]
        + joint_weights.w * shadow_model.joints[joint_indices.w];
}

@vertex
fn vs_shadow(in: VertexInput) -> @builtin(position) vec4<f32> {
    return light.light_vp * shadow_model.model * vec4<f32>(in.position, 1.0);
}

@vertex
fn vs_shadow_skinned(in: SkinnedVertexInput) -> @builtin(position) vec4<f32> {
    let skinned_pos = skin_matrix(in.joint_indices, in.joint_weights) * vec4<f32>(in.position, 1.0);
    return light.light_vp * shadow_model.model * skinned_pos;
}
