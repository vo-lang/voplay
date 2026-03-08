// Sprite shader — renders textured quads via instanced rendering.
// Each instance carries destination rect, source UV rect, tint color, and transform params.
// Supports flip X/Y, rotation, and per-sprite tinting.

struct Camera {
    projection: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,  // unit quad: (0,0)→(1,1)
};

struct InstanceInput {
    @location(1) dst_rect: vec4<f32>,   // x, y, w, h in world/screen coords
    @location(2) src_rect: vec4<f32>,   // u0, v0, u1, v1 (normalized UV)
    @location(3) color: vec4<f32>,      // tint RGBA (multiply)
    @location(4) params: vec4<f32>,     // rotation, flipX (0/1), flipY (0/1), _unused
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    var out: VertexOutput;

    let pos = instance.dst_rect.xy;
    let size = instance.dst_rect.zw;
    let rotation = instance.params.x;

    // Local position within the rect
    let local = vertex.position * size;

    // Apply rotation around center of rect
    let center = size * 0.5;
    let offset = local - center;
    let cos_r = cos(rotation);
    let sin_r = sin(rotation);
    let rotated = vec2<f32>(
        offset.x * cos_r - offset.y * sin_r,
        offset.x * sin_r + offset.y * cos_r,
    );
    let world_pos = pos + center + rotated;

    out.clip_position = camera.projection * vec4<f32>(world_pos, 0.0, 1.0);

    // Compute UV with source rect and flip
    var uv = vertex.position;
    if instance.params.y > 0.5 {
        uv.x = 1.0 - uv.x; // flipX
    }
    if instance.params.z > 0.5 {
        uv.y = 1.0 - uv.y; // flipY
    }
    // Map from [0,1] to source UV rect
    out.uv = mix(instance.src_rect.xy, instance.src_rect.zw, uv);

    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(sprite_texture, sprite_sampler, in.uv);
    let final_color = tex_color * in.color;
    // Discard fully transparent pixels
    if final_color.a < 0.001 {
        discard;
    }
    return final_color;
}
