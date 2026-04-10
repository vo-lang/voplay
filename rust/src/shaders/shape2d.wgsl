// 2D shape shader — renders rects, circles, and lines via instanced quads.
// Each instance carries a rect (position + size), color, and shape parameters.
// The fragment shader uses SDF for circles and rounded rects.

struct Camera {
    projection: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec2<f32>,  // unit quad: (0,0), (1,0), (0,1), (1,1)
};

struct InstanceInput {
    @location(1) rect: vec4<f32>,      // x, y, w, h in world/screen coords
    @location(2) color: vec4<f32>,     // RGBA
    @location(3) params: vec4<f32>,    // shape_type, rotation (radians), corner_radius, _unused
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,        // 0..1 within the quad
    @location(1) color: vec4<f32>,
    @location(2) shape_type: f32,
    @location(3) corner_radius: f32,
    @location(4) size_px: vec2<f32>,   // size in pixels (for SDF)
};

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    var out: VertexOutput;

    let pos = instance.rect.xy;   // top-left
    let size = instance.rect.zw;  // width, height
    let rotation = instance.params.y;

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
    out.uv = vertex.position;
    out.color = instance.color;
    out.shape_type = instance.params.x;
    out.corner_radius = instance.params.z;
    out.size_px = size;

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let shape = i32(in.shape_type + 0.5);

    let half = in.size_px * 0.5;
    let r = min(in.corner_radius, min(half.x, half.y));
    let rect_p = abs(in.uv * in.size_px - half);
    let rect_q = rect_p - half + vec2<f32>(r, r);
    let rect_d = length(max(rect_q, vec2<f32>(0.0))) - r;
    let rect_aa = fwidth(rect_d);
    let rounded_rect_alpha = 1.0 - smoothstep(-rect_aa, rect_aa, rect_d);
    let rect_alpha = select(1.0, rounded_rect_alpha, in.corner_radius > 0.0);

    let circle_center = vec2<f32>(0.5, 0.5);
    let circle_dist = length(in.uv - circle_center);
    let circle_aa = fwidth(circle_dist);
    let circle_alpha = 1.0 - smoothstep(0.5 - circle_aa, 0.5, circle_dist);

    let half_len = in.size_px.x * 0.5;
    let half_thick = in.size_px.y * 0.5;
    let line_p = in.uv * in.size_px - vec2<f32>(half_len, half_thick);
    let line_qx = abs(line_p.x) - half_len + half_thick;
    let line_q = vec2<f32>(max(line_qx, 0.0), line_p.y);
    let line_d = length(line_q) - half_thick;
    let line_aa = fwidth(line_d);
    let line_alpha = 1.0 - smoothstep(-line_aa, line_aa, line_d);

    var alpha = rect_alpha;
    alpha = select(alpha, circle_alpha, shape == 1);
    alpha = select(alpha, line_alpha, shape == 2);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
