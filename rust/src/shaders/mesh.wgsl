// 3D mesh shader — Blinn-Phong lighting.
// Supports directional and point lights, base color tint, and optional albedo texture.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct ModelUniform {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>, // transpose(inverse(model)) — upper 3x3 in 4x4
    base_color: vec4<f32>,
    material_params: vec4<f32>,
    emissive_color: vec4<f32>,
    texture_flags: vec4<f32>,
};

// Light types: 0 = directional, 1 = point
struct LightData {
    position_or_dir: vec4<f32>, // xyz = position (point) or direction (dir), w = type (0/1)
    color_intensity: vec4<f32>, // rgb = color, a = intensity
};

struct LightUniform {
    ambient: vec4<f32>,   // rgb = ambient color, a = unused
    ambient_ground: vec4<f32>,
    count: vec4<u32>,     // x = number of lights, y = fog mode
    lights: array<LightData, 8>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    shadow_vp: mat4x4<f32>,
    shadow_params: vec4<f32>,
    color_params: vec4<f32>,
    debug_params: vec4<u32>,
};

const RENDER_DEBUG_LIT: u32 = 0u;
const RENDER_DEBUG_ALBEDO: u32 = 1u;
const RENDER_DEBUG_NORMAL: u32 = 2u;
const RENDER_DEBUG_ROUGHNESS: u32 = 3u;
const RENDER_DEBUG_METALLIC: u32 = 4u;
const RENDER_DEBUG_SHADOW: u32 = 5u;
const RENDER_DEBUG_DIRECT: u32 = 6u;
const RENDER_DEBUG_AMBIENT: u32 = 7u;

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> model: ModelUniform;
@group(2) @binding(0) var<uniform> light_uni: LightUniform;
@group(3) @binding(0) var albedo_tex: texture_2d<f32>;
@group(3) @binding(1) var albedo_sampler: sampler;
@group(3) @binding(2) var shadow_tex: texture_depth_2d;
@group(3) @binding(3) var normal_tex: texture_2d<f32>;
@group(3) @binding(4) var metallic_roughness_tex: texture_2d<f32>;
@group(3) @binding(5) var emissive_tex: texture_2d<f32>;
@group(3) @binding(6) var toon_ramp_tex: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) base_color: vec4<f32>,
    @location(4) material_params: vec4<f32>,
    @location(5) emissive_color: vec4<f32>,
    @location(6) world_tangent: vec4<f32>,
    @location(7) texture_flags: vec4<f32>,
    @location(8) vertex_color: vec4<f32>,
};

struct InstanceInput {
    @location(5) model_0: vec4<f32>,
    @location(6) model_1: vec4<f32>,
    @location(7) model_2: vec4<f32>,
    @location(8) model_3: vec4<f32>,
    @location(9) normal_0: vec4<f32>,
    @location(10) normal_1: vec4<f32>,
    @location(11) normal_2: vec4<f32>,
    @location(12) base_color: vec4<f32>,
    @location(13) material_params: vec4<f32>,
    @location(14) emissive_color: vec4<f32>,
    @location(15) texture_flags: vec4<f32>,
};

fn apply_fog(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let fog_mode = light_uni.count.y;
    if fog_mode == 0u {
        return color;
    }

    let dist = distance(world_pos, camera.camera_pos);
    let fog_color = light_uni.fog_color.rgb;
    let start = light_uni.fog_params.x;
    let end_d = light_uni.fog_params.y;
    let density = light_uni.fog_params.z;

    var factor: f32;
    if fog_mode == 1u {
        factor = clamp((end_d - dist) / (end_d - start), 0.0, 1.0);
    } else if fog_mode == 2u {
        factor = exp(-density * dist);
    } else {
        let dd = density * dist;
        factor = exp(-dd * dd);
    }

    return fog_color + (color - fog_color) * factor;
}

fn tone_map_aces(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + vec3<f32>(b))) / (color * (c * color + vec3<f32>(d)) + vec3<f32>(e)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn apply_color_grading(color: vec3<f32>) -> vec3<f32> {
    let exposure = max(light_uni.color_params.x, 0.0);
    let contrast = max(light_uni.color_params.y, 0.0);
    let saturation = max(light_uni.color_params.z, 0.0);
    let tone_map = u32(light_uni.color_params.w + 0.5);
    var graded = max(color * exposure, vec3<f32>(0.0));
    if tone_map == 1u {
        graded = graded / (vec3<f32>(1.0) + graded);
    } else if tone_map == 2u {
        graded = tone_map_aces(graded);
    }
    let luma = dot(graded, vec3<f32>(0.2126, 0.7152, 0.0722));
    graded = vec3<f32>(luma) + (graded - vec3<f32>(luma)) * saturation;
    graded = vec3<f32>(0.5) + (graded - vec3<f32>(0.5)) * contrast;
    return max(graded, vec3<f32>(0.0));
}

fn shadow_factor(world_pos: vec3<f32>) -> f32 {
    if light_uni.shadow_params.x < 0.5 {
        return 1.0;
    }

    let shadow_clip = light_uni.shadow_vp * vec4<f32>(world_pos, 1.0);
    if shadow_clip.w <= 0.0 {
        return 1.0;
    }

    let shadow_ndc = shadow_clip.xyz / shadow_clip.w;
    let uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, shadow_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || shadow_ndc.z <= 0.0 || shadow_ndc.z >= 1.0 {
        return 1.0;
    }

    let shadow_size = vec2<i32>(textureDimensions(shadow_tex));
    let base_texel = vec2<i32>(uv * vec2<f32>(shadow_size));
    let max_texel = shadow_size - vec2<i32>(1, 1);
    let compare_depth = shadow_ndc.z - light_uni.shadow_params.y;
    var visibility = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let texel = clamp(base_texel + vec2<i32>(x, y), vec2<i32>(0, 0), max_texel);
            let sampled_depth = textureLoad(shadow_tex, texel, 0);
            visibility += select(0.0, 1.0, compare_depth <= sampled_depth);
        }
    }
    let raw_visibility = visibility / 9.0;
    let strength = clamp(light_uni.shadow_params.w, 0.0, 1.0);
    return 1.0 + (raw_visibility - 1.0) * strength;
}

fn ambient_light(normal: vec3<f32>) -> vec3<f32> {
    let sky_weight = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    return light_uni.ambient_ground.rgb + (light_uni.ambient.rgb - light_uni.ambient_ground.rgb) * sky_weight;
}

fn material_uv(in: VertexOutput) -> vec2<f32> {
    let uv_scale = in.material_params.x;
    return in.uv * vec2<f32>(uv_scale, uv_scale);
}

fn surface_normal(in: VertexOutput) -> vec3<f32> {
    let N = normalize(in.world_normal);
    let T_raw = in.world_tangent.xyz;
    let T = normalize(T_raw - N * dot(N, T_raw));
    let B = normalize(cross(N, T) * in.world_tangent.w);
    let sampled = textureSample(normal_tex, albedo_sampler, material_uv(in)).xyz * 2.0 - vec3<f32>(1.0);
    let tangent_normal = normalize(vec3<f32>(sampled.xy * in.texture_flags.x, sampled.z));
    let mapped = normalize(mat3x3<f32>(T, B, N) * tangent_normal);
    let normal_active = select(0.0, 1.0, in.texture_flags.x > 0.0);
    return normalize(N + (mapped - N) * normal_active);
}

fn roughness_metallic(in: VertexOutput) -> vec2<f32> {
    let mr = textureSample(metallic_roughness_tex, albedo_sampler, material_uv(in));
    let mr_active = select(0.0, 1.0, in.texture_flags.y > 0.5);
    let roughness = clamp(in.material_params.y * (1.0 + (mr.g - 1.0) * mr_active), 0.04, 1.0);
    let metallic = clamp(in.material_params.z * (1.0 + (mr.b - 1.0) * mr_active), 0.0, 1.0);
    return vec2<f32>(roughness, metallic);
}

fn diffuse_shape(diff: f32, toon: bool, in: VertexOutput) -> f32 {
    let ramp = textureSample(toon_ramp_tex, albedo_sampler, vec2<f32>(clamp(diff, 0.0, 1.0), 0.5)).r;
    let toon_step_0 = select(0.0, 0.28, diff > 0.08);
    let toon_step_1 = select(toon_step_0, 0.62, diff > 0.36);
    let toon_step_2 = select(toon_step_1, 1.0, diff > 0.72);
    let shaped = select(diff, toon_step_2, toon);
    return select(shaped, ramp, in.texture_flags.w > 0.5);
}

fn emissive_value(in: VertexOutput) -> vec3<f32> {
    let factor = in.emissive_color.rgb * max(in.emissive_color.a, 1.0);
    let sampled = textureSample(emissive_tex, albedo_sampler, material_uv(in)).rgb * factor;
    return select(factor, sampled, in.texture_flags.z > 0.5);
}

fn shade(albedo: vec4<f32>, in: VertexOutput) -> vec4<f32> {
    let N = surface_normal(in);
    let V = normalize(camera.camera_pos - in.world_pos);
    let rm = roughness_metallic(in);
    let roughness = rm.x;
    let metallic = rm.y;
    let toon = in.material_params.w >= 0.5;

    let ambient_color = ambient_light(N) * albedo.rgb;
    var direct_color = vec3<f32>(0.0);
    var shadow_debug = 1.0;
    let num_lights = min(light_uni.count.x, 8u);
    for (var i = 0u; i < num_lights; i = i + 1u) {
        let light = light_uni.lights[i];
        let light_type = u32(light.position_or_dir.w);
        let light_color = light.color_intensity.rgb * light.color_intensity.a;

        var L: vec3<f32>;
        var attenuation = 1.0;

        if light_type == 0u {
            L = normalize(light.position_or_dir.xyz);
        } else {
            let to_light = light.position_or_dir.xyz - in.world_pos;
            let dist = length(to_light);
            L = to_light / dist;
            attenuation = 1.0 / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }

        var shadow = 1.0;
        if light_type == 0u && i == light_uni.count.z {
            shadow = shadow_factor(in.world_pos);
            shadow_debug = shadow;
        }

        let diff = diffuse_shape(max(dot(N, L), 0.0), toon, in);
        direct_color += albedo.rgb * light_color * diff * attenuation * shadow;

        let H = normalize(L + V);
        let spec_power = 8.0 + (1.0 - roughness) * 88.0;
        let spec = pow(max(dot(N, H), 0.0), spec_power);
        let spec_strength = (0.18 + metallic * 0.45) * (1.0 - roughness * 0.7);
        let spec_color = vec3<f32>(
            1.0 + (albedo.r - 1.0) * metallic,
            1.0 + (albedo.g - 1.0) * metallic,
            1.0 + (albedo.b - 1.0) * metallic,
        );
        direct_color += spec_color * light_color * spec * attenuation * spec_strength * shadow;
    }

    let debug_mode = light_uni.debug_params.x;
    if debug_mode == RENDER_DEBUG_ALBEDO {
        return vec4<f32>(albedo.rgb, albedo.a);
    }
    if debug_mode == RENDER_DEBUG_NORMAL {
        return vec4<f32>(N * 0.5 + vec3<f32>(0.5), albedo.a);
    }
    if debug_mode == RENDER_DEBUG_ROUGHNESS {
        return vec4<f32>(vec3<f32>(roughness), albedo.a);
    }
    if debug_mode == RENDER_DEBUG_METALLIC {
        return vec4<f32>(vec3<f32>(metallic), albedo.a);
    }
    if debug_mode == RENDER_DEBUG_SHADOW {
        return vec4<f32>(vec3<f32>(shadow_debug), albedo.a);
    }
    if debug_mode == RENDER_DEBUG_DIRECT {
        return vec4<f32>(direct_color, albedo.a);
    }
    if debug_mode == RENDER_DEBUG_AMBIENT {
        return vec4<f32>(ambient_color, albedo.a);
    }

    var color = ambient_color + direct_color;
    color += emissive_value(in);
    color = apply_fog(color, in.world_pos);
    color = apply_color_grading(color);
    return vec4<f32>(color, albedo.a);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = model.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_proj * world_pos;
    out.world_pos = world_pos.xyz;
    out.world_normal = normalize((model.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_tangent = vec4<f32>(normalize((model.normal_matrix * vec4<f32>(in.tangent.xyz, 0.0)).xyz), in.tangent.w);
    out.uv = in.uv;
    out.base_color = model.base_color;
    out.material_params = model.material_params;
    out.emissive_color = model.emissive_color;
    out.texture_flags = model.texture_flags;
    out.vertex_color = in.color;
    return out;
}

@vertex
fn vs_instanced(in: VertexInput, instance: InstanceInput) -> VertexOutput {
    var out: VertexOutput;
    let model_mat = mat4x4<f32>(instance.model_0, instance.model_1, instance.model_2, instance.model_3);
    let normal_mat = mat4x4<f32>(instance.normal_0, instance.normal_1, instance.normal_2, vec4<f32>(0.0, 0.0, 0.0, 1.0));
    let world_pos = model_mat * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_proj * world_pos;
    out.world_pos = world_pos.xyz;
    out.world_normal = normalize((normal_mat * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_tangent = vec4<f32>(normalize((normal_mat * vec4<f32>(in.tangent.xyz, 0.0)).xyz), in.tangent.w);
    out.uv = in.uv;
    out.base_color = instance.base_color;
    out.material_params = instance.material_params;
    out.emissive_color = instance.emissive_color;
    out.texture_flags = instance.texture_flags;
    out.vertex_color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(albedo_tex, albedo_sampler, material_uv(in)) * in.base_color * in.vertex_color;
    return shade(albedo, in);
}

// Fragment shader variant without texture (uses base_color only).
@fragment
fn fs_main_no_tex(in: VertexOutput) -> @location(0) vec4<f32> {
    return shade(in.base_color * in.vertex_color, in);
}

@fragment
fn fs_instanced(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(albedo_tex, albedo_sampler, material_uv(in)) * in.base_color * in.vertex_color;
    return shade(albedo, in);
}

@fragment
fn fs_instanced_no_tex(in: VertexOutput) -> @location(0) vec4<f32> {
    return shade(in.base_color * in.vertex_color, in);
}
