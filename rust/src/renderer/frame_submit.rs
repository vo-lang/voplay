use super::*;

impl Renderer {
    pub(super) fn submit_frame_inner(&mut self, data: &[u8]) -> Result<(), String> {
        let perf_enabled = self.perf_stats_enabled;
        let perf_overrides = RendererPerfOverrides::current();
        let frame_start = if perf_enabled { Some(perf_now()) } else { None };
        self.debug_frame_count = self.debug_frame_count.wrapping_add(1);
        let debug_frame_count = self.debug_frame_count;
        let mut perf = if perf_enabled {
            RendererPerfStats {
                frame_id: debug_frame_count.min(u32::MAX as u64) as u32,
                display_tick: debug_frame_count.min(u32::MAX as u64) as u32,
                ..RendererPerfStats::default()
            }
        } else {
            RendererPerfStats::default()
        };
        if perf_enabled {
            perf.diagnostic_flags = perf_overrides.flags();
        }
        #[cfg(feature = "wasm")]
        let debug_scope_frame = Self::debug_should_log_frame(debug_frame_count);

        #[cfg(feature = "wasm")]
        self.update_canvas_metrics();
        #[cfg(feature = "wasm")]
        if debug_scope_frame {
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        }

        let acquire_start = if perf_enabled { Some(perf_now()) } else { None };
        let output = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("voplay: get_current_texture: {}", e))?;
        perf.surface_acquire_ms = elapsed_ms_opt(acquire_start);
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });

        let screen_w = self.screen_width;
        let screen_h = self.screen_height;

        // Reset draw list for this frame
        let mut clear_color = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        self.draw_list.clear();
        self.draw_list.set_screen_space(screen_w, screen_h);

        // 3D state for this frame
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
        let mut primitive_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_depth_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_shadow_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_depth_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_shadow_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_main_draw_calls = 0u32;
        let mut primitive_depth_draw_calls = 0u32;
        let mut primitive_shadow_draw_calls = 0u32;
        let mut primitive_main_submitted = false;
        let mut primitive_main_stats = PrimitiveDrawStats::default();
        let mut primitive_water_stats = PrimitiveDrawStats::default();
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
        let mut resident_chunk_rebuild_count = 0u32;
        let aspect = screen_w / screen_h;

        // Decode command stream into the unified draw list
        let decode_start = if perf_enabled { Some(perf_now()) } else { None };
        let mut reader = StreamReader::new(data);
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
                } => {
                    self.draw_list
                        .set_camera_2d(screen_w, screen_h, x, y, zoom, rotation);
                }
                DrawCommand::ResetCamera => {
                    self.draw_list.reset_camera();
                }
                DrawCommand::SetLayer { z } => {
                    self.draw_list.set_layer(z);
                }
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
                    self.draw_list.push_rect(x, y, w, h, [r, g, b, a]);
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
                    self.draw_list.push_circle(cx, cy, radius, [r, g, b, a]);
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
                    self.draw_list
                        .push_line(x1, y1, x2, y2, thickness, [r, g, b, a]);
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
                        self.draw_list.push_sprite(draw.texture_id, draw.instance);
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
                            // src_w/src_h == 0 means "use full texture"
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
                    self.draw_list.push_sprite(
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
                // --- 3D commands ---
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
                    self.render_world.upsert_object(RenderObjectUpdate {
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
                    self.render_world.destroy_object(scene_id, object_id);
                }
                DrawCommand::Scene3DClear { scene_id } => {
                    scene_removal_count += 1;
                    self.render_world.clear_scene(scene_id);
                    self.primitive_pipeline.clear_scene(scene_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, _, _), _| *shape_scene != scene_id);
                    self.primitive_materials
                        .retain(|(material_scene, _, _), _| *material_scene != scene_id);
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
                    resident_chunk_rebuild_count += 1;
                    let update = PrimitiveObjectUpdate {
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
                    };
                    self.primitive_pipeline.upsert_instance(
                        &self.device,
                        &self.queue,
                        update,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world.upsert_primitive_instance(update);
                }
                DrawCommand::Primitive3DDestroyInstance {
                    scene_id,
                    layer_id,
                    object_id,
                } => {
                    scene_removal_count += 1;
                    resident_chunk_rebuild_count += 1;
                    self.primitive_pipeline.destroy_instance(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        object_id,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .destroy_primitive_instance(scene_id, layer_id, object_id);
                }
                DrawCommand::Primitive3DClearLayer { scene_id, layer_id } => {
                    scene_removal_count += 1;
                    self.primitive_pipeline.clear_layer(scene_id, layer_id);
                    self.render_world.clear_primitive_layer(scene_id, layer_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, shape_layer, _), _| {
                            *shape_scene != scene_id || *shape_layer != layer_id
                        });
                    self.primitive_materials
                        .retain(|(material_scene, material_layer, _), _| {
                            *material_scene != scene_id || *material_layer != layer_id
                        });
                }
                DrawCommand::Primitive3DDestroyLayer { scene_id, layer_id } => {
                    scene_removal_count += 1;
                    self.primitive_pipeline.clear_layer(scene_id, layer_id);
                    self.render_world
                        .destroy_primitive_layer(scene_id, layer_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, shape_layer, _), _| {
                            *shape_scene != scene_id || *shape_layer != layer_id
                        });
                    self.primitive_materials
                        .retain(|(material_scene, material_layer, _), _| {
                            *material_scene != scene_id || *material_layer != layer_id
                        });
                }
                DrawCommand::Primitive3DReplaceChunk {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    resident_chunk_rebuild_count += 1;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| PrimitiveObjectUpdate {
                            scene_id,
                            layer_id,
                            object_id: instance.object_id,
                            model_id: instance.model_id,
                            pos: instance.pos,
                            rot: instance.rot,
                            scale: instance.scale,
                            material: instance.material,
                            visible: instance.visible,
                            flags: instance.flags,
                            lod_near: instance.lod_near,
                            lod_far: instance.lod_far,
                            wind_strength: instance.wind_strength,
                            atlas_uv: instance.atlas_uv,
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DReplaceChunkRefs {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    resident_chunk_rebuild_count += 1;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| {
                            let material = self
                                .primitive_materials
                                .get(&(scene_id, layer_id, instance.material_id))
                                .copied()
                                .unwrap_or_default();
                            PrimitiveObjectUpdate {
                                scene_id,
                                layer_id,
                                object_id: instance.object_id,
                                model_id: instance.model_id,
                                pos: instance.pos,
                                rot: instance.rot,
                                scale: instance.scale,
                                material,
                                visible: instance.visible,
                                flags: instance.flags,
                                lod_near: instance.lod_near,
                                lod_far: instance.lod_far,
                                wind_strength: instance.wind_strength,
                                atlas_uv: instance.atlas_uv,
                            }
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DReplaceChunkKeys {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    resident_chunk_rebuild_count += 1;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| {
                            let model_id = self
                                .primitive_shapes
                                .get(&(scene_id, layer_id, instance.shape_id))
                                .copied()
                                .unwrap_or_default();
                            let material = self
                                .primitive_materials
                                .get(&(scene_id, layer_id, instance.material_id))
                                .copied()
                                .unwrap_or_default();
                            let mut material = material;
                            if instance.tint != [0.0, 0.0, 0.0, 0.0] {
                                material.base_color[0] *= instance.tint[0];
                                material.base_color[1] *= instance.tint[1];
                                material.base_color[2] *= instance.tint[2];
                                material.base_color[3] *= instance.tint[3];
                            }
                            PrimitiveObjectUpdate {
                                scene_id,
                                layer_id,
                                object_id: instance.object_id,
                                model_id,
                                pos: instance.pos,
                                rot: instance.rot,
                                scale: instance.scale,
                                material,
                                visible: instance.visible,
                                flags: instance.flags,
                                lod_near: instance.lod_near,
                                lod_far: instance.lod_far,
                                wind_strength: instance.wind_strength,
                                atlas_uv: instance.atlas_uv,
                            }
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DUpsertMaterials {
                    scene_id,
                    layer_id,
                    materials,
                } => {
                    for material in materials {
                        self.primitive_materials.insert(
                            (scene_id, layer_id, material.material_id),
                            material.material,
                        );
                    }
                }
                DrawCommand::Primitive3DUpsertShapes {
                    scene_id,
                    layer_id,
                    shapes,
                } => {
                    for shape in shapes {
                        self.primitive_shapes
                            .insert((scene_id, layer_id, shape.shape_id), shape.model_id);
                    }
                }
                DrawCommand::Primitive3DSetChunkVisible {
                    scene_id,
                    layer_id,
                    chunk_id,
                    visible,
                } => {
                    resident_chunk_rebuild_count += 1;
                    self.render_world
                        .set_primitive_chunk_visible(scene_id, layer_id, chunk_id, visible);
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
                    // Project 3D world position to screen coordinates using the current 3D camera
                    if let Some(ref cam) = camera3d_uniform {
                        let clip = math3d::mat4_mul_vec4(
                            &cam.view_proj,
                            [world_pos.x, world_pos.y, world_pos.z, 1.0],
                        );
                        if clip[3] > 0.0 {
                            let ndc_x = clip[0] / clip[3];
                            let ndc_y = clip[1] / clip[3];
                            // NDC -> logical screen coordinates.
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
                                Self::fog_billboard_color(tint, world_pos, cam, &light_uniform);

                            self.draw_list.push_sprite(
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
        perf.decode_ms = elapsed_ms_opt(decode_start);
        if perf_overrides.has(RENDERER_DIAG_DISABLE_SHADOWS) {
            shadow_enabled = false;
            shadow_strength = 0.0;
            shadow_quality = 0;
        }
        if perf_overrides.has(RENDERER_DIAG_DISABLE_POST_EFFECTS) {
            post_bloom_strength = 0.0;
            post_sharpen_strength = 0.0;
            post_fxaa_strength = 0.0;
            post_contact_ao_strength = 0.0;
            post_contact_ao_quality = 0;
            projected_decals.clear();
            projected_decal_atlas_bindings.clear();
        } else {
            if perf_overrides.has(RENDERER_DIAG_DISABLE_BLOOM) {
                post_bloom_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_SHARPEN) {
                post_sharpen_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_FXAA) {
                post_fxaa_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_CONTACT_AO) {
                post_contact_ao_strength = 0.0;
                post_contact_ao_quality = 0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_DECALS) {
                projected_decals.clear();
                projected_decal_atlas_bindings.clear();
            }
        }
        let contact_ao_active = post_contact_ao_strength > 0.001 && post_contact_ao_quality > 0;
        let projected_decals_active = !projected_decals.is_empty();
        let post_depth_active = contact_ao_active || projected_decals_active;
        let depth_prepass_active = MAIN_SAMPLE_COUNT > 1 && post_depth_active;
        let primitives_enabled = !perf_overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVES);
        let primitive_shadows_enabled = primitives_enabled
            && shadow_enabled
            && !perf_overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS);

        let scene_update_start = if perf_enabled { Some(perf_now()) } else { None };
        for scene_id in &retained_scene_draws {
            self.render_world
                .collect_scene_draws(*scene_id, &mut model_draws);
            if !primitives_enabled {
                continue;
            }
            self.render_world.collect_scene_primitive_draws(
                *scene_id,
                camera3d_uniform.as_ref(),
                &mut primitive_draws,
                &mut primitive_chunks,
            );
            if depth_prepass_active {
                self.render_world.collect_scene_primitive_depth_draws(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_depth_draws,
                    &mut primitive_depth_chunks,
                );
            }
            if primitive_shadows_enabled {
                self.render_world.collect_scene_primitive_shadow_objects(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_shadow_draws,
                );
                self.render_world
                    .collect_scene_primitive_shadow_chunks_from_candidates(
                        *scene_id,
                        camera3d_uniform.as_ref(),
                        &primitive_chunks,
                        &mut primitive_shadow_chunks,
                    );
            }
        }
        perf.scene_update_ms = elapsed_ms_opt(scene_update_start);
        let water_pass_active = primitives_enabled
            && camera3d_uniform.is_some()
            && self
                .primitive_pipeline
                .has_water_surface(&primitive_draws, &primitive_chunks);
        let render_batch_plan = RenderBatchPlanner::build(
            debug_frame_count.min(u32::MAX as u64) as u32,
            0,
            &model_draws,
            &primitive_draws,
            &primitive_chunks,
        );

        // Flush font atlas (re-upload if new glyphs were rasterized)
        self.font_manager
            .ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        // Resolve draw list: sort by (layer, order), produce draw calls
        let frame = self.draw_list.resolve();
        Self::debug_submit_status(
            debug_frame_count,
            &format!(
                "voplay submit #{} bytes={} cmds={} cam3d={} modelCmds={} sceneUpserts={} sceneDraws={} models={} primitives={} primitiveChunks={} skybox={} projectedDecals={} diagFlags=0x{:x} 2d(rect/circ/line/text/sprite)={}/{}/{}/{}/{} resolved(shapes/sprites/calls/cams)={}/{}/{}/{} clear={:.2},{:.2},{:.2}",
                debug_frame_count,
                data.len(),
                command_count,
                camera3d_uniform.is_some(),
                model_command_count,
                scene_upsert_count,
                scene_draw_count,
                model_draws.len(),
                primitive_draws.len(),
                primitive_chunks.len(),
                skybox_count,
                projected_decal_count,
                perf_overrides.flags(),
                rect_count,
                circle_count,
                line_count,
                text_count,
                sprite_count,
                frame.shapes.len(),
                frame.sprites.len(),
                frame.draw_calls.len(),
                frame.cameras.len(),
                clear_color.r,
                clear_color.g,
                clear_color.b,
            ),
        );

        // Upload all camera uniforms into the dynamic offset buffer
        let align = self.camera_alignment;
        let cam_count = frame.cameras.len();
        if cam_count > self.camera_slot_capacity {
            let new_cap = cam_count.next_power_of_two();
            let (buf, bg) =
                Self::create_camera_buffer_and_bg(&self.device, &self.camera_bgl, new_cap, align);
            self.camera_buffer = buf;
            self.camera_bind_group = bg;
            self.camera_slot_capacity = new_cap;
        }
        for (i, cam) in frame.cameras.iter().enumerate() {
            let offset = i as u64 * align as u64;
            self.queue
                .write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(cam));
        }

        // Upload sorted 2D instance data
        self.pipeline2d
            .upload_instances(&self.device, &self.queue, &frame.shapes);
        self.pipeline_sprite
            .upload_instances(&self.device, &self.queue, &frame.sprites);

        let mut frame_graph = FrameGraph::single_view(
            debug_frame_count.min(u32::MAX as u64) as u32,
            perf_overrides.flags(),
        );
        frame_graph.declare_external_target(RES_SURFACE_COLOR, true);
        frame_graph.declare_target(
            RES_MAIN_COLOR,
            self.targets.post_color_view.is_some() || self.targets.msaa_color_view.is_some(),
        );
        frame_graph.declare_target(RES_DEPTH, self.targets.depth_view.is_some());
        frame_graph.declare_target(RES_SHADOW_MAP, true);
        frame_graph.declare_target(RES_POST_COLOR, self.targets.post_color_view.is_some());
        frame_graph.declare_external_target(RES_OVERLAY, true);
        if post_depth_active {
            frame_graph.declare_target(RES_RECEIVER_MASK, self.targets.receiver_mask_view.is_some());
            frame_graph.declare_target(RES_SURFACE_PROPS, self.targets.surface_props_view.is_some());
        }
        frame_graph.plan_pass(
            RenderPassKind::DepthPrepass,
            &[],
            &[RES_DEPTH],
            depth_prepass_active,
        );
        frame_graph.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            shadow_enabled
                && camera3d_uniform.is_some()
                && (!model_draws.is_empty()
                    || !primitive_shadow_draws.is_empty()
                    || !primitive_shadow_chunks.is_empty()),
        );
        frame_graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_SHADOW_MAP],
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::MainTransparent,
            &[RES_MAIN_COLOR, RES_DEPTH],
            &[RES_MAIN_COLOR],
            false,
        );
        frame_graph.plan_transient_pass(
            RenderPassKind::Water,
            &[RES_DEPTH, RES_MAIN_COLOR],
            &[RES_WATER_COLOR, RES_MAIN_COLOR],
            water_pass_active,
        );
        frame_graph.plan_pass(
            RenderPassKind::Post,
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            &[RES_POST_COLOR, RES_SURFACE_COLOR],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::Overlay,
            &[RES_SURFACE_COLOR],
            &[RES_OVERLAY],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::BackendSubmit,
            &[RES_OVERLAY],
            &[RES_SURFACE_COLOR],
            true,
        );
        macro_rules! execute_render_node {
            ($kind:expr, $enabled:expr, $execute:expr, $workload:expr) => {{
                let node = frame_graph
                    .node($kind)
                    .ok_or_else(|| format!("voplay: missing frame graph node {}", $kind.name()))?;
                frame_graph
                    .executor()
                    .execute_node(&node, $enabled, $execute, $workload)
            }};
        }

        perf.depth_pass_ms = execute_render_node!(
            RenderPassKind::DepthPrepass,
            true,
            || {
                let depth_start = if perf_enabled { Some(perf_now()) } else { None };
                let empty_model_draws: &[ModelDraw] = &[];
                let empty_primitive_draws: &[PrimitiveDraw] = &[];
                let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                if !primitive_depth_chunks.is_empty() {
                    self.primitive_pipeline.append_resident_depth_draws(
                        &primitive_depth_chunks,
                        &mut primitive_depth_draws,
                    );
                }
                let (depth_model_draws, depth_primitive_draws, depth_view_proj) =
                    if let Some(ref cam3d) = camera3d_uniform {
                        (
                            &model_draws[..],
                            &primitive_depth_draws[..],
                            cam3d.view_proj,
                        )
                    } else {
                        (
                            empty_model_draws,
                            empty_primitive_draws,
                            math3d::MAT4_IDENTITY,
                        )
                    };
                self.pipeline_depth.render_depth_pass(
                    &self.device,
                    &mut encoder,
                    &self.queue,
                    &depth_view_proj,
                    depth_model_draws,
                    depth_primitive_draws,
                    empty_primitive_chunks,
                    &self.primitive_pipeline,
                    &self.model_manager,
                );
                primitive_depth_draw_calls = self.pipeline_depth.last_primitive_batch_count();
                Ok(elapsed_ms_opt(depth_start))
            },
            || RenderPassWorkload::default()
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);

        let mut shadow_active = false;
        perf.shadow_pass_ms = execute_render_node!(
            RenderPassKind::Shadow,
            true,
            || {
                let shadow_start = if perf_enabled { Some(perf_now()) } else { None };
            if let Some(ref cam3d) = camera3d_uniform {
                if light_uniform.count[0] > 0 && light_uniform.lights[0].position_or_dir[3] == 0.0 {
                    let shadow_to_light = Vec3::new(
                        light_uniform.lights[0].position_or_dir[0],
                        light_uniform.lights[0].position_or_dir[1],
                        light_uniform.lights[0].position_or_dir[2],
                    );
                    let shadow_dir = (-shadow_to_light).normalize();
                    if shadow_dir.length() > 0.0 {
                        let mut cascade_count = shadow_cascade_count_for_quality(shadow_quality);
                        if camera3d_state.is_none() {
                            cascade_count = 1;
                        }
                        let shadow_atlas_size =
                            shadow_atlas_resolution(shadow_resolution, cascade_count);
                        let tile_resolution = if cascade_count > 1 {
                            (shadow_atlas_size / 2).max(1)
                        } else {
                            shadow_atlas_size
                        };
                        if self.pipeline_shadow.size() != shadow_atlas_size {
                            self.clear_texture_bind_group_caches();
                            self.pipeline_shadow.resize(&self.device, shadow_atlas_size);
                        }
                        let mut shadow_cascade_vps = [math3d::MAT4_IDENTITY; 4];
                        let mut shadow_cascade_splits = [0.0; 4];
                        let shadow_vp = if let Some((eye, target, up, fov, near, camera_far)) =
                            camera3d_state
                        {
                            let shadow_far = if shadow_distance > 0.0 {
                                shadow_distance.min(camera_far).max(near + 0.1)
                            } else {
                                camera_far
                            };
                            if cascade_count > 1 {
                                shadow_cascade_splits =
                                    compute_shadow_cascade_splits(near, shadow_far, cascade_count);
                                let mut cascade_near = near;
                                for cascade_index in 0..cascade_count {
                                    let cascade_far = shadow_cascade_splits[cascade_index];
                                    shadow_cascade_vps[cascade_index] =
                                        math3d::compute_shadow_vp_for_camera_stabilized(
                                            eye,
                                            target,
                                            up,
                                            fov.to_radians(),
                                            aspect,
                                            cascade_near,
                                            cascade_far,
                                            shadow_dir,
                                            tile_resolution,
                                        );
                                    cascade_near = cascade_far;
                                }
                                shadow_cascade_vps[0]
                            } else {
                                let shadow_vp = math3d::compute_shadow_vp_for_camera_stabilized(
                                    eye,
                                    target,
                                    up,
                                    fov.to_radians(),
                                    aspect,
                                    near,
                                    shadow_far,
                                    shadow_dir,
                                    tile_resolution,
                                );
                                shadow_cascade_vps[0] = shadow_vp;
                                shadow_cascade_splits[0] = shadow_far;
                                shadow_vp
                            }
                        } else {
                            let inv_view_proj =
                                    math3d::mat4_inverse(&cam3d.view_proj).ok_or_else(|| {
                                        "voplay: failed to invert camera view projection for shadow mapping"
                                            .to_string()
                                    })?;
                            let shadow_vp = math3d::compute_shadow_vp_stabilized(
                                &inv_view_proj,
                                shadow_dir,
                                tile_resolution,
                            );
                            shadow_cascade_vps[0] = shadow_vp;
                            shadow_vp
                        };
                        if cascade_count > 1 {
                            let mut cascade_primitive_shadow_draws: Vec<Vec<PrimitiveDraw>> =
                                Vec::new();
                            let mut cascade_primitive_shadow_chunks: Vec<Vec<PrimitiveChunkRef>> =
                                Vec::new();
                            if !primitive_shadow_draws.is_empty()
                                || !primitive_shadow_chunks.is_empty()
                            {
                                cascade_primitive_shadow_draws.reserve(cascade_count);
                                cascade_primitive_shadow_chunks.reserve(cascade_count);
                                for cascade_index in 0..cascade_count {
                                    let light_camera = Camera3DUniform {
                                        view_proj: shadow_cascade_vps[cascade_index],
                                        camera_pos: cam3d.camera_pos,
                                        _pad: 0.0,
                                    };
                                    let mut cascade_shadow_draws = Vec::new();
                                    let mut cascade_shadow_chunks = Vec::new();
                                    for scene_id in &retained_scene_draws {
                                        self.render_world
                                            .collect_scene_primitive_shadow_objects_for_light_view(
                                                *scene_id,
                                                camera3d_uniform.as_ref(),
                                                &light_camera,
                                                &mut cascade_shadow_draws,
                                            );
                                        self.render_world
                                            .collect_scene_primitive_shadow_chunks_for_light_view(
                                                *scene_id,
                                                camera3d_uniform.as_ref(),
                                                &light_camera,
                                                &primitive_shadow_chunks,
                                                &mut cascade_shadow_chunks,
                                            );
                                    }
                                    if !cascade_shadow_chunks.is_empty() {
                                        self.primitive_pipeline.append_resident_shadow_draws(
                                            &cascade_shadow_chunks,
                                            &mut cascade_shadow_draws,
                                        );
                                    }
                                    cascade_primitive_shadow_draws.push(cascade_shadow_draws);
                                    cascade_primitive_shadow_chunks.push(Vec::new());
                                }
                            }
                            let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                            self.pipeline_shadow.render_shadow_cascade_pass(
                                &self.device,
                                &mut encoder,
                                &self.queue,
                                &shadow_cascade_vps[..cascade_count],
                                &model_draws,
                                &primitive_shadow_draws,
                                &cascade_primitive_shadow_draws,
                                empty_primitive_chunks,
                                &cascade_primitive_shadow_chunks,
                                &self.primitive_pipeline,
                                &self.model_manager,
                            );
                        } else {
                            let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                            if !primitive_shadow_chunks.is_empty() {
                                self.primitive_pipeline.append_resident_shadow_draws(
                                    &primitive_shadow_chunks,
                                    &mut primitive_shadow_draws,
                                );
                            }
                            self.pipeline_shadow.render_shadow_pass(
                                &self.device,
                                &mut encoder,
                                &self.queue,
                                &shadow_vp,
                                &model_draws,
                                &primitive_shadow_draws,
                                empty_primitive_chunks,
                                &self.primitive_pipeline,
                                &self.model_manager,
                            );
                        }
                        primitive_shadow_draw_calls =
                            self.pipeline_shadow.last_primitive_batch_count();
                        light_uniform.shadow_vp = shadow_vp;
                        light_uniform.shadow_cascade_vp = shadow_cascade_vps;
                        light_uniform.shadow_cascade_splits = shadow_cascade_splits;
                        light_uniform.shadow_params =
                            [1.0, 0.002, shadow_softness, shadow_strength];
                        light_uniform.shadow_params2 = [
                            shadow_distance,
                            shadow_fade,
                            shadow_quality as f32,
                            cascade_count as f32,
                        ];
                        light_uniform.count[2] = 0;
                        shadow_active = true;
                    }
                }
            }
                Ok(elapsed_ms_opt(shadow_start))
            },
            || RenderPassWorkload::default()
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);
        if !shadow_active {
            light_uniform.shadow_vp = math3d::MAT4_IDENTITY;
            light_uniform.shadow_cascade_vp = [math3d::MAT4_IDENTITY; 4];
            light_uniform.shadow_cascade_splits = [0.0; 4];
            light_uniform.shadow_params = [0.0, 0.002, shadow_softness, shadow_strength];
            light_uniform.shadow_params2 =
                [shadow_distance, shadow_fade, shadow_quality as f32, 0.0];
        }
        // Render pass
        let main_aux_targets_enabled = post_depth_active;

        let mut post_uniform = PostUniform::from_settings(
            self.surface_config.width,
            self.surface_config.height,
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
        );
        let mut post_decal_light_vectors = [[0.0f32; 4]; 3];
        let mut post_decal_light_colors = [[0.0f32; 4]; 3];
        let mut post_decal_light_count = 0usize;
        for light in light_uniform
            .lights
            .iter()
            .take(light_uniform.count[0].min(light_uniform.lights.len() as u32) as usize)
        {
            if light.color_intensity[3] > 0.0 {
                post_decal_light_vectors[post_decal_light_count] = [
                    light.position_or_dir[0],
                    light.position_or_dir[1],
                    light.position_or_dir[2],
                    light.color_intensity[3],
                ];
                post_decal_light_colors[post_decal_light_count] = [
                    light.color_intensity[0],
                    light.color_intensity[1],
                    light.color_intensity[2],
                    light.position_or_dir[3],
                ];
                post_decal_light_count += 1;
                if post_decal_light_count >= post_decal_light_vectors.len() {
                    break;
                }
            }
        }
        if post_decal_light_count > 0 {
            post_uniform = post_uniform.with_decal_lights(
                &post_decal_light_vectors[..post_decal_light_count],
                &post_decal_light_colors[..post_decal_light_count],
            );
        }
        self.queue.write_buffer(
            &self.post_uniform_buffer,
            0,
            bytemuck::bytes_of(&post_uniform),
        );
        let post_inv_view_proj = camera3d_uniform
            .as_ref()
            .and_then(|camera| math3d::mat4_inverse(&camera.view_proj))
            .unwrap_or(math3d::MAT4_IDENTITY);
        let post_camera_pos = camera3d_state
            .map(|(eye, _, _, _, _, _)| eye.to_array())
            .unwrap_or([0.0, 0.0, 0.0]);
        self.queue.write_buffer(
            &self.post_decal_uniform_buffer,
            0,
            bytemuck::bytes_of(&PostDecalUniform::from_decals(
                post_inv_view_proj,
                post_camera_pos,
                &projected_decals,
                projected_decal_atlas_bindings.len() as u32,
            )),
        );
        perf.main_pass_ms = execute_render_node!(
            RenderPassKind::MainOpaque,
            true,
            || {
                let main_start = if perf_enabled { Some(perf_now()) } else { None };
                let main_setup_start = if perf_enabled { Some(perf_now()) } else { None };
                let post_color_view = self
                    .targets.post_color_view
                    .as_ref()
                    .ok_or_else(|| "voplay: missing post color target".to_string())?;
                let main_color_view = if MAIN_SAMPLE_COUNT > 1 {
                    self.targets.msaa_color_view
                        .as_ref()
                        .ok_or_else(|| "voplay: missing MSAA color target".to_string())?
                } else {
                    post_color_view
                };
                let receiver_mask_view = if main_aux_targets_enabled {
                    Some(
                        self.targets.receiver_mask_view
                            .as_ref()
                            .ok_or_else(|| "voplay: missing receiver mask target".to_string())?,
                    )
                } else {
                    None
                };
                let surface_props_view = if main_aux_targets_enabled {
                    Some(
                        self.targets.surface_props_view
                            .as_ref()
                            .ok_or_else(|| "voplay: missing surface props target".to_string())?,
                    )
                } else {
                    None
                };
                let main_receiver_mask_view = if main_aux_targets_enabled {
                    Some(if MAIN_SAMPLE_COUNT > 1 {
                        self.targets.msaa_receiver_mask_view.as_ref().ok_or_else(|| {
                            "voplay: missing MSAA receiver mask target".to_string()
                        })?
                    } else {
                        receiver_mask_view.expect("receiver mask view present")
                    })
                } else {
                    None
                };
                let main_surface_props_view = if main_aux_targets_enabled {
                    Some(if MAIN_SAMPLE_COUNT > 1 {
                        self.targets.msaa_surface_props_view.as_ref().ok_or_else(|| {
                            "voplay: missing MSAA surface props target".to_string()
                        })?
                    } else {
                        surface_props_view.expect("surface props view present")
                    })
                } else {
                    None
                };
                let resolve_target = if MAIN_SAMPLE_COUNT > 1 {
                    Some(post_color_view)
                } else {
                    None
                };
                let receiver_mask_resolve_target =
                    if main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1 {
                        receiver_mask_view
                    } else {
                        None
                    };
                let surface_props_resolve_target =
                    if main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1 {
                        surface_props_view
                    } else {
                        None
                    };
                let color_store = if MAIN_SAMPLE_COUNT > 1 {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                };
                let receiver_mask_store = if MAIN_SAMPLE_COUNT > 1 {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                };
                let surface_props_store = if MAIN_SAMPLE_COUNT > 1 {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                };
                let color_attachments = [
                    Some(wgpu::RenderPassColorAttachment {
                        view: main_color_view,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear_color),
                            store: color_store,
                        },
                    }),
                    main_receiver_mask_view.map(|view| wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: receiver_mask_resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: receiver_mask_store,
                        },
                    }),
                    main_surface_props_view.map(|view| wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: surface_props_resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: surface_props_store,
                        },
                    }),
                ];
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voplay_main"),
                    color_attachments: &color_attachments,
                    depth_stencil_attachment: self.targets.depth_view.as_ref().map(|dv| {
                        wgpu::RenderPassDepthStencilAttachment {
                            view: dv,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                perf.main_pass_setup_ms = elapsed_ms_opt(main_setup_start);

                if let (Some(cubemap_id), Some((eye, target, up, fov, near, far))) =
                    (skybox_cubemap_id, camera3d_state)
                {
                    if let Some(cubemap) = self.texture_manager.get_cubemap(cubemap_id) {
                        let skybox_start = if perf_enabled { Some(perf_now()) } else { None };
                        let view_rot = math3d::view_rotation_only(eye, target, up);
                        let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                        let vp = math3d::mat4_mul(&proj, &view_rot);
                        let inv_vp = math3d::mat4_inverse(&vp).unwrap_or(math3d::MAT4_IDENTITY);
                        self.pipeline_skybox.set_camera(&self.queue, &inv_vp);
                        self.pipeline_skybox.draw(
                            &mut render_pass,
                            cubemap,
                            main_aux_targets_enabled,
                        );
                        perf.main_skybox_ms += elapsed_ms_opt(skybox_start);
                    }
                }

                // Draw 3D models first (depth tested)
                if !model_draws.is_empty() {
                    if let Some(ref cam3d) = camera3d_uniform {
                        let model_start = if perf_enabled { Some(perf_now()) } else { None };
                        self.pipeline3d
                            .set_camera_and_lights(&self.queue, cam3d, &light_uniform);
                        let shadow_view = self.pipeline_shadow.shadow_texture_view();
                        self.pipeline3d.draw_models(
                            &self.device,
                            &self.queue,
                            &mut render_pass,
                            &model_draws,
                            &self.model_manager,
                            &self.texture_manager,
                            shadow_view,
                            main_aux_targets_enabled,
                        );
                        perf.main_model_ms += elapsed_ms_opt(model_start);
                    }
                }

                if !primitive_draws.is_empty() || !primitive_chunks.is_empty() {
                    if let Some(ref cam3d) = camera3d_uniform {
                        let primitive_start = if perf_enabled { Some(perf_now()) } else { None };
                        self.primitive_pipeline.set_camera_and_lights(
                            &self.queue,
                            cam3d,
                            &light_uniform,
                        );
                        let shadow_view = self.pipeline_shadow.shadow_texture_view();
                        let layered_stats = self.primitive_pipeline.draw_main_and_water(
                            &self.device,
                            &self.queue,
                            &mut render_pass,
                            &primitive_draws,
                            &primitive_chunks,
                            &self.model_manager,
                            &self.texture_manager,
                            shadow_view,
                            main_aux_targets_enabled,
                        );
                        primitive_main_stats = layered_stats.main;
                        primitive_water_stats = layered_stats.water;
                        primitive_main_submitted = primitive_main_stats.batch_count > 0;
                        perf.main_primitive_ms += if perf_enabled {
                            layered_stats.main_cpu_ms
                        } else {
                            elapsed_ms_opt(primitive_start)
                        };
                        perf.water_pass_ms += layered_stats.water_cpu_ms;
                    }
                }
                let main_close_start = if perf_enabled { Some(perf_now()) } else { None };
                drop(render_pass);
                perf.main_pass_close_ms = elapsed_ms_opt(main_close_start);
                Ok(elapsed_ms_opt(main_start))
            },
            || RenderPassWorkload {
                draw_calls: saturating_u32(model_draws.len() + primitive_draws.len()),
                batches: saturating_u32(model_draws.len() + primitive_chunks.len()),
                instances: saturating_u32(model_draws.len() + primitive_draws.len()),
                triangles: 0,
                upload_bytes: 0,
            }
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);
        if primitive_main_submitted {
            primitive_main_draw_calls = primitive_main_stats.batch_count;
        }
        let _transparent_pass_ms = execute_render_node!(
            RenderPassKind::MainTransparent,
            true,
            || Ok::<f64, String>(0.0),
            || RenderPassWorkload::default()
        )?;
        perf.water_pass_ms = execute_render_node!(
            RenderPassKind::Water,
            true,
            || Ok::<f64, String>(perf.water_pass_ms),
            || RenderPassWorkload {
                draw_calls: primitive_water_stats.batch_count,
                batches: primitive_water_stats.batch_count,
                instances: primitive_water_stats.instance_count,
                triangles: primitive_water_stats.triangle_count,
                upload_bytes: 0,
            }
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);

        perf.post_pass_ms = execute_render_node!(
            RenderPassKind::Post,
            true,
            || {
                let post_start = if perf_enabled { Some(perf_now()) } else { None };
                let post_color_view = self
                    .targets.post_color_view
                    .as_ref()
                    .ok_or_else(|| "voplay: missing post color target".to_string())?;
                let receiver_mask_view = self
                    .targets.receiver_mask_view
                    .as_ref()
                    .ok_or_else(|| "voplay: missing receiver mask target".to_string())?;
                let surface_props_view = self
                    .targets.surface_props_view
                    .as_ref()
                    .ok_or_else(|| "voplay: missing surface props target".to_string())?;
                let dynamic_post_bind_group;
                let post_bind_group = if projected_decal_atlas_bindings.is_empty() {
                    self.post_bind_group
                        .as_ref()
                        .ok_or_else(|| "voplay: missing post bind group".to_string())?
                } else {
                    let fallback_decal_atlas = self.pipeline_post.decal_fallback_view();
                    let fallback_decal_normal_atlas =
                        self.pipeline_post.decal_normal_fallback_view();
                    let fallback_decal_roughness_atlas =
                        self.pipeline_post.decal_roughness_fallback_view();
                    let fallback_decal_mask_atlas = self.pipeline_post.decal_mask_fallback_view();
                    let mut decal_atlas_views = [fallback_decal_atlas; MAX_POST_DECAL_ATLASES];
                    let mut decal_normal_atlas_views =
                        [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES];
                    let mut decal_roughness_atlas_views =
                        [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES];
                    let mut decal_mask_atlas_views =
                        [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES];
                    for (slot, binding) in projected_decal_atlas_bindings.iter().enumerate() {
                        if let Some(texture) = self.texture_manager.get(binding.albedo_id) {
                            decal_atlas_views[slot] = &texture.view;
                        }
                        if let Some(texture) = self.texture_manager.get(binding.normal_id) {
                            decal_normal_atlas_views[slot] = &texture.view;
                        }
                        if let Some(texture) = self.texture_manager.get(binding.roughness_id) {
                            decal_roughness_atlas_views[slot] = &texture.view;
                        }
                        if let Some(texture) = self.texture_manager.get(binding.mask_id) {
                            decal_mask_atlas_views[slot] = &texture.view;
                        }
                    }
                    let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
                        self.pipeline_depth.depth_texture_view()
                    } else {
                        self.targets.depth_view
                            .as_ref()
                            .ok_or_else(|| "voplay: missing depth target".to_string())?
                    };
                    dynamic_post_bind_group = self.pipeline_post.create_bind_group(
                        &self.device,
                        post_color_view,
                        post_depth_view,
                        &self.post_uniform_buffer,
                        &self.post_decal_uniform_buffer,
                        decal_atlas_views,
                        decal_normal_atlas_views,
                        decal_roughness_atlas_views,
                        decal_mask_atlas_views,
                        receiver_mask_view,
                        surface_props_view,
                    );
                    &dynamic_post_bind_group
                };
                let mut post_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voplay_post"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.pipeline_post.draw(&mut post_pass, &post_bind_group);
                Ok(elapsed_ms_opt(post_start))
            },
            || RenderPassWorkload {
                draw_calls: 1,
                batches: 1,
                instances: 1,
                triangles: 2,
                upload_bytes: 0,
            }
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);

        perf.overlay_pass_ms = execute_render_node!(
            RenderPassKind::Overlay,
            true,
            || {
                let overlay_start = if perf_enabled { Some(perf_now()) } else { None };
                let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("voplay_overlay_2d"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                for dc in &frame.draw_calls {
                    let cam_offset = dc.camera_idx as u32 * align;
                    match &dc.kind {
                        DrawCallKind::Shapes { start, count } => {
                            self.pipeline2d.draw_range(
                                &mut overlay_pass,
                                &self.camera_bind_group,
                                &[cam_offset],
                                *start,
                                *count,
                            );
                        }
                        DrawCallKind::Sprites {
                            texture_id,
                            start,
                            count,
                        } => {
                            if let Some(tex) = self.texture_manager.get(*texture_id) {
                                self.pipeline_sprite.draw_range(
                                    &mut overlay_pass,
                                    &self.camera_bind_group,
                                    &[cam_offset],
                                    &tex.bind_group,
                                    *start,
                                    *count,
                                );
                            }
                        }
                    }
                }
                Ok(elapsed_ms_opt(overlay_start))
            },
            || RenderPassWorkload {
                draw_calls: saturating_u32(frame.draw_calls.len()),
                batches: saturating_u32(frame.draw_calls.len()),
                instances: saturating_u32(frame.shapes.len() + frame.sprites.len()),
                triangles: 0,
                upload_bytes: 0,
            }
        )?
        .map(|diagnostic| diagnostic.elapsed_ms)
        .unwrap_or(0.0);

        execute_render_node!(
            RenderPassKind::BackendSubmit,
            true,
            || {
                let queue_submit_start = if perf_enabled { Some(perf_now()) } else { None };
                self.queue.submit(std::iter::once(encoder.finish()));
                perf.queue_submit_cpu_ms = elapsed_ms_opt(queue_submit_start);
                let present_start = if perf_enabled { Some(perf_now()) } else { None };
                output.present();
                perf.present_cpu_ms = elapsed_ms_opt(present_start);
                Ok(perf.queue_submit_cpu_ms + perf.present_cpu_ms)
            },
            || RenderPassWorkload::default()
        )?;
        self.last_frame_graph_report = frame_graph.report();
        if perf_enabled {
            perf.submit_frame_ms = elapsed_ms_opt(frame_start);
            perf.graph_pass_count = self.last_frame_graph_report.pass_count;
            perf.graph_resource_count = self.last_frame_graph_report.resource_count;
            perf.graph_target_count = self.last_frame_graph_report.target_count;
            perf.graph_ready_target_count = self.last_frame_graph_report.ready_target_count;
            perf.graph_transient_target_count = self.last_frame_graph_report.transient_target_count;
            perf.graph_persistent_target_count =
                self.last_frame_graph_report.persistent_target_count;
            perf.graph_external_target_count = self.last_frame_graph_report.external_target_count;
            perf.graph_missing_read_count = self.last_frame_graph_report.missing_read_count;
            perf.graph_resize_generation = self.last_frame_graph_report.resize_generation;
            perf.graph_target_creates = self.last_frame_graph_report.resource_churn.target_creates;
            perf.graph_target_reuses = self.last_frame_graph_report.resource_churn.target_reuses;
            perf.graph_target_recreates =
                self.last_frame_graph_report.resource_churn.target_recreates;
            perf.graph_alias_reuses = self.last_frame_graph_report.resource_churn.alias_reuses;
            perf.text_draws = text_count;
            perf.sprite_draws = sprite_count;
            perf.primitive_draws = primitive_main_draw_calls;
            perf.water_draws = primitive_water_stats.batch_count;
            perf.water_instances = primitive_water_stats.instance_count;
            perf.water_triangles = primitive_water_stats.triangle_count;
            perf.primitive_chunks = saturating_u32(render_batch_plan.visible_chunks.len());
            perf.retained_scene_upserts = scene_upsert_count;
            perf.retained_scene_removals = scene_removal_count;
            perf.resident_chunk_rebuilds =
                resident_chunk_rebuild_count.saturating_add(render_batch_plan.resident_rebuilds);
            perf.shadow_cascades = if shadow_active {
                light_uniform.shadow_params2[3].max(1.0) as u32
            } else {
                0
            };
            let primitive_shadow_draw_count = primitive_shadow_draw_calls;
            let primitive_depth_draw_count = primitive_depth_draw_calls;
            perf.post_effects = 1
                + (post_bloom_strength > 0.0) as u32
                + (post_sharpen_strength > 0.0) as u32
                + (post_fxaa_strength > 0.0) as u32
                + contact_ao_active as u32
                + projected_decals_active as u32;
            perf.visible_objects = render_batch_plan.visible_objects;
            let mut model_mesh_draws = 0u32;
            let mut skinned_mesh_draws = 0u32;
            let mut instance_count = 0u32;
            let mut triangle_count = 0u32;
            for draw in &model_draws {
                let Some(gpu_model) = self.model_manager.get(draw.model_id) else {
                    continue;
                };
                for mesh in &gpu_model.meshes {
                    model_mesh_draws = model_mesh_draws.saturating_add(1);
                    if mesh.skinned {
                        skinned_mesh_draws = skinned_mesh_draws.saturating_add(1);
                    }
                    instance_count = instance_count.saturating_add(1);
                    triangle_count = triangle_count.saturating_add(mesh.index_count / 3);
                }
            }
            instance_count = instance_count.saturating_add(primitive_main_stats.instance_count);
            triangle_count = triangle_count.saturating_add(primitive_main_stats.triangle_count);
            instance_count = instance_count.saturating_add(primitive_water_stats.instance_count);
            triangle_count = triangle_count.saturating_add(primitive_water_stats.triangle_count);
            perf.model_draws = model_mesh_draws;
            perf.skinned_draws = skinned_mesh_draws;
            perf.instances = instance_count;
            perf.triangles = triangle_count;
            perf.draw_calls = saturating_u32(frame.draw_calls.len())
                .saturating_add(model_mesh_draws)
                .saturating_add(perf.primitive_draws)
                .saturating_add(perf.water_draws)
                .saturating_add(
                    perf.shadow_cascades
                        .saturating_mul(model_mesh_draws)
                        .saturating_add(primitive_shadow_draw_count),
                )
                .saturating_add(if depth_prepass_active {
                    model_mesh_draws + primitive_depth_draw_count
                } else {
                    0
                });
            let camera_upload = frame.cameras.len() * std::mem::size_of::<CameraUniform>();
            let shape_upload =
                frame.shapes.len() * std::mem::size_of::<crate::pipeline2d::ShapeInstance>();
            let sprite_upload = frame.sprites.len() * std::mem::size_of::<SpriteInstance>();
            let post_upload = std::mem::size_of::<PostUniform>()
                + std::mem::size_of::<PostDecalUniform>()
                + projected_decals.len() * std::mem::size_of::<PostDecalGpu>();
            perf.upload_bytes =
                saturating_u32(camera_upload + shape_upload + sprite_upload + post_upload);
            if perf.submit_frame_ms >= 16.0 {
                eprintln!(
                    "voplay renderer slow submit frame={} total={:.2}ms acquire={:.2}ms decode={:.2}ms scene={:.2}ms depth={:.2}ms shadow={:.2}ms main={:.2}ms(setup={:.2} sky={:.2} model={:.2} primitive={:.2} close={:.2}) post={:.2}ms overlay={:.2}ms queue={:.2}ms present={:.2}ms graphPasses={} graphResources={} graphTargets={}/{} slowestPass={} slowestPassMs={:.2} draws={} primitives={} chunks={} cascades={} postEffects={} upload={} flags=0x{:x}",
                    perf.frame_id,
                    perf.submit_frame_ms,
                    perf.surface_acquire_ms,
                    perf.decode_ms,
                    perf.scene_update_ms,
                    perf.depth_pass_ms,
                    perf.shadow_pass_ms,
                    perf.main_pass_ms,
                    perf.main_pass_setup_ms,
                    perf.main_skybox_ms,
                    perf.main_model_ms,
                    perf.main_primitive_ms,
                    perf.main_pass_close_ms,
                    perf.post_pass_ms,
                    perf.overlay_pass_ms,
                    perf.queue_submit_cpu_ms,
                    perf.present_cpu_ms,
                    self.last_frame_graph_report.pass_count,
                    self.last_frame_graph_report.resource_count,
                    self.last_frame_graph_report.ready_target_count,
                    self.last_frame_graph_report.target_count,
                    self.last_frame_graph_report.slowest_pass,
                    self.last_frame_graph_report.slowest_pass_ms,
                    perf.draw_calls,
                    perf.primitive_draws,
                    perf.primitive_chunks,
                    perf.shadow_cascades,
                    perf.post_effects,
                    perf.upload_bytes,
                    perf.diagnostic_flags,
                );
            }
            self.last_perf_packet = encode_renderer_perf_packet(&perf);
        } else {
            self.last_perf_packet.clear();
        }
        #[cfg(feature = "wasm")]
        if debug_scope_frame {
            let error_future = self.device.pop_error_scope();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(error) = error_future.await {
                    crate::externs::render::wasm_debug(&format!(
                        "voplay gpu validation #{}: {}",
                        debug_frame_count, error
                    ));
                }
            });
        }

        Ok(())
    }
}
