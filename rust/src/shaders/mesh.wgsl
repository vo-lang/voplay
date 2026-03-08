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
};

// Light types: 0 = directional, 1 = point
struct LightData {
    position_or_dir: vec4<f32>, // xyz = position (point) or direction (dir), w = type (0/1)
    color_intensity: vec4<f32>, // rgb = color, a = intensity
};

struct LightUniform {
    ambient: vec4<f32>,   // rgb = ambient color, a = unused
    count: vec4<u32>,     // x = number of lights
    lights: array<LightData, 8>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> model: ModelUniform;
@group(2) @binding(0) var<uniform> light_uni: LightUniform;
@group(3) @binding(0) var albedo_tex: texture_2d<f32>;
@group(3) @binding(1) var albedo_sampler: sampler;

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
    let albedo = textureSample(albedo_tex, albedo_sampler, in.uv) * model.base_color;
    let N = normalize(in.world_normal);
    let V = normalize(camera.camera_pos - in.world_pos);

    // Ambient
    var color = light_uni.ambient.rgb * albedo.rgb;

    let num_lights = min(light_uni.count.x, 8u);
    for (var i = 0u; i < num_lights; i = i + 1u) {
        let light = light_uni.lights[i];
        let light_type = u32(light.position_or_dir.w);
        let light_color = light.color_intensity.rgb * light.color_intensity.a;

        var L: vec3<f32>;
        var attenuation = 1.0;

        if light_type == 0u {
            // Directional: position_or_dir.xyz is direction TO the light
            L = normalize(light.position_or_dir.xyz);
        } else {
            // Point
            let to_light = light.position_or_dir.xyz - in.world_pos;
            let dist = length(to_light);
            L = to_light / dist;
            attenuation = 1.0 / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }

        // Diffuse
        let diff = max(dot(N, L), 0.0);
        color += albedo.rgb * light_color * diff * attenuation;

        // Specular (Blinn-Phong)
        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), 32.0);
        color += light_color * spec * attenuation * 0.5;
    }

    return vec4<f32>(color, albedo.a);
}

// Fragment shader variant without texture (uses base_color only).
@fragment
fn fs_main_no_tex(in: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = model.base_color;
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

        let diff = max(dot(N, L), 0.0);
        color += albedo.rgb * light_color * diff * attenuation;

        let H = normalize(L + V);
        let spec = pow(max(dot(N, H), 0.0), 32.0);
        color += light_color * spec * attenuation * 0.5;
    }

    return vec4<f32>(color, albedo.a);
}
