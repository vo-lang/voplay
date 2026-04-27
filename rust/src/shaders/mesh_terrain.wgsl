struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos: vec3<f32>,
    _pad: f32,
};

struct ModelUniform {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    base_color: vec4<f32>,
    material_params: vec4<f32>,
    emissive_color: vec4<f32>,
    texture_flags: vec4<f32>,
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
@group(3) @binding(0) var control_tex: texture_2d<f32>;
@group(3) @binding(1) var control_sampler: sampler;
@group(3) @binding(2) var shadow_tex: texture_depth_2d;
@group(3) @binding(3) var layer_sampler: sampler;
@group(3) @binding(4) var layer0_tex: texture_2d<f32>;
@group(3) @binding(5) var layer1_tex: texture_2d<f32>;
@group(3) @binding(6) var layer2_tex: texture_2d<f32>;
@group(3) @binding(7) var layer3_tex: texture_2d<f32>;
@group(3) @binding(8) var layer0_normal_tex: texture_2d<f32>;
@group(3) @binding(9) var layer1_normal_tex: texture_2d<f32>;
@group(3) @binding(10) var layer2_normal_tex: texture_2d<f32>;
@group(3) @binding(11) var layer3_normal_tex: texture_2d<f32>;
@group(3) @binding(12) var layer0_mr_tex: texture_2d<f32>;
@group(3) @binding(13) var layer1_mr_tex: texture_2d<f32>;
@group(3) @binding(14) var layer2_mr_tex: texture_2d<f32>;
@group(3) @binding(15) var layer3_mr_tex: texture_2d<f32>;

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
    @location(3) world_tangent: vec4<f32>,
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

fn terrain_surface_normal(in: VertexOutput, weights: vec4<f32>, uv0: vec2<f32>, uv1: vec2<f32>, uv2: vec2<f32>, uv3: vec2<f32>) -> vec3<f32> {
    let N = normalize(in.world_normal);
    let T_raw = in.world_tangent.xyz;
    let T = normalize(T_raw - N * dot(N, T_raw));
    let B = normalize(cross(N, T) * in.world_tangent.w);

    let s0 = textureSample(layer0_normal_tex, layer_sampler, uv0).xyz * 2.0 - vec3<f32>(1.0);
    let s1 = textureSample(layer1_normal_tex, layer_sampler, uv1).xyz * 2.0 - vec3<f32>(1.0);
    let s2 = textureSample(layer2_normal_tex, layer_sampler, uv2).xyz * 2.0 - vec3<f32>(1.0);
    let s3 = textureSample(layer3_normal_tex, layer_sampler, uv3).xyz * 2.0 - vec3<f32>(1.0);
    let n0 = normalize(vec3<f32>(s0.xy * model.texture_flags.x, s0.z));
    let n1 = normalize(vec3<f32>(s1.xy * model.texture_flags.y, s1.z));
    let n2 = normalize(vec3<f32>(s2.xy * model.texture_flags.z, s2.z));
    let n3 = normalize(vec3<f32>(s3.xy * model.texture_flags.w, s3.z));
    let tangent_normal = normalize(n0 * weights.r + n1 * weights.g + n2 * weights.b + n3 * weights.a);
    return normalize(mat3x3<f32>(T, B, N) * tangent_normal);
}

fn terrain_roughness_metallic(weights: vec4<f32>, uv0: vec2<f32>, uv1: vec2<f32>, uv2: vec2<f32>, uv3: vec2<f32>) -> vec2<f32> {
    let mr0 = textureSample(layer0_mr_tex, layer_sampler, uv0);
    let mr1 = textureSample(layer1_mr_tex, layer_sampler, uv1);
    let mr2 = textureSample(layer2_mr_tex, layer_sampler, uv2);
    let mr3 = textureSample(layer3_mr_tex, layer_sampler, uv3);
    let rm0 = vec2<f32>(
        select(0.72, clamp(mr0.g, 0.04, 1.0), model.emissive_color.x > 0.5),
        select(0.0, clamp(mr0.b, 0.0, 1.0), model.emissive_color.x > 0.5),
    );
    let rm1 = vec2<f32>(
        select(0.72, clamp(mr1.g, 0.04, 1.0), model.emissive_color.y > 0.5),
        select(0.0, clamp(mr1.b, 0.0, 1.0), model.emissive_color.y > 0.5),
    );
    let rm2 = vec2<f32>(
        select(0.72, clamp(mr2.g, 0.04, 1.0), model.emissive_color.z > 0.5),
        select(0.0, clamp(mr2.b, 0.0, 1.0), model.emissive_color.z > 0.5),
    );
    let rm3 = vec2<f32>(
        select(0.72, clamp(mr3.g, 0.04, 1.0), model.emissive_color.w > 0.5),
        select(0.0, clamp(mr3.b, 0.0, 1.0), model.emissive_color.w > 0.5),
    );
    return rm0 * weights.r + rm1 * weights.g + rm2 * weights.b + rm3 * weights.a;
}

fn shade(albedo: vec4<f32>, normal: vec3<f32>, roughness: f32, metallic: f32, in: VertexOutput) -> vec4<f32> {
    let N = normalize(normal);
    let V = normalize(camera.camera_pos - in.world_pos);

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

        let diff = max(dot(N, L), 0.0);
        direct_color += albedo.rgb * light_color * diff * attenuation * shadow;

        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), 32.0);
        direct_color += light_color * spec * attenuation * 0.5 * shadow;
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
    return out;
}

@fragment
fn fs_main_terrain(in: VertexOutput) -> @location(0) vec4<f32> {
    let weights_raw = textureSample(control_tex, control_sampler, in.uv);
    let weight_sum = max(weights_raw.r + weights_raw.g + weights_raw.b + weights_raw.a, 0.00001);
    let weights = weights_raw / weight_sum;

    let uv0 = in.uv * vec2<f32>(model.material_params.x, model.material_params.x);
    let uv1 = in.uv * vec2<f32>(model.material_params.y, model.material_params.y);
    let uv2 = in.uv * vec2<f32>(model.material_params.z, model.material_params.z);
    let uv3 = in.uv * vec2<f32>(model.material_params.w, model.material_params.w);

    let c0 = textureSample(layer0_tex, layer_sampler, uv0);
    let c1 = textureSample(layer1_tex, layer_sampler, uv1);
    let c2 = textureSample(layer2_tex, layer_sampler, uv2);
    let c3 = textureSample(layer3_tex, layer_sampler, uv3);
    let albedo = (c0 * weights.r + c1 * weights.g + c2 * weights.b + c3 * weights.a) * model.base_color;
    let normal = terrain_surface_normal(in, weights, uv0, uv1, uv2, uv3);
    let rm = terrain_roughness_metallic(weights, uv0, uv1, uv2, uv3);
    return shade(albedo, normal, rm.x, rm.y, in);
}
