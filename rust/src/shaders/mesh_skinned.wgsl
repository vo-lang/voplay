struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct SkinnedModelUniform {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    base_color: vec4<f32>,
    material_params: vec4<f32>,
    emissive_color: vec4<f32>,
    texture_flags: vec4<f32>,
    material_response: vec4<f32>,
    texture_flags2: vec4<f32>,
    joint_count: vec4<u32>,
    joints: array<mat4x4<f32>, 128>,
};

struct LightData {
    position_or_dir: vec4<f32>,
    color_intensity: vec4<f32>,
};

struct LightUniform {
    ambient: vec4<f32>,
    ambient_ground: vec4<f32>,
    count: vec4<u32>,
    lights: array<LightData, 8>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    shadow_vp: mat4x4<f32>,
    shadow_cascade_vp: array<mat4x4<f32>, 4>,
    shadow_cascade_splits: vec4<f32>,
    shadow_params: vec4<f32>,
    shadow_params2: vec4<f32>,
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
const PI: f32 = 3.14159265359;

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> skinned_model: SkinnedModelUniform;
@group(2) @binding(0) var<uniform> light_uni: LightUniform;
@group(3) @binding(0) var albedo_tex: texture_2d<f32>;
@group(3) @binding(1) var albedo_sampler: sampler;
@group(3) @binding(2) var shadow_tex: texture_depth_2d;
@group(3) @binding(3) var normal_tex: texture_2d<f32>;
@group(3) @binding(4) var metallic_roughness_tex: texture_2d<f32>;
@group(3) @binding(5) var emissive_tex: texture_2d<f32>;
@group(3) @binding(6) var toon_ramp_tex: texture_2d<f32>;
@group(3) @binding(7) var mask_tex: texture_2d<f32>;

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) joint_indices: vec4<u32>,
    @location(6) joint_weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world_tangent: vec4<f32>,
    @location(4) vertex_color: vec4<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) receiver_mask: vec4<f32>,
    @location(2) surface_props: vec4<f32>,
};

fn surface_props(in: VertexOutput) -> vec4<f32> {
    let N = surface_normal(in);
    let rm = roughness_metallic(in);
    return vec4<f32>(N * 0.5 + vec3<f32>(0.5), rm.x);
}

fn fragment_output(color: vec4<f32>, in: VertexOutput) -> FragmentOutput {
    var out: FragmentOutput;
    out.color = color;
    out.receiver_mask = vec4<f32>(2.0 / 255.0, 0.0, 0.0, 1.0);
    out.surface_props = surface_props(in);
    return out;
}

fn skinned_textured_fragment_color(in: VertexOutput) -> vec4<f32> {
    var albedo = textureSample(albedo_tex, albedo_sampler, material_uv(in)) * skinned_model.base_color * in.vertex_color;
    albedo.a = albedo.a * material_mask(in).a;
    return shade(albedo, in);
}

fn skinned_untextured_fragment_color(in: VertexOutput) -> vec4<f32> {
    var albedo = skinned_model.base_color * in.vertex_color;
    albedo.a = albedo.a * material_mask(in).a;
    return shade(albedo, in);
}

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
    let shadow_quality = u32(light_uni.shadow_params2.z + 0.5);
    if light_uni.shadow_params.x < 0.5 || shadow_quality == 0u {
        return 1.0;
    }

    let cascade_count = min(u32(light_uni.shadow_params2.w + 0.5), 4u);
    let world_dist = distance(world_pos, camera.camera_pos);
    let shadow_distance = light_uni.shadow_params2.x;
    if shadow_distance > 0.0 && world_dist >= shadow_distance {
        return 1.0;
    }
    var cascade_index = 0u;
    if cascade_count > 1u {
        if world_dist > light_uni.shadow_cascade_splits.x {
            cascade_index = 1u;
        }
        if cascade_count > 2u && world_dist > light_uni.shadow_cascade_splits.y {
            cascade_index = 2u;
        }
        if cascade_count > 3u && world_dist > light_uni.shadow_cascade_splits.z {
            cascade_index = 3u;
        }
        if cascade_index >= cascade_count {
            return 1.0;
        }
    }

    var shadow_matrix = light_uni.shadow_vp;
    if cascade_count > 1u {
        shadow_matrix = light_uni.shadow_cascade_vp[cascade_index];
    }
    let shadow_clip = shadow_matrix * vec4<f32>(world_pos, 1.0);
    if shadow_clip.w <= 0.0 {
        return 1.0;
    }

    let shadow_ndc = shadow_clip.xyz / shadow_clip.w;
    let uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, shadow_ndc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || shadow_ndc.z <= 0.0 || shadow_ndc.z >= 1.0 {
        return 1.0;
    }

    let shadow_size = vec2<i32>(textureDimensions(shadow_tex));
    var atlas_uv = uv;
    var min_texel = vec2<i32>(0, 0);
    var max_texel = shadow_size - vec2<i32>(1, 1);
    if cascade_count > 1u {
        let tile = vec2<f32>(f32(cascade_index % 2u), f32(cascade_index / 2u));
        atlas_uv = (uv + tile) * 0.5;
        let tile_size = max(shadow_size / vec2<i32>(2, 2), vec2<i32>(1, 1));
        let tile_index = vec2<i32>(i32(cascade_index % 2u), i32(cascade_index / 2u));
        min_texel = tile_index * tile_size;
        max_texel = min(min_texel + tile_size - vec2<i32>(1, 1), shadow_size - vec2<i32>(1, 1));
    }
    let base_texel = clamp(vec2<i32>(atlas_uv * vec2<f32>(shadow_size)), min_texel, max_texel);
    let compare_depth = shadow_ndc.z - light_uni.shadow_params.y;
    let softness = clamp(light_uni.shadow_params.z, 0.5, 4.0);
    var sample_radius: i32 = 0;
    if shadow_quality >= 2u {
        sample_radius = 2;
    }
    if shadow_quality >= 3u {
        sample_radius = 3;
    }
    if shadow_quality >= 4u {
        sample_radius = 4;
    }
    var visibility = 0.0;
    var weight_total = 0.0;
    for (var y = -sample_radius; y <= sample_radius; y = y + 1) {
        for (var x = -sample_radius; x <= sample_radius; x = x + 1) {
            let tap = vec2<f32>(f32(x), f32(y));
            let normalized_dist = length(tap) / max(f32(sample_radius), 1.0);
            let weight = 0.2 + max(0.0, 1.0 - normalized_dist) * softness;
            let texel = clamp(base_texel + vec2<i32>(x, y), min_texel, max_texel);
            let sampled_depth = textureLoad(shadow_tex, texel, 0);
            visibility += select(0.0, 1.0, compare_depth <= sampled_depth) * weight;
            weight_total += weight;
        }
    }
    let raw_visibility = visibility / max(weight_total, 0.0001);
    let strength = clamp(light_uni.shadow_params.w, 0.0, 1.0);
    var distance_fade = 1.0;
    if shadow_distance > 0.0 {
        let fade_width = max(light_uni.shadow_params2.y, 1.0);
        distance_fade = 1.0 - smoothstep(max(shadow_distance - fade_width, 0.0), shadow_distance, world_dist);
    }
    return 1.0 + (raw_visibility - 1.0) * strength * distance_fade;
}

fn ambient_light(normal: vec3<f32>) -> vec3<f32> {
    let sky_weight = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    return light_uni.ambient_ground.rgb + (light_uni.ambient.rgb - light_uni.ambient_ground.rgb) * sky_weight;
}

fn skin_matrix(joint_indices: vec4<u32>, joint_weights: vec4<f32>) -> mat4x4<f32> {
    return joint_weights.x * skinned_model.joints[joint_indices.x]
        + joint_weights.y * skinned_model.joints[joint_indices.y]
        + joint_weights.z * skinned_model.joints[joint_indices.z]
        + joint_weights.w * skinned_model.joints[joint_indices.w];
}

@vertex
fn vs_skinned(in: SkinnedVertexInput) -> VertexOutput {
    let skin = skin_matrix(in.joint_indices, in.joint_weights);
    let skinned_pos = skin * vec4<f32>(in.position, 1.0);
    let skinned_normal = skin * vec4<f32>(in.normal, 0.0);
    let skinned_tangent = skin * vec4<f32>(in.tangent.xyz, 0.0);

    var out: VertexOutput;
    let world_pos = skinned_model.model * skinned_pos;
    out.clip_position = camera.view_proj * world_pos;
    out.world_pos = world_pos.xyz;
    out.world_normal = normalize((skinned_model.normal_matrix * skinned_normal).xyz);
    out.world_tangent = vec4<f32>(normalize((skinned_model.normal_matrix * skinned_tangent).xyz), in.tangent.w);
    out.uv = in.uv;
    out.vertex_color = in.color;
    return out;
}

fn material_uv(in: VertexOutput) -> vec2<f32> {
    let uv_scale = skinned_model.material_params.x;
    return in.uv * vec2<f32>(uv_scale, uv_scale);
}

fn surface_normal(in: VertexOutput) -> vec3<f32> {
    let N = normalize(in.world_normal);
    let T_raw = in.world_tangent.xyz;
    let T = normalize(T_raw - N * dot(N, T_raw));
    let B = normalize(cross(N, T) * in.world_tangent.w);
    let sampled = textureSample(normal_tex, albedo_sampler, material_uv(in)).xyz * 2.0 - vec3<f32>(1.0);
    let tangent_normal = normalize(vec3<f32>(sampled.xy * skinned_model.texture_flags.x, sampled.z));
    let mapped = normalize(mat3x3<f32>(T, B, N) * tangent_normal);
    let normal_active = select(0.0, 1.0, skinned_model.texture_flags.x > 0.0);
    return normalize(N + (mapped - N) * normal_active);
}

fn material_mask(in: VertexOutput) -> vec4<f32> {
    let mask_enabled = select(0.0, 1.0, skinned_model.texture_flags2.x > 0.5);
    let sampled = textureSample(mask_tex, albedo_sampler, material_uv(in));
    return vec4<f32>(1.0) + (sampled - vec4<f32>(1.0)) * mask_enabled;
}

fn roughness_metallic(in: VertexOutput) -> vec2<f32> {
    let mr = textureSample(metallic_roughness_tex, albedo_sampler, material_uv(in));
    let mr_active = select(0.0, 1.0, skinned_model.texture_flags.y > 0.5);
    let response = clamp(skinned_model.material_response.z, 0.0, 4.0);
    let mask = material_mask(in);
    let mask_active = select(0.0, 1.0, skinned_model.texture_flags2.x > 0.5);
    let roughness = clamp(skinned_model.material_params.y * (1.0 + (mr.g - 1.0) * mr_active * response) * (1.0 + (mask.r - 1.0) * mask_active * response), 0.04, 1.0);
    let metallic = clamp(skinned_model.material_params.z * (1.0 + (mr.b - 1.0) * mr_active * response) * (1.0 + (mask.g - 1.0) * mask_active * response), 0.0, 1.0);
    return vec2<f32>(roughness, metallic);
}

fn diffuse_shape(diff: f32, toon: bool) -> f32 {
    let ramp = textureSample(toon_ramp_tex, albedo_sampler, vec2<f32>(clamp(diff, 0.0, 1.0), 0.5)).r;
    let toon_step_0 = select(0.0, 0.28, diff > 0.08);
    let toon_step_1 = select(toon_step_0, 0.62, diff > 0.36);
    let toon_step_2 = select(toon_step_1, 1.0, diff > 0.72);
    let toon_response = clamp(skinned_model.material_response.w, 0.0, 1.0);
    let shaped = mix(diff, toon_step_2, select(0.0, toon_response, toon));
    return mix(shaped, ramp, clamp(skinned_model.texture_flags.w * toon_response, 0.0, 1.0));
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    let f = pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

fn pbr_direct(albedo: vec3<f32>, roughness: f32, metallic: f32, N: vec3<f32>, V: vec3<f32>, L: vec3<f32>, diffuse_factor: f32) -> vec3<f32> {
    let n_dot_l = max(dot(N, L), 0.0);
    let n_dot_v = max(dot(N, V), 0.001);
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }

    let H = normalize(L + V);
    let n_dot_h = max(dot(N, H), 0.0);
    let v_dot_h = max(dot(V, H), 0.0);
    let r = clamp(roughness, 0.04, 1.0);
    let alpha = max(r * r, 0.0025);
    let alpha2 = alpha * alpha;
    let d_denom = max(n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0, 0.001);
    let d = alpha2 / max(PI * d_denom * d_denom, 0.001);
    let k = ((r + 1.0) * (r + 1.0)) * 0.125;
    let g_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
    let g_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let f = fresnel_schlick(v_dot_h, f0);
    let specular = f * ((d * g_l * g_v) / max(4.0 * n_dot_l * n_dot_v, 0.001)) * n_dot_l;
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metallic) * albedo * diffuse_factor;
    return diffuse + specular;
}

fn emissive_value(in: VertexOutput) -> vec3<f32> {
    let factor = skinned_model.emissive_color.rgb * max(skinned_model.emissive_color.a, 1.0);
    let sampled = textureSample(emissive_tex, albedo_sampler, material_uv(in)).rgb * factor;
    return select(factor, sampled, skinned_model.texture_flags.z > 0.5);
}

fn shade(albedo: vec4<f32>, in: VertexOutput) -> vec4<f32> {
    let N = surface_normal(in);
    let V = normalize(camera.camera_pos - in.world_pos);
    let rm = roughness_metallic(in);
    let roughness = rm.x;
    let metallic = rm.y;
    let toon = skinned_model.material_params.w >= 0.5;

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

        let diff = diffuse_shape(max(dot(N, L), 0.0), toon);
        direct_color += pbr_direct(albedo.rgb, roughness, metallic, N, V, L, diff) * light_color * attenuation * shadow;
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

@fragment
fn fs_skinned(in: VertexOutput) -> FragmentOutput {
    return fragment_output(skinned_textured_fragment_color(in), in);
}

@fragment
fn fs_skinned_no_tex(in: VertexOutput) -> FragmentOutput {
    return fragment_output(skinned_untextured_fragment_color(in), in);
}

@fragment
fn fs_skinned_color(in: VertexOutput) -> @location(0) vec4<f32> {
    return skinned_textured_fragment_color(in);
}

@fragment
fn fs_skinned_no_tex_color(in: VertexOutput) -> @location(0) vec4<f32> {
    return skinned_untextured_fragment_color(in);
}
