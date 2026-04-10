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
};

// Light types: 0 = directional, 1 = point
struct LightData {
    position_or_dir: vec4<f32>, // xyz = position (point) or direction (dir), w = type (0/1)
    color_intensity: vec4<f32>, // rgb = color, a = intensity
};

struct LightUniform {
    ambient: vec4<f32>,   // rgb = ambient color, a = unused
    count: vec4<u32>,     // x = number of lights, y = fog mode
    lights: array<LightData, 8>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    shadow_vp: mat4x4<f32>,
    shadow_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> model: ModelUniform;
@group(2) @binding(0) var<uniform> light_uni: LightUniform;
@group(3) @binding(0) var albedo_tex: texture_2d<f32>;
@group(3) @binding(1) var albedo_sampler: sampler;
@group(3) @binding(2) var shadow_tex: texture_depth_2d;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
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
    return visibility / 9.0;
}

fn shade(albedo: vec4<f32>, in: VertexOutput) -> vec4<f32> {
    let N = normalize(in.world_normal);
    let V = normalize(camera.camera_pos - in.world_pos);

    var color = light_uni.ambient.rgb * albedo.rgb;
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
        }

        let diff = max(dot(N, L), 0.0);
        color += albedo.rgb * light_color * diff * attenuation * shadow;

        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), 32.0);
        color += light_color * spec * attenuation * 0.5 * shadow;
    }

    color = apply_fog(color, in.world_pos);
    return vec4<f32>(color, albedo.a);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = model.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_proj * world_pos;
    out.world_pos = world_pos.xyz;
    out.world_normal = normalize((model.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv_scale = model.material_params.x;
    let albedo = textureSample(albedo_tex, albedo_sampler, in.uv * vec2<f32>(uv_scale, uv_scale)) * model.base_color;
    return shade(albedo, in);
}

// Fragment shader variant without texture (uses base_color only).
@fragment
fn fs_main_no_tex(in: VertexOutput) -> @location(0) vec4<f32> {
    return shade(model.base_color, in);
}
