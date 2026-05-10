// 3D mesh shader — PBR-style direct lighting.
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
    material_response: vec4<f32>,
    texture_flags2: vec4<f32>,
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
const PRIMITIVE_FLAG_WIND: u32 = 2u;
const PRIMITIVE_FLAG_BILLBOARD: u32 = 4u;
const PRIMITIVE_FLAG_Y_BILLBOARD: u32 = 8u;
const PRIMITIVE_FLAG_ATLAS_UV: u32 = 16u;

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
@group(3) @binding(7) var mask_tex: texture_2d<f32>;

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
    @location(9) instance_params: vec4<f32>,
    @location(10) material_response: vec4<f32>,
    @location(11) texture_flags2: vec4<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) receiver_mask: vec4<f32>,
    @location(2) surface_props: vec4<f32>,
};

fn alpha_cutout_enabled(in: VertexOutput) -> bool {
    let flags = u32(max(in.instance_params.x, 0.0) + 0.5);
    return (flags & (PRIMITIVE_FLAG_BILLBOARD | PRIMITIVE_FLAG_Y_BILLBOARD | PRIMITIVE_FLAG_ATLAS_UV)) != 0u;
}

fn surface_props(in: VertexOutput, procedural_detail: bool, front_facing: bool) -> vec4<f32> {
    let N = oriented_surface_normal(in, front_facing);
    let rm = roughness_metallic(in);
    var roughness = rm.x;
    if procedural_detail {
        let rough_noise = value_noise(in.world_pos.xz * 3.7 + in.uv * 5.0);
        roughness = clamp(roughness + (rough_noise - 0.5) * procedural_detail_strength(in) * 0.35, 0.04, 1.0);
    }
    return vec4<f32>(N * 0.5 + vec3<f32>(0.5), roughness);
}

fn fragment_output(color: vec4<f32>, in: VertexOutput, procedural_detail: bool, front_facing: bool) -> FragmentOutput {
    var out: FragmentOutput;
    out.color = color;
    out.receiver_mask = vec4<f32>(2.0 / 255.0, 0.0, 0.0, 1.0);
    out.surface_props = surface_props(in, procedural_detail, front_facing);
    return out;
}

fn textured_fragment_color(in: VertexOutput, front_facing: bool) -> vec4<f32> {
    var albedo = textureSample(albedo_tex, albedo_sampler, material_uv(in)) * in.base_color * in.vertex_color;
    albedo.a = albedo.a * material_mask(in).a;
    if alpha_clip_enabled(in) && albedo.a < 0.45 {
        discard;
    }
    return shade(albedo, in, false, front_facing);
}

fn untextured_fragment_color(in: VertexOutput, front_facing: bool) -> vec4<f32> {
    var albedo = in.base_color * in.vertex_color;
    albedo.a = albedo.a * material_mask(in).a;
    return shade(albedo, in, true, front_facing);
}

struct InstanceInput {
    @location(5) model_0: vec4<f32>,
    @location(6) model_1: vec4<f32>,
    @location(7) model_2: vec4<f32>,
    @location(8) model_3: vec4<f32>,
    @location(9) base_color: vec4<f32>,
    @location(10) material_params: vec4<f32>,
    @location(11) emissive_color: vec4<f32>,
    @location(12) texture_flags: vec4<f32>,
    @location(13) material_response: vec4<f32>,
    @location(14) texture_flags2: vec4<f32>,
    @location(15) instance_params: vec4<f32>,
};

struct PrimitiveInstanceInput {
    @location(5) model_0: vec4<f32>,
    @location(6) model_1: vec4<f32>,
    @location(7) model_2: vec4<f32>,
    @location(8) model_3: vec4<f32>,
    @location(9) base_color: vec4<f32>,
    @location(10) material_params: vec4<f32>,
    @location(11) emissive_color: vec4<f32>,
    @location(12) texture_flags: vec4<f32>,
    @location(13) instance_params: vec4<f32>,
    @location(14) atlas_uv: vec4<f32>,
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
    var sample_radius: i32 = 1;
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

fn material_uv(in: VertexOutput) -> vec2<f32> {
    let flags = u32(max(in.instance_params.x, 0.0) + 0.5);
    let uv_scale = select(in.material_params.x, 1.0, (flags & PRIMITIVE_FLAG_ATLAS_UV) != 0u);
    return in.uv * vec2<f32>(uv_scale, uv_scale);
}

fn alpha_clip_enabled(in: VertexOutput) -> bool {
    return alpha_cutout_enabled(in);
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

fn oriented_surface_normal(in: VertexOutput, front_facing: bool) -> vec3<f32> {
    var N = surface_normal(in);
    if alpha_cutout_enabled(in) {
        if !front_facing {
            N = -N;
        }
        N = normalize(N + vec3<f32>(0.0, 0.82, 0.0));
    }
    return N;
}

fn material_mask(in: VertexOutput) -> vec4<f32> {
    let mask_enabled = select(0.0, 1.0, in.texture_flags2.x > 0.5);
    let sampled = textureSample(mask_tex, albedo_sampler, material_uv(in));
    return vec4<f32>(1.0) + (sampled - vec4<f32>(1.0)) * mask_enabled;
}

fn roughness_metallic(in: VertexOutput) -> vec2<f32> {
    let mr = textureSample(metallic_roughness_tex, albedo_sampler, material_uv(in));
    let mr_active = select(0.0, 1.0, in.texture_flags.y > 0.5);
    let response = clamp(in.material_response.z, 0.0, 4.0);
    let mask = material_mask(in);
    let mask_active = select(0.0, 1.0, in.texture_flags2.x > 0.5);
    let roughness = clamp(in.material_params.y * (1.0 + (mr.g - 1.0) * mr_active * response) * (1.0 + (mask.r - 1.0) * mask_active * response), 0.04, 1.0);
    let metallic = clamp(in.material_params.z * (1.0 + (mr.b - 1.0) * mr_active * response) * (1.0 + (mask.g - 1.0) * mask_active * response), 0.0, 1.0);
    return vec2<f32>(roughness, metallic);
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn procedural_detail_strength(in: VertexOutput) -> f32 {
    let roughness = clamp(in.material_params.y, 0.0, 1.0);
    let uv_scale = max(in.material_params.x, 1.0);
    let material_detail = max(in.material_response.x, 0.0);
    return clamp(((roughness - 0.52) * 0.42 + (uv_scale - 1.0) * 0.055) * material_detail, 0.0, 1.0);
}

fn apply_procedural_surface_detail(albedo: vec4<f32>, in: VertexOutput, fallback_detail: bool) -> vec4<f32> {
    let fallback_macro = select(0.0, 0.35, fallback_detail);
    let macro_blend = max(in.material_response.y, fallback_macro);
    if macro_blend <= 0.001 {
        return albedo;
    }
    let strength = procedural_detail_strength(in) * macro_blend;
    if strength <= 0.001 {
        return albedo;
    }
    let p = in.world_pos.xz;
    let macro_noise = value_noise(p * 0.18 + in.uv * 0.37);
    let grain = value_noise(p * 2.35 + in.uv * 9.0);
    let fine = value_noise(p * 9.5 + vec2<f32>(macro_noise * 13.0, grain * 7.0));
    let variation = ((macro_noise - 0.5) * 0.9 + (grain - 0.5) * 0.48 + (fine - 0.5) * 0.22) * strength;
    let warm_cool = vec3<f32>(1.0 + variation * 0.42, 1.0 + variation * 0.18, 1.0 - variation * 0.24);
    return vec4<f32>(max(albedo.rgb * (1.0 + variation) * warm_cool, vec3<f32>(0.0)), albedo.a);
}

fn diffuse_shape(diff: f32, toon: bool, in: VertexOutput) -> f32 {
    let ramp = textureSample(toon_ramp_tex, albedo_sampler, vec2<f32>(clamp(diff, 0.0, 1.0), 0.5)).r;
    let toon_step_0 = select(0.0, 0.28, diff > 0.08);
    let toon_step_1 = select(toon_step_0, 0.62, diff > 0.36);
    let toon_step_2 = select(toon_step_1, 1.0, diff > 0.72);
    let toon_response = clamp(in.material_response.w, 0.0, 1.0);
    let shaped = mix(diff, toon_step_2, select(0.0, toon_response, toon));
    return mix(shaped, ramp, clamp(in.texture_flags.w * toon_response, 0.0, 1.0));
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
    let factor = in.emissive_color.rgb * max(in.emissive_color.a, 1.0);
    let sampled = textureSample(emissive_tex, albedo_sampler, material_uv(in)).rgb * factor;
    return select(factor, sampled, in.texture_flags.z > 0.5);
}

fn shade(input_albedo: vec4<f32>, in: VertexOutput, procedural_detail: bool, front_facing: bool) -> vec4<f32> {
    let albedo = apply_procedural_surface_detail(input_albedo, in, procedural_detail);
    let N = oriented_surface_normal(in, front_facing);
    let V = normalize(camera.camera_pos - in.world_pos);
    let rm = roughness_metallic(in);
    var roughness = rm.x;
    if procedural_detail {
        let rough_noise = value_noise(in.world_pos.xz * 3.7 + in.uv * 5.0);
        roughness = clamp(roughness + (rough_noise - 0.5) * procedural_detail_strength(in) * 0.35, 0.04, 1.0);
    }
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
    out.material_response = model.material_response;
    out.texture_flags2 = model.texture_flags2;
    out.vertex_color = in.color;
    out.instance_params = vec4<f32>(0.0);
    return out;
}

fn normalize_or(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let len_sq = dot(v, v);
    if len_sq <= 0.0000001 {
        return fallback;
    }
    return v * inverseSqrt(len_sq);
}

fn normal_from_model_matrix(model_mat: mat4x4<f32>, local_dir: vec3<f32>) -> vec3<f32> {
    let axis_x = model_mat[0].xyz;
    let axis_y = model_mat[1].xyz;
    let axis_z = model_mat[2].xyz;
    let normal_x = axis_x / max(dot(axis_x, axis_x), 0.000001);
    let normal_y = axis_y / max(dot(axis_y, axis_y), 0.000001);
    let normal_z = axis_z / max(dot(axis_z, axis_z), 0.000001);
    return normalize_or(normal_x * local_dir.x + normal_y * local_dir.y + normal_z * local_dir.z, vec3<f32>(0.0, 1.0, 0.0));
}

fn direction_from_model_matrix(model_mat: mat4x4<f32>, local_dir: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let transformed = model_mat[0].xyz * local_dir.x + model_mat[1].xyz * local_dir.y + model_mat[2].xyz * local_dir.z;
    return normalize_or(transformed, fallback);
}

fn build_instanced_vertex(
    in: VertexInput,
    model_mat: mat4x4<f32>,
    base_color: vec4<f32>,
    material_params: vec4<f32>,
    emissive_color: vec4<f32>,
    texture_flags: vec4<f32>,
    material_response: vec4<f32>,
    texture_flags2: vec4<f32>,
    instance_params: vec4<f32>,
    atlas_rect: vec4<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    let center = model_mat[3].xyz;
    let instance_flags = u32(max(instance_params.x, 0.0) + 0.5);
    let lod_near = max(instance_params.y, 0.0);
    let lod_far = max(instance_params.z, 0.0);
    let wind_strength = max(instance_params.w, 0.0);
    let center_dist = distance(center, camera.camera_pos);
    let lod_hidden = (lod_near > 0.0 && center_dist < lod_near) || (lod_far > 0.0 && center_dist > lod_far);
    let billboard = (instance_flags & (PRIMITIVE_FLAG_BILLBOARD | PRIMITIVE_FLAG_Y_BILLBOARD)) != 0u;
    var local_pos = in.position;
    if (instance_flags & PRIMITIVE_FLAG_WIND) != 0u && wind_strength > 0.0 && !lod_hidden {
        let frame_time = f32(light_uni.debug_params.y) * 0.016;
        let phase = dot(center.xz, vec2<f32>(0.37, 0.61));
        var blade_weight = clamp((in.position.y + 0.5) * 0.75, 0.0, 1.0);
        if billboard {
            blade_weight = clamp(in.position.z + 0.5, 0.0, 1.0);
        }
        let sway = sin(frame_time * 1.7 + phase + in.position.y * 2.1) * wind_strength * blade_weight;
        local_pos.x = local_pos.x + sway;
        local_pos.z = local_pos.z + sway * 0.35;
    }
    var world_pos = model_mat * vec4<f32>(local_pos, 1.0);
    var world_normal = normal_from_model_matrix(model_mat, in.normal);
    var world_tangent = vec4<f32>(direction_from_model_matrix(model_mat, in.tangent.xyz, vec3<f32>(1.0, 0.0, 0.0)), in.tangent.w);
    if billboard && !lod_hidden {
        let yaw_locked = (instance_flags & PRIMITIVE_FLAG_Y_BILLBOARD) != 0u;
        var to_camera = camera.camera_pos - center;
        if yaw_locked {
            to_camera.y = 0.0;
        }
        if length(to_camera) < 0.0001 {
            to_camera = vec3<f32>(0.0, 0.0, 1.0);
        }
        let forward = normalize(to_camera);
        var right = cross(vec3<f32>(0.0, 1.0, 0.0), forward);
        if length(right) < 0.0001 {
            right = vec3<f32>(1.0, 0.0, 0.0);
        }
        right = normalize(right);
        var up = vec3<f32>(0.0, 1.0, 0.0);
        if !yaw_locked {
            up = normalize(cross(forward, right));
        }
        let scale_x = max(length(model_mat[0].xyz), 0.0001);
        let scale_y = max(length(model_mat[1].xyz), 0.0001);
        let scale_z = max(length(model_mat[2].xyz), 0.0001);
        world_pos = vec4<f32>(
            center + right * local_pos.x * scale_x + up * local_pos.z * scale_z + vec3<f32>(0.0, 1.0, 0.0) * local_pos.y * scale_y,
            1.0
        );
        world_normal = forward;
        world_tangent = vec4<f32>(right, 1.0);
    }
    out.clip_position = camera.view_proj * world_pos;
    if lod_hidden {
        out.clip_position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    }
    out.world_pos = world_pos.xyz;
    out.world_normal = world_normal;
    out.world_tangent = world_tangent;
    var atlas_uv = atlas_rect;
    if atlas_uv.z <= 0.0 || atlas_uv.w <= 0.0 {
        atlas_uv = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    }
    out.uv = atlas_uv.xy + in.uv * atlas_uv.zw;
    out.base_color = base_color;
    out.material_params = material_params;
    out.emissive_color = emissive_color;
    out.texture_flags = texture_flags;
    out.material_response = material_response;
    out.texture_flags2 = texture_flags2;
    out.vertex_color = in.color;
    out.instance_params = instance_params;
    return out;
}

@vertex
fn vs_instanced(in: VertexInput, instance: InstanceInput) -> VertexOutput {
    let model_mat = mat4x4<f32>(instance.model_0, instance.model_1, instance.model_2, instance.model_3);
    return build_instanced_vertex(
        in,
        model_mat,
        instance.base_color,
        instance.material_params,
        instance.emissive_color,
        instance.texture_flags,
        instance.material_response,
        instance.texture_flags2,
        instance.instance_params,
        vec4<f32>(0.0, 0.0, 1.0, 1.0),
    );
}

@vertex
fn vs_instanced_primitive(in: VertexInput, instance: PrimitiveInstanceInput) -> VertexOutput {
    let model_mat = mat4x4<f32>(instance.model_0, instance.model_1, instance.model_2, instance.model_3);
    return build_instanced_vertex(
        in,
        model_mat,
        instance.base_color,
        instance.material_params,
        instance.emissive_color,
        instance.texture_flags,
        vec4<f32>(1.0, 0.0, 1.0, 1.0),
        vec4<f32>(0.0),
        instance.instance_params,
        instance.atlas_uv,
    );
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> FragmentOutput {
    return fragment_output(textured_fragment_color(in, front_facing), in, false, front_facing);
}

// Fragment shader variant without texture (uses base_color only).
@fragment
fn fs_main_no_tex(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> FragmentOutput {
    return fragment_output(untextured_fragment_color(in, front_facing), in, true, front_facing);
}

@fragment
fn fs_instanced(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> FragmentOutput {
    return fragment_output(textured_fragment_color(in, front_facing), in, false, front_facing);
}

@fragment
fn fs_instanced_no_tex(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> FragmentOutput {
    return fragment_output(untextured_fragment_color(in, front_facing), in, true, front_facing);
}

@fragment
fn fs_main_color(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    return textured_fragment_color(in, front_facing);
}

@fragment
fn fs_main_no_tex_color(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    return untextured_fragment_color(in, front_facing);
}

@fragment
fn fs_instanced_color(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    return textured_fragment_color(in, front_facing);
}

@fragment
fn fs_instanced_no_tex_color(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    return untextured_fragment_color(in, front_facing);
}
