@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(3) var camera_depth_tex: texture_depth_2d;
@group(0) @binding(5) var decal_atlas_tex0: texture_2d<f32>;
@group(0) @binding(6) var decal_atlas_tex1: texture_2d<f32>;
@group(0) @binding(7) var receiver_mask_tex: texture_2d<f32>;
@group(0) @binding(8) var surface_props_tex: texture_2d<f32>;
@group(0) @binding(9) var decal_normal_atlas_tex0: texture_2d<f32>;
@group(0) @binding(10) var decal_normal_atlas_tex1: texture_2d<f32>;
@group(0) @binding(11) var decal_roughness_atlas_tex0: texture_2d<f32>;
@group(0) @binding(12) var decal_roughness_atlas_tex1: texture_2d<f32>;
@group(0) @binding(13) var decal_mask_atlas_tex0: texture_2d<f32>;
@group(0) @binding(14) var decal_mask_atlas_tex1: texture_2d<f32>;

struct PostUniform {
    // texel_size.xy, bloom threshold, bloom strength
    params0: vec4<f32>,
    // sharpen strength, FXAA strength, reserved, reserved
    params1: vec4<f32>,
    // contact AO strength, radius in pixels, depth response scale, reserved
    params2: vec4<f32>,
    // primary decal light direction xyz, intensity
    params3: vec4<f32>,
    // contact AO detail strength, detail radius, normal bias, quality tier
    params4: vec4<f32>,
    // secondary and tertiary decal light direction xyz, intensity
    params5: vec4<f32>,
    params6: vec4<f32>,
    // decal light count, decal ambient floor, reserved, reserved
    params7: vec4<f32>,
    // decal light color rgb, type (0=directional, 1=point)
    params8: vec4<f32>,
    params9: vec4<f32>,
    params10: vec4<f32>,
};

@group(0) @binding(2) var<uniform> post: PostUniform;

struct PostDecal {
    center_width: vec4<f32>,
    right_opacity: vec4<f32>,
    forward_length: vec4<f32>,
    color_depth: vec4<f32>,
    uv_rect: vec4<f32>,
    atlas_params: vec4<f32>,
    material_params: vec4<f32>,
    angle_params: vec4<f32>,
};

struct PostDecalUniform {
    inv_view_proj: mat4x4<f32>,
    // decal count, atlas count, reserved, reserved
    params: vec4<u32>,
    camera_pos: vec4<f32>,
    decals: array<PostDecal, 32>,
};

@group(0) @binding(4) var<uniform> decal_uniform: PostDecalUniform;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(3.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    var out: VertexOutput;
    let pos = positions[vertex_index];
    out.clip_position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = pos * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn depth_at_uv(uv: vec2<f32>) -> f32 {
    let size_u = textureDimensions(camera_depth_tex);
    let size = vec2<i32>(size_u);
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size_u)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(camera_depth_tex, pixel, 0);
}

fn receiver_mask_at_uv(uv: vec2<f32>) -> u32 {
    let size_u = textureDimensions(receiver_mask_tex);
    let size = vec2<i32>(size_u);
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size_u)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    let packed = textureLoad(receiver_mask_tex, pixel, 0).r;
    return u32(round(packed * 255.0));
}

fn surface_props_at_uv(uv: vec2<f32>) -> vec4<f32> {
    let size_u = textureDimensions(surface_props_tex);
    let size = vec2<i32>(size_u);
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size_u)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(surface_props_tex, pixel, 0);
}

fn surface_normal_at_uv(uv: vec2<f32>) -> vec3<f32> {
    return safe_normalize3(surface_props_at_uv(uv).rgb * 2.0 - vec3<f32>(1.0), vec3<f32>(0.0, 1.0, 0.0));
}

fn world_pos_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world_h = decal_uniform.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world_h.xyz / max(abs(world_h.w), 0.000001);
}

fn safe_normalize3(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let len_sq = dot(v, v);
    if len_sq <= 0.0000001 {
        return fallback;
    }
    return v * inverseSqrt(len_sq);
}

fn contact_ao_factor(uv: vec2<f32>, center_depth: f32) -> f32 {
    let strength = post.params2.x;
    if strength <= 0.001 || center_depth >= 0.9999 {
        return 1.0;
    }

    let texel = post.params0.xy;
    let radius = post.params2.y * mix(1.35, 0.55, smoothstep(0.18, 0.98, center_depth));
    let depth_scale = post.params2.z;
    let detail_strength = post.params4.x;
    let detail_radius = post.params4.y * mix(1.18, 0.66, smoothstep(0.18, 0.98, center_depth));
    let normal_bias = post.params4.z;
    let quality = u32(max(post.params4.w, 0.0) + 0.5);
    if quality == 0u {
        return 1.0;
    }
    let broad_samples = select(select(8u, 12u, quality >= 3u), 4u, quality == 1u);
    let detail_samples = select(select(4u, 8u, quality >= 3u), 0u, quality <= 1u);
    let offsets = array<vec2<f32>, 12>(
        vec2<f32>(1.0, 0.0),
        vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(2.0, 0.5),
        vec2<f32>(-2.0, -0.5),
        vec2<f32>(0.5, 2.0),
        vec2<f32>(-0.5, -2.0),
    );

    var occlusion = 0.0;
    var total_weight = 0.0;
    var detail_occlusion = 0.0;
    var detail_weight = 0.0;
    var first_ring_signal = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let offset = offsets[i];
        let sample_uv = clamp(uv + offset * texel * radius, vec2<f32>(0.0), vec2<f32>(1.0));
        let sample_depth = depth_at_uv(sample_uv);
        let nearer_delta = max(center_depth - sample_depth, 0.0);
        let local_reject = 1.0 - smoothstep(0.018, 0.12, nearer_delta);
        let edge_delta = nearer_delta * depth_scale;
        let plane_reject = smoothstep(0.045 + normal_bias, 0.23 + normal_bias, edge_delta);
        let sample_occlusion = smoothstep(0.02, 0.75, edge_delta) * local_reject * plane_reject;
        let weight = 1.0 / (1.0 + dot(offset, offset) * 0.18);
        occlusion += sample_occlusion * weight;
        total_weight += weight;
        first_ring_signal = max(first_ring_signal, sample_occlusion);

        if i < detail_samples && detail_strength > 0.001 {
            let detail_uv = clamp(uv + offset * texel * detail_radius, vec2<f32>(0.0), vec2<f32>(1.0));
            let detail_depth = depth_at_uv(detail_uv);
            let detail_nearer_delta = max(center_depth - detail_depth, 0.0);
            let detail_edge_delta = detail_nearer_delta * depth_scale * 1.35;
            let detail_plane_reject = smoothstep(0.025 + normal_bias * 0.5, 0.16 + normal_bias, detail_edge_delta);
            let detail_local_reject = 1.0 - smoothstep(0.012, 0.08, detail_nearer_delta);
            let detail_sample = smoothstep(0.015, 0.42, detail_edge_delta) * detail_plane_reject * detail_local_reject;
            let detail_w = 1.0 / (1.0 + dot(offset, offset) * 0.32);
            detail_occlusion += detail_sample * detail_w;
            detail_weight += detail_w;
            first_ring_signal = max(first_ring_signal, detail_sample * detail_strength);
        }
    }
    if broad_samples <= 4u || first_ring_signal <= 0.0005 {
        let ao = occlusion / max(total_weight, 0.0001);
        let detail_ao = detail_occlusion / max(detail_weight, 0.0001);
        return 1.0 - clamp(ao * strength + detail_ao * detail_strength, 0.0, 0.58);
    }
    for (var i = 4u; i < 12u; i = i + 1u) {
        if i >= broad_samples {
            break;
        }
        let offset = offsets[i];
        let sample_uv = clamp(uv + offset * texel * radius, vec2<f32>(0.0), vec2<f32>(1.0));
        let sample_depth = depth_at_uv(sample_uv);
        let nearer_delta = max(center_depth - sample_depth, 0.0);
        let local_reject = 1.0 - smoothstep(0.018, 0.12, nearer_delta);
        let edge_delta = nearer_delta * depth_scale;
        let plane_reject = smoothstep(0.045 + normal_bias, 0.23 + normal_bias, edge_delta);
        let sample_occlusion = smoothstep(0.02, 0.75, edge_delta) * local_reject * plane_reject;
        let weight = 1.0 / (1.0 + dot(offset, offset) * 0.18);
        occlusion += sample_occlusion * weight;
        total_weight += weight;

        if i < detail_samples && detail_strength > 0.001 {
            let detail_uv = clamp(uv + offset * texel * detail_radius, vec2<f32>(0.0), vec2<f32>(1.0));
            let detail_depth = depth_at_uv(detail_uv);
            let detail_nearer_delta = max(center_depth - detail_depth, 0.0);
            let detail_edge_delta = detail_nearer_delta * depth_scale * 1.35;
            let detail_plane_reject = smoothstep(0.025 + normal_bias * 0.5, 0.16 + normal_bias, detail_edge_delta);
            let detail_local_reject = 1.0 - smoothstep(0.012, 0.08, detail_nearer_delta);
            let detail_sample = smoothstep(0.015, 0.42, detail_edge_delta) * detail_plane_reject * detail_local_reject;
            let detail_w = 1.0 / (1.0 + dot(offset, offset) * 0.32);
            detail_occlusion += detail_sample * detail_w;
            detail_weight += detail_w;
        }
    }

    let ao = occlusion / max(total_weight, 0.0001);
    let detail_ao = detail_occlusion / max(detail_weight, 0.0001);
    return 1.0 - clamp(ao * strength + detail_ao * detail_strength, 0.0, 0.58);
}

fn sample_decal_atlas(slot: u32, atlas_uv: vec2<f32>) -> vec4<f32> {
    if slot == 1u {
        return textureSampleLevel(decal_atlas_tex1, source_sampler, atlas_uv, 0.0);
    }
    return textureSampleLevel(decal_atlas_tex0, source_sampler, atlas_uv, 0.0);
}

fn sample_decal_normal_atlas(slot: u32, atlas_uv: vec2<f32>) -> vec4<f32> {
    if slot == 1u {
        return textureSampleLevel(decal_normal_atlas_tex1, source_sampler, atlas_uv, 0.0);
    }
    return textureSampleLevel(decal_normal_atlas_tex0, source_sampler, atlas_uv, 0.0);
}

fn sample_decal_roughness_atlas(slot: u32, atlas_uv: vec2<f32>) -> vec4<f32> {
    if slot == 1u {
        return textureSampleLevel(decal_roughness_atlas_tex1, source_sampler, atlas_uv, 0.0);
    }
    return textureSampleLevel(decal_roughness_atlas_tex0, source_sampler, atlas_uv, 0.0);
}

fn sample_decal_mask_atlas(slot: u32, atlas_uv: vec2<f32>) -> vec4<f32> {
    if slot == 1u {
        return textureSampleLevel(decal_mask_atlas_tex1, source_sampler, atlas_uv, 0.0);
    }
    return textureSampleLevel(decal_mask_atlas_tex0, source_sampler, atlas_uv, 0.0);
}

fn luma3(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn decal_light_params(index: u32) -> vec4<f32> {
    if index == 1u {
        return post.params5;
    }
    if index == 2u {
        return post.params6;
    }
    return post.params3;
}

fn decal_light_color_params(index: u32) -> vec4<f32> {
    if index == 1u {
        return post.params9;
    }
    if index == 2u {
        return post.params10;
    }
    return post.params8;
}

fn decal_lighting_amount(normal: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let count = min(u32(max(post.params7.x, 0.0) + 0.5), 3u);
    var amount = vec3<f32>(max(post.params7.y, 0.04));
    for (var i = 0u; i < 3u; i = i + 1u) {
        if i >= count {
            break;
        }
        let light = decal_light_params(i);
        let light_color_type = decal_light_color_params(i);
        let intensity = max(light.w, 0.0);
        if intensity <= 0.0 {
            continue;
        }
        var dir = safe_normalize3(light.xyz, vec3<f32>(0.0, 1.0, 0.0));
        var attenuation = 1.0;
        if light_color_type.w >= 0.5 {
            let to_light = light.xyz - world_pos;
            let dist = length(to_light);
            dir = safe_normalize3(to_light, vec3<f32>(0.0, 1.0, 0.0));
            attenuation = 1.0 / (1.0 + 0.09 * dist + 0.032 * dist * dist);
        }
        amount = amount + max(dot(normal, dir), 0.04) * intensity * attenuation * max(light_color_type.rgb, vec3<f32>(0.0));
    }
    return max(amount, vec3<f32>(0.001));
}

fn projected_decal_color(base: vec3<f32>, uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let count = min(decal_uniform.params.x, 32u);
    if count == 0u || depth >= 0.9999 {
        return base;
    }
    let world_pos = world_pos_from_depth(uv, depth);
    let receiver_mask = receiver_mask_at_uv(uv);
    let surface_props = surface_props_at_uv(uv);
    let surface_normal = normalize(surface_props.rgb * 2.0 - vec3<f32>(1.0));
    let surface_roughness = clamp(surface_props.a, 0.04, 1.0);
    var color = base;
    for (var i = 0u; i < 32u; i = i + 1u) {
        if i >= count {
            break;
        }
        let decal = decal_uniform.decals[i];
        let rel = world_pos - decal.center_width.xyz;
        let half_width = max(decal.center_width.w, 0.001);
        let half_length = max(decal.forward_length.w, 0.001);
        let depth_extent = max(decal.color_depth.w, 0.001);
        let right_axis = normalize(decal.right_opacity.xyz);
        let forward_axis = normalize(decal.forward_length.xyz);
        let projection_normal = safe_normalize3(cross(forward_axis, right_axis), vec3<f32>(0.0, 1.0, 0.0));
        let local_x = dot(rel, right_axis) / half_width;
        let local_z = dot(rel, forward_axis) / half_length;
        let x = abs(local_x);
        let z = abs(local_z);
        let y = abs(rel.y) / depth_extent;
        let lateral_edge = max(x, z);
        let decal_receiver_mask = u32(max(decal.atlas_params.w, 0.0));
        let accepts_receiver = decal_receiver_mask == 0u || (receiver_mask & decal_receiver_mask) != 0u;
        let inside = select(0.0, 1.0, lateral_edge <= 1.0 && y <= 1.0 && accepts_receiver);
        let edge_fade = 1.0 - smoothstep(0.78, 1.0, lateral_edge);
        let depth_fade = 1.0 - smoothstep(0.72, 1.0, y);
        let fade_start = decal.atlas_params.y;
        let fade_end = decal.atlas_params.z;
        var distance_fade = 1.0;
        if fade_end > fade_start {
            let camera_dist = distance(decal.center_width.xyz, decal_uniform.camera_pos.xyz);
            distance_fade = 1.0 - smoothstep(fade_start, fade_end, camera_dist);
        }
        let angle_start = decal.angle_params.x;
        let angle_end = decal.angle_params.y;
        var angle_fade = 1.0;
        if angle_end > angle_start {
            let receiver_alignment = abs(dot(surface_normal, projection_normal));
            angle_fade = smoothstep(angle_start, angle_end, receiver_alignment);
        }
        let alpha = clamp(decal.right_opacity.w * edge_fade * depth_fade * distance_fade * angle_fade * inside, 0.0, 1.0);
        var decal_rgb = decal.color_depth.rgb;
        var decal_alpha = alpha;
        var atlas_sample = vec4<f32>(1.0);
        let atlas_slot = u32(max(decal.atlas_params.x, 0.0));
        let use_atlas = decal_uniform.params.y > atlas_slot && decal.atlas_params.x >= 0.0 && decal.uv_rect.z > 0.0 && decal.uv_rect.w > 0.0;
        var local_uv = vec2<f32>(local_x * 0.5 + 0.5, 1.0 - (local_z * 0.5 + 0.5));
        var atlas_uv = decal.uv_rect.xy + local_uv * decal.uv_rect.zw;
        let material_flags = u32(max(decal.material_params.w, 0.0) + 0.5);
        let has_normal_atlas = (material_flags & 1u) != 0u && use_atlas;
        let has_roughness_atlas = (material_flags & 2u) != 0u && use_atlas;
        let has_mask_atlas = (material_flags & 4u) != 0u && use_atlas;
        if use_atlas {
            atlas_sample = sample_decal_atlas(atlas_slot, atlas_uv);
            decal_rgb = atlas_sample.rgb * decal.color_depth.rgb;
            decal_alpha = decal_alpha * atlas_sample.a;
        }
        if has_mask_atlas {
            decal_alpha = decal_alpha * sample_decal_mask_atlas(atlas_slot, atlas_uv).r;
        }
        let normal_strength = decal.material_params.x;
        let roughness_strength = decal.material_params.z;
        if (normal_strength > 0.001 || roughness_strength > 0.001) && decal_alpha > 0.001 {
            var detail_x = sin((local_uv.x + luma3(atlas_sample.rgb) * 0.23) * 37.699);
            var detail_y = cos((local_uv.y + luma3(atlas_sample.rgb) * 0.31) * 31.416);
            if use_atlas {
                let texel = vec2<f32>(1.0 / 1024.0, 1.0 / 1024.0) * decal.uv_rect.zw;
                let sample_x = sample_decal_atlas(atlas_slot, clamp(atlas_uv + vec2<f32>(texel.x, 0.0), decal.uv_rect.xy, decal.uv_rect.xy + decal.uv_rect.zw)).rgb;
                let sample_y = sample_decal_atlas(atlas_slot, clamp(atlas_uv + vec2<f32>(0.0, texel.y), decal.uv_rect.xy, decal.uv_rect.xy + decal.uv_rect.zw)).rgb;
                let center_h = luma3(atlas_sample.rgb);
                detail_x = luma3(sample_x) - center_h;
                detail_y = luma3(sample_y) - center_h;
            }
            var decal_normal = surface_normal;
            if normal_strength > 0.001 {
                if has_normal_atlas {
                    let normal_sample = sample_decal_normal_atlas(atlas_slot, atlas_uv).xyz * 2.0 - vec3<f32>(1.0);
                    let tangent_normal = safe_normalize3(vec3<f32>(normal_sample.xy * normal_strength, max(normal_sample.z, 0.001)), vec3<f32>(0.0, 0.0, 1.0));
                    decal_normal = safe_normalize3(right_axis * tangent_normal.x - forward_axis * tangent_normal.y + surface_normal * tangent_normal.z, surface_normal);
                } else {
                    decal_normal = normalize(surface_normal + (right_axis * detail_x + forward_axis * detail_y) * normal_strength * 5.0);
                }
            }
            let base_light = decal_lighting_amount(surface_normal, world_pos);
            let decal_light = decal_lighting_amount(decal_normal, world_pos);
            let normal_response = clamp(decal_light / base_light, vec3<f32>(0.68), vec3<f32>(1.32));
            var decal_roughness = decal.material_params.y;
            if has_roughness_atlas {
                decal_roughness = clamp(sample_decal_roughness_atlas(atlas_slot, atlas_uv).r, 0.04, 1.0);
            }
            let blended_roughness = mix(surface_roughness, decal_roughness, roughness_strength * decal_alpha);
            let roughness_response = clamp(1.0 + (surface_roughness - blended_roughness) * 0.18, 0.86, 1.14);
            decal_rgb = decal_rgb * mix(vec3<f32>(1.0), normal_response * vec3<f32>(roughness_response), clamp(decal_alpha * 0.75, 0.0, 1.0));
        }
        color = mix(color, decal_rgb, decal_alpha);
    }
    return color;
}

fn scene_color_with_projected_decals(uv: vec2<f32>) -> vec4<f32> {
    let clamped_uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let source = textureSample(source_tex, source_sampler, clamped_uv);
    if decal_uniform.params.x == 0u {
        return source;
    }
    let depth = depth_at_uv(clamped_uv);
    return vec4<f32>(projected_decal_color(source.rgb, clamped_uv, depth), source.a);
}

fn source_color_at_uv(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(source_tex, source_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = post.params0.xy;
    let center = scene_color_with_projected_decals(in.uv);
    let center_rgb = center.rgb;
    let bloom_strength = post.params0.w;
    let sharpen_strength = post.params1.x;
    let fxaa_strength = post.params1.y;
    let ao_strength = post.params2.x;
    if bloom_strength <= 0.001 && sharpen_strength <= 0.001 && fxaa_strength <= 0.001 {
        var ao = 1.0;
        if ao_strength > 0.001 {
            ao = contact_ao_factor(in.uv, depth_at_uv(in.uv));
        }
        return vec4(min(center_rgb * ao, vec3<f32>(1.0)), center.a);
    }

    let n = source_color_at_uv(in.uv + vec2<f32>(0.0, -texel.y)).rgb;
    let s = source_color_at_uv(in.uv + vec2<f32>(0.0, texel.y)).rgb;
    let e = source_color_at_uv(in.uv + vec2<f32>(texel.x, 0.0)).rgb;
    let w = source_color_at_uv(in.uv + vec2<f32>(-texel.x, 0.0)).rgb;

    let c_l = luma(center_rgb);
    let n_l = luma(n);
    let s_l = luma(s);
    let e_l = luma(e);
    let w_l = luma(w);
    let l_min = min(c_l, min(min(n_l, s_l), min(e_l, w_l)));
    let l_max = max(c_l, max(max(n_l, s_l), max(e_l, w_l)));
    let edge_range = l_max - l_min;
    let horizontal_edge = abs(n_l + s_l - 2.0 * c_l) > abs(e_l + w_l - 2.0 * c_l);
    var edge_sample: vec3<f32>;
    if horizontal_edge {
        edge_sample = (n + s) * 0.5;
    } else {
        edge_sample = (e + w) * 0.5;
    }
    let fxaa_blend = smoothstep(0.025, 0.18, edge_range) * fxaa_strength * 0.34;
    let anti_aliased = mix(center_rgb, edge_sample, fxaa_blend);

    let cross_blur = (n + s + e + w) * 0.25;
    let sharpened = max(anti_aliased + (anti_aliased - cross_blur) * sharpen_strength, vec3<f32>(0.0));

    let bloom_gate = smoothstep(post.params0.z, min(post.params0.z + 0.22, 1.0), luma(cross_blur));
    let bloom = cross_blur * bloom_gate * bloom_strength;
    var ao = 1.0;
    if ao_strength > 0.001 {
        ao = contact_ao_factor(in.uv, depth_at_uv(in.uv));
    }
    let color = min(sharpened * ao + bloom, vec3<f32>(1.0));
    return vec4<f32>(color, center.a);
}
