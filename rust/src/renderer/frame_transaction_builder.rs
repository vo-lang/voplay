use super::frame_decode::FrameDecodeOutput;
use super::frame_transaction::FrameTransaction;
use super::*;
use crate::renderer_frame::RenderFrameDecode;

pub(super) struct FrameTransactionBuilder<'a> {
    data: &'a [u8],
    screen_w: f32,
    screen_h: f32,
    aspect: f32,
    debug_frame_count: u64,
    perf_enabled: bool,
    texture_manager: &'a TextureManager,
    font_manager: &'a mut FontManager,
}

impl<'a> FrameTransactionBuilder<'a> {
    pub(super) fn new(
        data: &'a [u8],
        screen_w: f32,
        screen_h: f32,
        aspect: f32,
        debug_frame_count: u64,
        perf_enabled: bool,
        texture_manager: &'a TextureManager,
        font_manager: &'a mut FontManager,
    ) -> Self {
        Self {
            data,
            screen_w,
            screen_h,
            aspect,
            debug_frame_count,
            perf_enabled,
            texture_manager,
            font_manager,
        }
    }

    pub(super) fn decode(self) -> FrameDecodeOutput {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let aspect = self.aspect;
        let debug_frame_count = self.debug_frame_count;
        let mut transaction = FrameTransaction::new(screen_w, screen_h);
        let mut clear_color = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let mut camera3d_uniform: Option<Camera3DUniform> = None;
        let mut camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)> = None;
        let mut skybox_cubemap_id: Option<u32> = None;
        let mut shadow_enabled = false;
        let mut shadow_resolution = 2048u32;
        let mut shadow_strength = 1.0f32;
        let mut shadow_softness = 1.0f32;
        let mut shadow_distance = 0.0f32;
        let mut shadow_fade = 0.0f32;
        let mut shadow_quality = 3u32;
        let mut post_bloom_threshold = 0.74f32;
        let mut post_bloom_strength = 0.105f32;
        let mut post_sharpen_strength = 0.055f32;
        let mut post_fxaa_strength = 0.82f32;
        let mut post_contact_ao_strength = 0.0f32;
        let mut post_contact_ao_radius = 2.5f32;
        let mut post_contact_ao_depth_scale = 70.0f32;
        let mut post_contact_ao_detail_strength = 0.18f32;
        let mut post_contact_ao_detail_radius = 0.95f32;
        let mut post_contact_ao_normal_bias = 0.015f32;
        let mut post_contact_ao_quality = 2u32;
        let mut light_uniform = LightUniform {
            ambient: [0.1, 0.1, 0.1, 1.0],
            ambient_ground: [0.1, 0.1, 0.1, 1.0],
            count: [0, 0, 0, 0],
            lights: [LightData {
                position_or_dir: [0.0; 4],
                color_intensity: [0.0; 4],
            }; 8],
            fog_color: [0.0, 0.0, 0.0, 1.0],
            fog_params: [0.0, 0.0, 0.0, 0.0],
            shadow_vp: math3d::MAT4_IDENTITY,
            shadow_cascade_vp: [math3d::MAT4_IDENTITY; 4],
            shadow_cascade_splits: [0.0; 4],
            shadow_params: [0.0, 0.002, 1.0, 1.0],
            shadow_params2: [0.0, 0.0, 0.0, 0.0],
            color_params: [1.0, 1.0, 1.0, 0.0],
            debug_params: [0, debug_frame_count as u32, 0, 0],
        };
        let mut model_draws: Vec<ModelDraw> = Vec::new();
        let mut projected_decals: Vec<PostDecalGpu> = Vec::new();
        let mut projected_decal_atlas_bindings: Vec<ProjectedDecalAtlasBinding> = Vec::new();
        let mut current_projected_decal_atlas_id: Option<u32> = None;
        let mut current_projected_decal_normal_atlas_id: Option<u32> = None;
        let mut current_projected_decal_roughness_atlas_id: Option<u32> = None;
        let mut current_projected_decal_mask_atlas_id: Option<u32> = None;
        let mut current_projected_decal_fade = [0.0f32, 0.0f32];
        let mut current_projected_decal_angle_fade = [0.0f32, 0.0f32];
        let mut current_projected_decal_receivers = DECAL_RECEIVER_ALL;
        let mut current_projected_decal_surface = [0.0f32, 0.72f32, 0.0f32];
        let mut retained_scene_draws: Vec<u32> = Vec::new();
        let mut command_count = 0u32;
        let mut rect_count = 0u32;
        let mut circle_count = 0u32;
        let mut line_count = 0u32;
        let mut text_count = 0u32;
        let mut sprite_count = 0u32;
        let mut model_command_count = 0u32;
        let mut projected_decal_count = 0u32;
        let mut scene_upsert_count = 0u32;
        let mut scene_removal_count = 0u32;
        let mut scene_draw_count = 0u32;
        let mut skybox_count = 0u32;
        let decode_start = if self.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let mut reader = StreamReader::new(self.data);
        while let Some(cmd) = reader.next_command() {
            command_count += 1;
            match cmd {
                DrawCommand::Clear { r, g, b, a } => {
                    clear_color = wgpu::Color {
                        r: r as f64,
                        g: g as f64,
                        b: b as f64,
                        a: a as f64,
                    };
                }
                DrawCommand::SetCamera2D {
                    x,
                    y,
                    zoom,
                    rotation,
                } => transaction.set_camera_2d(x, y, zoom, rotation),
                DrawCommand::ResetCamera => transaction.reset_camera(),
                DrawCommand::SetLayer { z } => transaction.set_layer(z),
                DrawCommand::DrawRect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                    a,
                } => {
                    rect_count += 1;
                    transaction.push_rect(x, y, w, h, [r, g, b, a]);
                }
                DrawCommand::DrawCircle {
                    cx,
                    cy,
                    radius,
                    r,
                    g,
                    b,
                    a,
                } => {
                    circle_count += 1;
                    transaction.push_circle(cx, cy, radius, [r, g, b, a]);
                }
                DrawCommand::DrawLine {
                    x1,
                    y1,
                    x2,
                    y2,
                    thickness,
                    r,
                    g,
                    b,
                    a,
                } => {
                    line_count += 1;
                    transaction.push_line(x1, y1, x2, y2, thickness, [r, g, b, a]);
                }
                DrawCommand::SetFont { font_id } => {
                    self.font_manager.set_current(font_id);
                }
                DrawCommand::DrawText {
                    x,
                    y,
                    size,
                    r,
                    g,
                    b,
                    a,
                    text,
                } => {
                    text_count += 1;
                    let draws = self.font_manager.layout_text(&text, x, y, size, r, g, b, a);
                    for draw in draws {
                        transaction.push_sprite(draw.texture_id, draw.instance);
                    }
                }
                DrawCommand::DrawSprite {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    dst_x,
                    dst_y,
                    dst_w,
                    dst_h,
                    flip_x,
                    flip_y,
                    rotation,
                    r,
                    g,
                    b,
                    a,
                } => {
                    sprite_count += 1;
                    let (u0, v0, u1, v1) = if let Some(tex) = self.texture_manager.get(tex_id) {
                        if src_w == 0.0 && src_h == 0.0 {
                            (0.0, 0.0, 1.0, 1.0)
                        } else {
                            let tw = tex.width as f32;
                            let th = tex.height as f32;
                            (
                                src_x / tw,
                                src_y / th,
                                (src_x + src_w) / tw,
                                (src_y + src_h) / th,
                            )
                        }
                    } else {
                        (0.0, 0.0, 1.0, 1.0)
                    };
                    transaction.push_sprite(
                        tex_id,
                        SpriteInstance {
                            dst_rect: [dst_x, dst_y, dst_w, dst_h],
                            src_rect: [u0, v0, u1, v1],
                            color: [r, g, b, a],
                            params: [
                                rotation,
                                if flip_x { 1.0 } else { 0.0 },
                                if flip_y { 1.0 } else { 0.0 },
                                0.0,
                            ],
                        },
                    );
                }
                DrawCommand::SetCamera3D {
                    eye,
                    target,
                    up,
                    fov,
                    near,
                    far,
                } => {
                    camera3d_state = Some((eye, target, up, fov, near, far));
                    let v = math3d::look_at_rh(eye, target, up);
                    let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                    let view_proj = math3d::mat4_mul(&proj, &v);
                    camera3d_uniform = Some(Camera3DUniform {
                        view_proj,
                        camera_pos: eye.to_array(),
                        _pad: 0.0,
                    });
                }
                DrawCommand::SetLights3D {
                    ambient_r,
                    ambient_g,
                    ambient_b,
                    ambient_ground_r,
                    ambient_ground_g,
                    ambient_ground_b,
                    lights,
                } => {
                    light_uniform.ambient = [ambient_r, ambient_g, ambient_b, 1.0];
                    light_uniform.ambient_ground =
                        [ambient_ground_r, ambient_ground_g, ambient_ground_b, 1.0];
                    let count = lights.len().min(8);
                    light_uniform.count[0] = count as u32;
                    for (i, l) in lights.iter().take(8).enumerate() {
                        let (v, w_type) = if l.light_type == 0 {
                            (l.direction, 0.0f32)
                        } else {
                            (l.position, 1.0f32)
                        };
                        light_uniform.lights[i] = LightData {
                            position_or_dir: [v.x, v.y, v.z, w_type],
                            color_intensity: [l.color.x, l.color.y, l.color.z, l.intensity],
                        };
                    }
                }
                DrawCommand::SetFog3D {
                    mode,
                    color,
                    start,
                    end,
                    density,
                } => {
                    light_uniform.count[1] = mode as u32;
                    light_uniform.fog_color = [color.x, color.y, color.z, 1.0];
                    light_uniform.fog_params = [start, end, density, 0.0];
                }
                DrawCommand::SetColorGrading3D {
                    tone_map,
                    exposure,
                    contrast,
                    saturation,
                } => {
                    light_uniform.color_params = [
                        exposure.max(0.0),
                        contrast.max(0.0),
                        saturation.max(0.0),
                        tone_map as f32,
                    ];
                }
                DrawCommand::SetShadow3D {
                    enabled,
                    resolution,
                    strength,
                    softness,
                    distance,
                    fade,
                    quality,
                } => {
                    shadow_quality = quality.min(4);
                    shadow_enabled = enabled && shadow_quality > 0;
                    shadow_resolution = resolution.max(1);
                    shadow_strength = strength.clamp(0.0, 1.0);
                    shadow_softness = softness.clamp(0.5, 4.0);
                    shadow_distance = distance.max(0.0);
                    shadow_fade = fade.max(0.0);
                }
                DrawCommand::SetRenderDebug3D { mode } => {
                    light_uniform.debug_params[0] = mode.min(12) as u32;
                }
                DrawCommand::SetPostProcess3D {
                    bloom_threshold,
                    bloom_strength,
                    sharpen_strength,
                    fxaa_strength,
                } => {
                    post_bloom_threshold = bloom_threshold.clamp(0.0, 1.0);
                    post_bloom_strength = bloom_strength.clamp(0.0, 2.0);
                    post_sharpen_strength = sharpen_strength.clamp(0.0, 1.0);
                    post_fxaa_strength = fxaa_strength.clamp(0.0, 1.5);
                }
                DrawCommand::SetContactAO3D {
                    strength,
                    radius,
                    depth_scale,
                    detail_strength,
                    detail_radius,
                    normal_bias,
                    quality,
                } => {
                    post_contact_ao_strength = strength.clamp(0.0, 1.5);
                    post_contact_ao_radius = radius.clamp(0.5, 8.0);
                    post_contact_ao_depth_scale = depth_scale.clamp(1.0, 400.0);
                    post_contact_ao_detail_strength = detail_strength.clamp(0.0, 1.0);
                    post_contact_ao_detail_radius = detail_radius.clamp(0.35, 3.0);
                    post_contact_ao_normal_bias = normal_bias.clamp(0.0, 0.08);
                    post_contact_ao_quality = quality.min(4);
                }
                DrawCommand::DrawSkybox { cubemap_id } => {
                    skybox_count += 1;
                    skybox_cubemap_id = Some(cubemap_id);
                }
                DrawCommand::DrawProjectedDecal3D {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                } => {
                    projected_decal_count += 1;
                    projected_decals.push(
                        PostDecalGpu::new(position.to_array(), yaw, width, length, depth, color)
                            .with_distance_fade(
                                current_projected_decal_fade[0],
                                current_projected_decal_fade[1],
                            )
                            .with_angle_fade(
                                current_projected_decal_angle_fade[0],
                                current_projected_decal_angle_fade[1],
                            )
                            .with_receiver_mask(current_projected_decal_receivers)
                            .with_surface_response(
                                current_projected_decal_surface[0],
                                current_projected_decal_surface[1],
                                current_projected_decal_surface[2],
                            ),
                    );
                }
                DrawCommand::SetProjectedDecalAtlas3D { atlas_id } => {
                    current_projected_decal_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalNormalAtlas3D { atlas_id } => {
                    current_projected_decal_normal_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalRoughnessAtlas3D { atlas_id } => {
                    current_projected_decal_roughness_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalMaskAtlas3D { atlas_id } => {
                    current_projected_decal_mask_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalDistanceFade3D { start, end } => {
                    current_projected_decal_fade = if start >= 0.0 && end > start {
                        [start, end]
                    } else {
                        [0.0, 0.0]
                    };
                }
                DrawCommand::SetProjectedDecalAngleFade3D { start, end } => {
                    current_projected_decal_angle_fade = if start >= 0.0 && end > start {
                        [start.clamp(0.0, 1.0), end.clamp(0.0, 1.0)]
                    } else {
                        [0.0, 0.0]
                    };
                }
                DrawCommand::SetProjectedDecalReceiverMask3D { mask } => {
                    current_projected_decal_receivers = if mask == 0 {
                        DECAL_RECEIVER_ALL
                    } else {
                        mask.min(DECAL_RECEIVER_ALL)
                    };
                }
                DrawCommand::SetProjectedDecalSurfaceResponse3D {
                    normal_strength,
                    roughness,
                    roughness_strength,
                } => {
                    current_projected_decal_surface = [
                        normal_strength.clamp(0.0, 2.0),
                        if roughness > 0.0 {
                            roughness.clamp(0.04, 1.0)
                        } else {
                            0.72
                        },
                        roughness_strength.clamp(0.0, 1.0),
                    ];
                }
                DrawCommand::DrawProjectedDecal3DUV {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                    uv_rect,
                } => {
                    let albedo_id = current_projected_decal_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let normal_id = current_projected_decal_normal_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let roughness_id = current_projected_decal_roughness_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let mask_id = current_projected_decal_mask_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let binding = ProjectedDecalAtlasBinding {
                        albedo_id,
                        normal_id,
                        roughness_id,
                        mask_id,
                    };
                    let atlas_slot =
                        if albedo_id != 0 || normal_id != 0 || roughness_id != 0 || mask_id != 0 {
                            if let Some(slot) = projected_decal_atlas_bindings
                                .iter()
                                .position(|existing| *existing == binding)
                            {
                                Some(slot as u32)
                            } else if projected_decal_atlas_bindings.len() < MAX_POST_DECAL_ATLASES
                            {
                                projected_decal_atlas_bindings.push(binding);
                                Some((projected_decal_atlas_bindings.len() - 1) as u32)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                    let normal_atlas_enabled = atlas_slot.is_some() && normal_id != 0;
                    let roughness_atlas_enabled = atlas_slot.is_some() && roughness_id != 0;
                    let mask_atlas_enabled = atlas_slot.is_some() && mask_id != 0;
                    projected_decal_count += 1;
                    projected_decals.push(
                        PostDecalGpu::new_with_uv(
                            position.to_array(),
                            yaw,
                            width,
                            length,
                            depth,
                            color,
                            uv_rect,
                            atlas_slot,
                        )
                        .with_distance_fade(
                            current_projected_decal_fade[0],
                            current_projected_decal_fade[1],
                        )
                        .with_angle_fade(
                            current_projected_decal_angle_fade[0],
                            current_projected_decal_angle_fade[1],
                        )
                        .with_receiver_mask(current_projected_decal_receivers)
                        .with_surface_response(
                            current_projected_decal_surface[0],
                            current_projected_decal_surface[1],
                            current_projected_decal_surface[2],
                        )
                        .with_material_maps(
                            normal_atlas_enabled,
                            roughness_atlas_enabled,
                            mask_atlas_enabled,
                        ),
                    );
                }
                DrawCommand::DrawModel {
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    animation_world_id,
                    animation_target_id,
                } => {
                    model_command_count += 1;
                    let model_mat = math3d::model_matrix(pos, rot, scale);
                    let normal_mat = math3d::normal_matrix(&model_mat);
                    model_draws.push(ModelDraw {
                        model_id,
                        model_uniform: ModelUniform {
                            model: model_mat,
                            normal_matrix: normal_mat,
                            base_color: [1.0, 1.0, 1.0, 1.0],
                            material_params: [1.0, 1.0, 1.0, 1.0],
                            emissive_color: [0.0, 0.0, 0.0, 0.0],
                            texture_flags: [0.0, 0.0, 0.0, 0.0],
                            material_response: [1.0, 0.0, 1.0, 1.0],
                            texture_flags2: [0.0, 0.0, 0.0, 0.0],
                        },
                        material,
                        animation_world_id,
                        animation_target_id,
                    });
                }
                DrawCommand::Scene3DUpsertObject {
                    scene_id,
                    object_id,
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    visible,
                    animation_world_id,
                    animation_target_id,
                } => {
                    scene_upsert_count += 1;
                    transaction.upsert_object(RenderObjectUpdate {
                        scene_id,
                        object_id,
                        model_id,
                        pos,
                        rot,
                        scale,
                        material,
                        visible,
                        animation_world_id,
                        animation_target_id,
                    });
                }
                DrawCommand::Scene3DDestroyObject {
                    scene_id,
                    object_id,
                } => {
                    scene_removal_count += 1;
                    transaction.destroy_object(scene_id, object_id);
                }
                DrawCommand::Scene3DClear { scene_id } => {
                    scene_removal_count += 1;
                    transaction.clear_scene(scene_id);
                }
                DrawCommand::Scene3DDraw { scene_id } => {
                    scene_draw_count += 1;
                    retained_scene_draws.push(scene_id);
                }
                DrawCommand::Primitive3DUpsertInstance {
                    scene_id,
                    layer_id,
                    object_id,
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    visible,
                    flags,
                    lod_near,
                    lod_far,
                    wind_strength,
                    atlas_uv,
                } => {
                    scene_upsert_count += 1;
                    transaction.upsert_primitive_instance(PrimitiveObjectUpdate {
                        scene_id,
                        layer_id,
                        object_id,
                        model_id,
                        pos,
                        rot,
                        scale,
                        material,
                        visible,
                        flags,
                        lod_near,
                        lod_far,
                        wind_strength,
                        atlas_uv,
                    });
                }
                DrawCommand::Primitive3DDestroyInstance {
                    scene_id,
                    layer_id,
                    object_id,
                } => {
                    scene_removal_count += 1;
                    transaction.destroy_primitive_instance(scene_id, layer_id, object_id);
                }
                DrawCommand::Primitive3DClearLayer { scene_id, layer_id } => {
                    scene_removal_count += 1;
                    transaction.clear_primitive_layer(scene_id, layer_id);
                }
                DrawCommand::Primitive3DDestroyLayer { scene_id, layer_id } => {
                    scene_removal_count += 1;
                    transaction.destroy_primitive_layer(scene_id, layer_id);
                }
                DrawCommand::Primitive3DReplaceChunk {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    transaction.replace_primitive_chunk(scene_id, layer_id, chunk_id, instances);
                }
                DrawCommand::Primitive3DReplaceChunkRefs {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    transaction
                        .replace_primitive_chunk_refs(scene_id, layer_id, chunk_id, instances);
                }
                DrawCommand::Primitive3DReplaceChunkKeys {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    transaction
                        .replace_primitive_chunk_keys(scene_id, layer_id, chunk_id, instances);
                }
                DrawCommand::Primitive3DUpsertMaterials {
                    scene_id,
                    layer_id,
                    materials,
                } => transaction.upsert_primitive_materials(scene_id, layer_id, materials),
                DrawCommand::Primitive3DUpsertShapes {
                    scene_id,
                    layer_id,
                    shapes,
                } => transaction.upsert_primitive_shapes(scene_id, layer_id, shapes),
                DrawCommand::Primitive3DSetChunkVisible {
                    scene_id,
                    layer_id,
                    chunk_id,
                    visible,
                } => {
                    transaction.set_primitive_chunk_visible(scene_id, layer_id, chunk_id, visible);
                }
                DrawCommand::DrawBillboard {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    world_pos,
                    w: bw,
                    h: bh,
                    tint,
                } => {
                    if let Some(ref cam) = camera3d_uniform {
                        let clip = math3d::mat4_mul_vec4(
                            &cam.view_proj,
                            [world_pos.x, world_pos.y, world_pos.z, 1.0],
                        );
                        if clip[3] > 0.0 {
                            let ndc_x = clip[0] / clip[3];
                            let ndc_y = clip[1] / clip[3];
                            let screen_x = (ndc_x + 1.0) * 0.5 * screen_w - bw * 0.5;
                            let screen_y = (1.0 - ndc_y) * 0.5 * screen_h - bh * 0.5;

                            let (u0, v0, u1, v1) =
                                if let Some(tex) = self.texture_manager.get(tex_id) {
                                    if src_w == 0.0 && src_h == 0.0 {
                                        (0.0, 0.0, 1.0, 1.0)
                                    } else {
                                        let tw = tex.width as f32;
                                        let th = tex.height as f32;
                                        (
                                            src_x / tw,
                                            src_y / th,
                                            (src_x + src_w) / tw,
                                            (src_y + src_h) / th,
                                        )
                                    }
                                } else {
                                    (0.0, 0.0, 1.0, 1.0)
                                };
                            let color =
                                Renderer::fog_billboard_color(tint, world_pos, cam, &light_uniform);

                            transaction.push_sprite(
                                tex_id,
                                SpriteInstance {
                                    dst_rect: [screen_x, screen_y, bw, bh],
                                    src_rect: [u0, v0, u1, v1],
                                    color,
                                    params: [0.0, 0.0, 0.0, 0.0],
                                },
                            );
                        }
                    }
                }
            }
        }
        let elapsed_ms = elapsed_ms_opt(decode_start);

        FrameDecodeOutput {
            stage: RenderFrameDecode {
                frame_id: debug_frame_count.min(u32::MAX as u64) as u32,
                command_count,
                scene_mutation_count: scene_upsert_count.saturating_add(scene_removal_count),
                overlay_command_count: rect_count
                    .saturating_add(circle_count)
                    .saturating_add(line_count)
                    .saturating_add(text_count)
                    .saturating_add(sprite_count),
                elapsed_ms,
            },
            transaction,
            clear_color,
            camera3d_uniform,
            camera3d_state,
            skybox_cubemap_id,
            shadow_enabled,
            shadow_resolution,
            shadow_strength,
            shadow_softness,
            shadow_distance,
            shadow_fade,
            shadow_quality,
            post_bloom_threshold,
            post_bloom_strength,
            post_sharpen_strength,
            post_fxaa_strength,
            post_contact_ao_strength,
            post_contact_ao_radius,
            post_contact_ao_depth_scale,
            post_contact_ao_detail_strength,
            post_contact_ao_detail_radius,
            post_contact_ao_normal_bias,
            post_contact_ao_quality,
            light_uniform,
            model_draws,
            projected_decals,
            projected_decal_atlas_bindings,
            retained_scene_draws,
            rect_count,
            circle_count,
            line_count,
            text_count,
            sprite_count,
            model_command_count,
            projected_decal_count,
            scene_upsert_count,
            scene_removal_count,
            scene_draw_count,
            skybox_count,
        }
    }
}
