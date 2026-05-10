struct InvVPUniform {
    inv_vp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> ubo: InvVPUniform;
@group(1) @binding(0) var skybox_tex: texture_cube<f32>;
@group(1) @binding(1) var skybox_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) direction: vec3<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) receiver_mask: vec4<f32>,
    @location(2) surface_props: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let pos = positions[vertex_index];

    var out: VertexOutput;
    out.position = vec4<f32>(pos, 1.0, 1.0);
    let world_dir = ubo.inv_vp * vec4<f32>(pos, 1.0, 1.0);
    out.direction = normalize(world_dir.xyz / world_dir.w);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    var out: FragmentOutput;
    out.color = textureSample(skybox_tex, skybox_sampler, in.direction);
    out.receiver_mask = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    out.surface_props = vec4<f32>(0.5, 1.0, 0.5, 1.0);
    return out;
}

@fragment
fn fs_main_color(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(skybox_tex, skybox_sampler, in.direction);
}
