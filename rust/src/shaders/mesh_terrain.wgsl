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
    shadow_cascade_vp: array<mat4x4<f32>, 4>,
    shadow_cascade_splits: vec4<f32>,
    shadow_params: vec4<f32>,
    shadow_params2: vec4<f32>,
    color_params: vec4<f32>,
    debug_params: vec4<u32>,
};

struct TerrainMaterialUniform {
    params0: vec4<f32>,
    params1: vec4<f32>,
    params2: vec4<f32>,
    params3: vec4<f32>,
};

const RENDER_DEBUG_LIT: u32 = 0u;
const RENDER_DEBUG_ALBEDO: u32 = 1u;
const RENDER_DEBUG_NORMAL: u32 = 2u;
const RENDER_DEBUG_ROUGHNESS: u32 = 3u;
const RENDER_DEBUG_METALLIC: u32 = 4u;
const RENDER_DEBUG_SHADOW: u32 = 5u;
const RENDER_DEBUG_DIRECT: u32 = 6u;
const RENDER_DEBUG_AMBIENT: u32 = 7u;
const RENDER_DEBUG_TERRAIN_CONTROL: u32 = 8u;
const RENDER_DEBUG_TERRAIN_SLOPE: u32 = 9u;
const RENDER_DEBUG_TERRAIN_HEIGHT: u32 = 10u;
const RENDER_DEBUG_TERRAIN_CURVATURE: u32 = 11u;
const RENDER_DEBUG_TERRAIN_MACRO: u32 = 12u;
const PI: f32 = 3.14159265359;

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
@group(3) @binding(16) var<uniform> terrain_material: TerrainMaterialUniform;

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

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) receiver_mask: vec4<f32>,
    @location(2) surface_props: vec4<f32>,
};

fn fragment_output(color: vec4<f32>, normal: vec3<f32>, roughness: f32) -> FragmentOutput {
    var out: FragmentOutput;
    out.color = color;
    out.receiver_mask = vec4<f32>(1.0 / 255.0, 0.0, 0.0, 1.0);
    out.surface_props = vec4<f32>(normalize(normal) * 0.5 + vec3<f32>(0.5), clamp(roughness, 0.04, 1.0));
    return out;
}

struct TerrainFragmentResult {
    color: vec4<f32>,
    normal: vec3<f32>,
    roughness: f32,
};

fn terrain_fragment_result(color: vec4<f32>, normal: vec3<f32>, roughness: f32) -> TerrainFragmentResult {
    var out: TerrainFragmentResult;
    out.color = color;
    out.normal = normal;
    out.roughness = roughness;
    return out;
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

fn hash21(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
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

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < 5; i = i + 1) {
        v += value_noise(q) * amp;
        q = mat2x2<f32>(1.72, -0.58, 0.58, 1.72) * q + vec2<f32>(13.7, -9.2);
        amp *= 0.5;
    }
    return v;
}

fn rotate_uv(uv: vec2<f32>, angle: f32) -> vec2<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec2<f32>(uv.x * c - uv.y * s, uv.x * s + uv.y * c);
}

fn terrain_layer_uv(uv: vec2<f32>, world_pos: vec3<f32>, world_normal: vec3<f32>, uv_scale: f32, salt: f32) -> vec2<f32> {
    let n = normalize(world_normal);
    let steepness = smoothstep(terrain_material.params1.x, terrain_material.params1.y, 1.0 - clamp(abs(n.y), 0.0, 1.0));
    let projected_scale = max(uv_scale, 0.0001) * 0.0115;
    let x_side_uv = world_pos.zy * projected_scale;
    let z_side_uv = world_pos.xy * projected_scale;
    let side_weight = max(abs(n.x) + abs(n.z), 0.0001);
    let side_uv = (x_side_uv * abs(n.x) + z_side_uv * abs(n.z)) / side_weight;
    let projected_uv = rotate_uv(side_uv + vec2<f32>(salt * 3.17, -salt * 2.41), 0.19 + salt * 0.09);
    return mix(uv, projected_uv, steepness * 0.88);
}

fn terrain_variant_sample(tex: texture_2d<f32>, uv: vec2<f32>, cell: vec2<f32>, salt: f32) -> vec4<f32> {
    let angle = 0.35 + salt * 0.11 + hash21(cell + vec2<f32>(salt * 5.7, 19.3)) * 1.35;
    let scale = mix(1.18, 1.46, hash21(cell + vec2<f32>(-11.7, salt * 3.9)));
    let jitter = vec2<f32>(
        hash21(cell + vec2<f32>(salt, 1.0)),
        hash21(cell + vec2<f32>(2.0, salt)),
    ) - vec2<f32>(0.5);
    return textureSample(tex, layer_sampler, rotate_uv(uv * scale + jitter * 0.20, angle));
}

fn sample_layer_albedo(tex: texture_2d<f32>, uv: vec2<f32>, world_xz: vec2<f32>, salt: f32) -> vec4<f32> {
    let c0 = textureSample(tex, layer_sampler, uv);
    let anti_tile = clamp(terrain_material.params2.x, 0.0, 1.0);
    if anti_tile <= 0.001 {
        return c0;
    }

    let grid = world_xz * 0.028 + vec2<f32>(salt, -salt * 0.37);
    let cell = floor(grid);
    let f = fract(grid);
    let w = f * f * (vec2<f32>(3.0) - f * 2.0);
    let c00 = terrain_variant_sample(tex, uv, cell, salt);
    let c10 = terrain_variant_sample(tex, uv, cell + vec2<f32>(1.0, 0.0), salt);
    let c01 = terrain_variant_sample(tex, uv, cell + vec2<f32>(0.0, 1.0), salt);
    let c11 = terrain_variant_sample(tex, uv, cell + vec2<f32>(1.0, 1.0), salt);
    let smoothed_variant = mix(mix(c00, c10, w.x), mix(c01, c11, w.x), w.y);
    let anti_tiled = mix(c0, smoothed_variant, 0.48);
    return mix(c0, anti_tiled, anti_tile);
}

fn slope_adjusted_weights(weights_raw: vec4<f32>, world_normal: vec3<f32>) -> vec4<f32> {
    var weights = weights_raw / max(weights_raw.r + weights_raw.g + weights_raw.b + weights_raw.a, 0.00001);
    let slope = smoothstep(terrain_material.params1.x, terrain_material.params1.y, 1.0 - clamp(world_normal.y, 0.0, 1.0));
    let exposed = slope * (weights.r * 0.32 + weights.g * 0.22);
    weights.r = max(0.0, weights.r - exposed * 0.68);
    weights.g = max(0.0, weights.g - exposed * 0.32);
    weights.b += exposed * max(terrain_material.params1.z, 0.0);
    weights.a += exposed * max(terrain_material.params1.w, 0.0);
    let total = max(weights.r + weights.g + weights.b + weights.a, 0.00001);
    return weights / total;
}

fn height_curvature_adjusted_weights(weights_raw: vec4<f32>, in: VertexOutput) -> vec4<f32> {
    var weights = weights_raw;
    let height_strength = clamp(terrain_material.params3.x, 0.0, 1.0);
    if height_strength > 0.001 {
        let low = terrain_material.params3.y;
        let high = max(terrain_material.params3.z, low + 0.001);
        let h = smoothstep(low, high, in.world_pos.y);
        let lowland = (1.0 - h) * height_strength;
        let upland = h * height_strength;
        weights.r += upland * 0.08;
        weights.g += upland * 0.10;
        weights.b += lowland * 0.12;
        weights.a += upland * 0.04;
    }

    let curvature_strength = max(terrain_material.params3.w, 0.0);
    if curvature_strength > 0.001 {
        let n = normalize(in.world_normal);
        let curvature = clamp((length(dpdx(n)) + length(dpdy(n))) * 10.0, 0.0, 1.0) * curvature_strength;
        weights.r = max(0.0, weights.r - curvature * 0.08);
        weights.g = max(0.0, weights.g - curvature * 0.04);
        weights.b += curvature * 0.10;
        weights.a += curvature * 0.22;
    }

    let total = max(weights.r + weights.g + weights.b + weights.a, 0.00001);
    return weights / total;
}

fn terrain_height_color_grade(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let height_strength = clamp(terrain_material.params3.x, 0.0, 1.0);
    if height_strength <= 0.001 {
        return color;
    }
    let low = terrain_material.params3.y;
    let high = max(terrain_material.params3.z, low + 0.001);
    let h = smoothstep(low, high, world_pos.y);
    let low_tint = vec3<f32>(0.90, 0.95, 1.0);
    let high_tint = vec3<f32>(1.02, 1.0, 0.94);
    let tint = mix(low_tint, high_tint, h);
    return mix(color, color * tint, height_strength * 0.42);
}

fn terrain_macro_light_dir() -> vec3<f32> {
    var dir = normalize(vec3<f32>(-0.42, 0.82, 0.32));
    let num_lights = min(light_uni.count.x, 8u);
    if num_lights > 0u {
        let index = min(light_uni.count.z, num_lights - 1u);
        let light = light_uni.lights[index];
        if u32(light.position_or_dir.w) == 0u {
            dir = normalize(light.position_or_dir.xyz);
        }
    }
    return dir;
}

fn terrain_macro_modulation(world_pos: vec3<f32>, world_normal: vec3<f32>, weights: vec4<f32>) -> vec3<f32> {
    let p = world_pos.xz;
    let macro_scale = max(terrain_material.params0.x, 0.0001);
    let broad = fbm(p * (0.0065 * macro_scale));
    let hill_band = fbm(p * (0.015 * macro_scale) + vec2<f32>(7.1, -2.8));
    let sun = clamp(dot(normalize(world_normal), terrain_macro_light_dir()) * 0.5 + 0.5, 0.0, 1.0);
    let exposed = clamp(weights.b + weights.a, 0.0, 1.0);
    let sun_band = smoothstep(0.28, 0.92, sun);
    let broad_luma = 0.78 + broad * 0.18 + hill_band * 0.08;
    let directional_luma = mix(0.82, 1.08, sun_band);
    let cool_shadow = mix(vec3<f32>(0.88, 0.94, 1.04), vec3<f32>(1.0), sun_band);
    let warm_highlight = mix(vec3<f32>(1.0), vec3<f32>(1.035, 1.0, 0.93), smoothstep(0.58, 1.0, sun));
    let exposed_bias = mix(vec3<f32>(0.96, 1.015, 0.97), vec3<f32>(1.035, 0.99, 0.93), exposed * 0.65 + hill_band * 0.12);
    return cool_shadow * warm_highlight * exposed_bias * vec3<f32>(clamp(broad_luma * directional_luma, 0.68, 1.16));
}

fn terrain_detail_luma(albedo: vec3<f32>) -> f32 {
    return dot(albedo, vec3<f32>(0.299, 0.587, 0.114));
}

fn terrain_surface_normal(in: VertexOutput, weights: vec4<f32>, uv0: vec2<f32>, uv1: vec2<f32>, uv2: vec2<f32>, uv3: vec2<f32>) -> vec3<f32> {
    let N = normalize(in.world_normal);
    let T_raw = in.world_tangent.xyz;
    let T = normalize(T_raw - N * dot(N, T_raw));
    let B = normalize(cross(N, T) * in.world_tangent.w);
    let detail_fade = 1.0 - smoothstep(terrain_material.params2.z, terrain_material.params2.w, distance(in.world_pos, camera.camera_pos));

    let s0 = textureSample(layer0_normal_tex, layer_sampler, uv0).xyz * 2.0 - vec3<f32>(1.0);
    let s1 = textureSample(layer1_normal_tex, layer_sampler, uv1).xyz * 2.0 - vec3<f32>(1.0);
    let s2 = textureSample(layer2_normal_tex, layer_sampler, uv2).xyz * 2.0 - vec3<f32>(1.0);
    let s3 = textureSample(layer3_normal_tex, layer_sampler, uv3).xyz * 2.0 - vec3<f32>(1.0);
    let n0 = normalize(vec3<f32>(s0.xy * model.texture_flags.x * detail_fade, s0.z));
    let n1 = normalize(vec3<f32>(s1.xy * model.texture_flags.y * detail_fade, s1.z));
    let n2 = normalize(vec3<f32>(s2.xy * model.texture_flags.z * detail_fade, s2.z));
    let n3 = normalize(vec3<f32>(s3.xy * model.texture_flags.w * detail_fade, s3.z));
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

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    let f = pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

fn pbr_direct(albedo: vec3<f32>, roughness: f32, metallic: f32, N: vec3<f32>, V: vec3<f32>, L: vec3<f32>) -> vec3<f32> {
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
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metallic) * albedo * n_dot_l;
    return diffuse + specular;
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

        direct_color += pbr_direct(albedo.rgb, roughness, metallic, N, V, L) * light_color * attenuation * shadow;
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

fn terrain_fragment_color(in: VertexOutput) -> TerrainFragmentResult {
    let weights_raw = textureSample(control_tex, control_sampler, in.uv);
    let weights = height_curvature_adjusted_weights(slope_adjusted_weights(weights_raw, normalize(in.world_normal)), in);

    let uv0 = in.uv * vec2<f32>(model.material_params.x, model.material_params.x);
    let uv1 = in.uv * vec2<f32>(model.material_params.y, model.material_params.y);
    let uv2 = in.uv * vec2<f32>(model.material_params.z, model.material_params.z);
    let uv3 = in.uv * vec2<f32>(model.material_params.w, model.material_params.w);

    let layer_uv0 = terrain_layer_uv(uv0, in.world_pos, in.world_normal, model.material_params.x, 1.0);
    let layer_uv1 = terrain_layer_uv(uv1, in.world_pos, in.world_normal, model.material_params.y, 2.0);
    let layer_uv2 = terrain_layer_uv(uv2, in.world_pos, in.world_normal, model.material_params.z, 3.0);
    let layer_uv3 = terrain_layer_uv(uv3, in.world_pos, in.world_normal, model.material_params.w, 4.0);

    let world_xz = in.world_pos.xz;
    let c0 = sample_layer_albedo(layer0_tex, layer_uv0, world_xz, 1.0);
    let c1 = sample_layer_albedo(layer1_tex, layer_uv1, world_xz, 2.0);
    let c2 = sample_layer_albedo(layer2_tex, layer_uv2, world_xz, 3.0);
    let c3 = sample_layer_albedo(layer3_tex, layer_uv3, world_xz, 4.0);
    let tiled = c0 * weights.r + c1 * weights.g + c2 * weights.b + c3 * weights.a;
    let detail_fade = 1.0 - smoothstep(terrain_material.params0.z, terrain_material.params0.w, distance(in.world_pos, camera.camera_pos));
    let detail = clamp(0.76 + terrain_detail_luma(tiled.rgb) * 0.38, 0.62, 1.12);
    let macro_strength = clamp(terrain_material.params0.y, 0.0, 1.0);
    let detail_strength = clamp(terrain_material.params2.y, 0.0, 2.0);
    let macro_modulation = terrain_macro_modulation(in.world_pos, normalize(in.world_normal), weights);
    let material_rgb = mix(tiled.rgb, tiled.rgb * macro_modulation, macro_strength);
    let stylized_rgb = terrain_height_color_grade(mix(material_rgb, material_rgb * detail, detail_fade * detail_strength), in.world_pos);
    let albedo = vec4<f32>(stylized_rgb, tiled.a) * model.base_color;
    let normal = terrain_surface_normal(in, weights, layer_uv0, layer_uv1, layer_uv2, layer_uv3);
    let rm = terrain_roughness_metallic(weights, layer_uv0, layer_uv1, layer_uv2, layer_uv3);
    let debug_mode = light_uni.debug_params.x;
    if debug_mode == RENDER_DEBUG_TERRAIN_CONTROL {
        return terrain_fragment_result(vec4<f32>(weights.rgb, 1.0), normal, rm.x);
    }
    if debug_mode == RENDER_DEBUG_TERRAIN_SLOPE {
        let slope = smoothstep(terrain_material.params1.x, terrain_material.params1.y, 1.0 - clamp(normalize(in.world_normal).y, 0.0, 1.0));
        return terrain_fragment_result(vec4<f32>(vec3<f32>(slope), 1.0), normal, rm.x);
    }
    if debug_mode == RENDER_DEBUG_TERRAIN_HEIGHT {
        let low = terrain_material.params3.y;
        let high = max(terrain_material.params3.z, low + 0.001);
        let h = smoothstep(low, high, in.world_pos.y);
        return terrain_fragment_result(vec4<f32>(vec3<f32>(h), 1.0), normal, rm.x);
    }
    if debug_mode == RENDER_DEBUG_TERRAIN_CURVATURE {
        let n = normalize(in.world_normal);
        let curvature = clamp((length(dpdx(n)) + length(dpdy(n))) * 10.0, 0.0, 1.0);
        return terrain_fragment_result(vec4<f32>(vec3<f32>(curvature), 1.0), normal, rm.x);
    }
    if debug_mode == RENDER_DEBUG_TERRAIN_MACRO {
        return terrain_fragment_result(vec4<f32>(clamp(tiled.rgb * macro_modulation, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0), normal, rm.x);
    }
    return terrain_fragment_result(shade(albedo, normal, rm.x, rm.y, in), normal, rm.x);
}

@fragment
fn fs_main_terrain(in: VertexOutput) -> FragmentOutput {
    let result = terrain_fragment_color(in);
    return fragment_output(result.color, result.normal, result.roughness);
}

@fragment
fn fs_main_terrain_color(in: VertexOutput) -> @location(0) vec4<f32> {
    return terrain_fragment_color(in).color;
}
