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
};

struct LightData {
    position_or_dir: vec4<f32>,
    color_intensity: vec4<f32>,
};

struct LightUniform {
    ambient: vec4<f32>,
    count: vec4<u32>,
    lights: array<LightData, 8>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    shadow_vp: mat4x4<f32>,
    shadow_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> model: ModelUniform;
@group(2) @binding(0) var<uniform> light_uni: LightUniform;
@group(3) @binding(0) var control_tex: texture_2d<f32>;
@group(3) @binding(1) var control_sampler: sampler;
@group(3) @binding(2) var shadow_tex: texture_depth_2d;
@group(3) @binding(3) var shadow_sampler: sampler_comparison;
@group(3) @binding(4) var layer0_tex: texture_2d<f32>;
@group(3) @binding(5) var layer0_sampler: sampler;
@group(3) @binding(6) var layer1_tex: texture_2d<f32>;
@group(3) @binding(7) var layer1_sampler: sampler;
@group(3) @binding(8) var layer2_tex: texture_2d<f32>;
@group(3) @binding(9) var layer2_sampler: sampler;
@group(3) @binding(10) var layer3_tex: texture_2d<f32>;
@group(3) @binding(11) var layer3_sampler: sampler;

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

    let texel = light_uni.shadow_params.z;
    let compare_depth = shadow_ndc.z - light_uni.shadow_params.y;
    var visibility = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            visibility += textureSampleCompare(shadow_tex, shadow_sampler, uv + offset, compare_depth);
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
fn fs_main_terrain(in: VertexOutput) -> @location(0) vec4<f32> {
    let weights_raw = textureSample(control_tex, control_sampler, in.uv);
    let weight_sum = max(weights_raw.r + weights_raw.g + weights_raw.b + weights_raw.a, 0.00001);
    let weights = weights_raw / weight_sum;

    let uv0 = in.uv * vec2<f32>(model.material_params.x, model.material_params.x);
    let uv1 = in.uv * vec2<f32>(model.material_params.y, model.material_params.y);
    let uv2 = in.uv * vec2<f32>(model.material_params.z, model.material_params.z);
    let uv3 = in.uv * vec2<f32>(model.material_params.w, model.material_params.w);

    let c0 = textureSample(layer0_tex, layer0_sampler, uv0);
    let c1 = textureSample(layer1_tex, layer1_sampler, uv1);
    let c2 = textureSample(layer2_tex, layer2_sampler, uv2);
    let c3 = textureSample(layer3_tex, layer3_sampler, uv3);
    let albedo = (c0 * weights.r + c1 * weights.g + c2 * weights.b + c3 * weights.a) * model.base_color;
    return shade(albedo, in);
}
