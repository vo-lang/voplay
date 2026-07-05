use super::frame_decode::FrameDecodeOutput;
use super::pass_dispatch::FramePassDispatcher;
use super::post_pass::{PostPassSetup, PostPassSetupContext};
use super::*;

impl Renderer {
    pub(super) fn run_frame_orchestrator(&mut self, data: &[u8]) -> Result<(), String> {
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

        let screen_w = self.screen_width;
        let screen_h = self.screen_height;

        let aspect = screen_w / screen_h;
        let FrameDecodeOutput {
            stage: decode_stage,
            clear_color,
            camera3d_uniform,
            camera3d_state,
            skybox_cubemap_id,
            mut shadow_enabled,
            shadow_resolution,
            mut shadow_strength,
            shadow_softness,
            shadow_distance,
            shadow_fade,
            mut shadow_quality,
            post_bloom_threshold,
            mut post_bloom_strength,
            mut post_sharpen_strength,
            mut post_fxaa_strength,
            mut post_contact_ao_strength,
            post_contact_ao_radius,
            post_contact_ao_depth_scale,
            post_contact_ao_detail_strength,
            post_contact_ao_detail_radius,
            post_contact_ao_normal_bias,
            mut post_contact_ao_quality,
            mut light_uniform,
            mut model_draws,
            mut projected_decals,
            mut projected_decal_atlas_bindings,
            retained_scene_draws,
            rect_count,
            circle_count,
            line_count,
            text_count,
            sprite_count,
            model_command_count,
            projected_decal_count: _projected_decal_count,
            scene_upsert_count,
            scene_removal_count,
            scene_draw_count,
            skybox_count,
            resident_chunk_rebuild_count,
        } = self.decode_frame_commands(
            data,
            screen_w,
            screen_h,
            aspect,
            debug_frame_count,
            perf_enabled,
        );
        perf.decode_ms = decode_stage.elapsed_ms;
        let command_count = decode_stage.command_count;

        let mut primitive_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_depth_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_shadow_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_chunk_info: Vec<PrimitiveChunkBatchInfo> = Vec::new();
        let mut primitive_depth_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_shadow_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_main_draw_calls = 0u32;
        let mut primitive_depth_draw_calls = 0u32;
        let mut primitive_shadow_draw_calls = 0u32;
        let mut primitive_main_submitted = false;
        let mut primitive_main_stats = PrimitiveDrawStats::default();
        let mut primitive_transparent_stats = PrimitiveDrawStats::default();
        let mut primitive_water_stats = PrimitiveDrawStats::default();
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
            self.render_world
                .collect_scene_primitive_draws_with_chunk_info(
                    *scene_id,
                    None,
                    &mut primitive_draws,
                    &mut primitive_chunks,
                    &mut primitive_chunk_info,
                );
        }
        perf.scene_update_ms = elapsed_ms_opt(scene_update_start);
        let frame_id = debug_frame_count.min(u32::MAX as u64) as u32;
        let terrain_batch_inputs =
            RenderBatchPlanner::terrain_inputs(frame_id, &model_draws, &self.model_manager);
        let decal_batch_inputs = RenderBatchPlanner::decal_inputs(frame_id, &projected_decals);
        let render_batch_plan = RenderBatchPlanner::build(
            frame_id,
            0,
            &model_draws,
            &terrain_batch_inputs,
            &primitive_draws,
            &primitive_chunks,
            &primitive_chunk_info,
            &decal_batch_inputs,
            camera3d_uniform.as_ref(),
            RenderBatchQualityProfile::default(),
        );
        let planned_model_draws = render_batch_plan.model_batches(&model_draws);
        let _planned_terrain_draws = render_batch_plan.terrain_batches(&model_draws);
        let planned_primitive_draws = render_batch_plan.primitive_draw_batches(&primitive_draws);
        let planned_primitive_chunks = render_batch_plan.primitive_chunk_batches(&primitive_chunks);
        let planned_water_draws = render_batch_plan.water_draw_batches(&primitive_draws);
        let planned_water_chunks = render_batch_plan.water_chunk_batches(&primitive_chunks);
        let planned_projected_decals = render_batch_plan.decal_batches(&projected_decals);
        let projected_decals_active = !planned_projected_decals.is_empty();
        let post_depth_active = contact_ao_active || projected_decals_active;
        let depth_prepass_active = MAIN_SAMPLE_COUNT > 1 && post_depth_active;
        if depth_prepass_active {
            for scene_id in &retained_scene_draws {
                self.render_world.collect_scene_primitive_depth_draws(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_depth_draws,
                    &mut primitive_depth_chunks,
                );
            }
            primitive_depth_chunks.retain(|chunk| planned_primitive_chunks.contains(chunk));
        }
        if primitive_shadows_enabled {
            for scene_id in &retained_scene_draws {
                self.render_world.collect_scene_primitive_shadow_objects(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_shadow_draws,
                );
                self.render_world
                    .collect_scene_primitive_shadow_chunks_from_candidates(
                        *scene_id,
                        camera3d_uniform.as_ref(),
                        &planned_primitive_chunks,
                        &mut primitive_shadow_chunks,
                    );
            }
        }
        let mut material_groups = Vec::<u32>::new();
        for chunk in &render_batch_plan.visible_chunks {
            if !material_groups.contains(&chunk.material_group) {
                material_groups.push(chunk.material_group);
            }
        }
        let material_group_count = saturating_u32(material_groups.len());
        let water_pass_active = primitives_enabled
            && camera3d_uniform.is_some()
            && self
                .primitive_pipeline
                .has_water_surface(&planned_water_draws, &planned_water_chunks);
        let transparent_pass_active = primitives_enabled
            && camera3d_uniform.is_some()
            && self.primitive_pipeline.has_translucent_surface(
                &planned_primitive_draws,
                &planned_primitive_chunks,
                &self.model_manager,
                &self.texture_manager,
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
                planned_model_draws.len(),
                planned_primitive_draws.len(),
                planned_primitive_chunks.len(),
                skybox_count,
                planned_projected_decals.len(),
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

        let graph_build_start = if perf_enabled { Some(perf_now()) } else { None };
        let mut frame_graph = FrameGraph::single_view(
            debug_frame_count.min(u32::MAX as u64) as u32,
            perf_overrides.flags(),
        );
        frame_graph.declare_external_target(RES_SURFACE_COLOR, true);
        frame_graph.declare_target(RES_MAIN_COLOR, self.resources.main_color_ready());
        frame_graph.declare_target(RES_DEPTH, self.resources.depth_view().is_some());
        frame_graph.declare_target(RES_SHADOW_MAP, true);
        frame_graph.declare_target(RES_POST_COLOR, self.resources.post_color_view().is_some());
        frame_graph.declare_external_target(RES_OVERLAY, true);
        frame_graph.declare_transient_target(RES_CAPTURE, false);
        frame_graph.declare_external_target(RES_READBACK, false);
        frame_graph.declare_target(
            RES_RECEIVER_MASK,
            !post_depth_active || self.resources.receiver_mask_view().is_some(),
        );
        frame_graph.declare_target(
            RES_SURFACE_PROPS,
            !post_depth_active || self.resources.surface_props_view().is_some(),
        );
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
            transparent_pass_active,
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
        let frame_graph_build_ms = elapsed_ms_opt(graph_build_start);
        let depth_node = frame_graph
            .node(RenderPassKind::DepthPrepass)
            .ok_or_else(|| {
                format!(
                    "voplay: missing frame graph node {}",
                    RenderPassKind::DepthPrepass.name()
                )
            })?;
        let shadow_node = frame_graph.node(RenderPassKind::Shadow).ok_or_else(|| {
            format!(
                "voplay: missing frame graph node {}",
                RenderPassKind::Shadow.name()
            )
        })?;
        let (mut output, view, surface_acquire_ms) = self.acquire_surface_texture(perf_enabled)?;
        perf.surface_acquire_ms = surface_acquire_ms;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });
        let mut shadow_active = false;
        let depth_pass_ms;
        let shadow_pass_ms;
        let main_pass_ms;
        let water_pass_ms;
        let post_pass_ms;
        let overlay_pass_ms;
        {
            let mut dispatcher = FramePassDispatcher {
                renderer: self,
                encoder: Some(encoder),
                output: Some(output),
                surface_view: &view,
                frame: &frame,
                camera_alignment: align,
                clear_color,
                camera3d_uniform: camera3d_uniform.as_ref(),
                camera3d_state,
                skybox_cubemap_id,
                light_uniform: &mut light_uniform,
                planned_model_draws: &planned_model_draws,
                primitive_depth_draws: &mut primitive_depth_draws,
                primitive_depth_chunks: &primitive_depth_chunks,
                primitive_shadow_draws: &mut primitive_shadow_draws,
                primitive_shadow_chunks: &primitive_shadow_chunks,
                retained_scene_draws: &retained_scene_draws,
                planned_primitive_draws: &planned_primitive_draws,
                planned_primitive_chunks: &planned_primitive_chunks,
                planned_water_draws: &planned_water_draws,
                planned_water_chunks: &planned_water_chunks,
                projected_decal_atlas_bindings: &projected_decal_atlas_bindings,
                shadow_resolution,
                shadow_quality,
                shadow_distance,
                shadow_fade,
                shadow_softness,
                shadow_strength,
                main_aux_targets_enabled: post_depth_active,
                aspect,
                perf_enabled,
                perf: &mut perf,
                primitive_depth_draw_calls: &mut primitive_depth_draw_calls,
                primitive_shadow_draw_calls: &mut primitive_shadow_draw_calls,
                primitive_main_stats: &mut primitive_main_stats,
                primitive_transparent_stats: &mut primitive_transparent_stats,
                primitive_main_submitted: &mut primitive_main_submitted,
                primitive_water_stats: &mut primitive_water_stats,
                shadow_active: &mut shadow_active,
            };
            depth_pass_ms = frame_graph
                .executor()
                .execute_node(&depth_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            shadow_pass_ms = frame_graph
                .executor()
                .execute_node(&shadow_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            let frame_parts = dispatcher.into_frame_parts()?;
            encoder = frame_parts.0;
            output = frame_parts.1;
        }
        perf.depth_pass_ms = depth_pass_ms;
        perf.shadow_pass_ms = shadow_pass_ms;
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
        {
            let mut post_setup = PostPassSetupContext {
                renderer: self,
                camera3d_uniform: camera3d_uniform.as_ref(),
                camera3d_state,
                light_uniform: &light_uniform,
                projected_decals: &planned_projected_decals,
                projected_decal_atlas_binding_count: projected_decal_atlas_bindings.len() as u32,
                bloom_threshold: post_bloom_threshold,
                bloom_strength: post_bloom_strength,
                sharpen_strength: post_sharpen_strength,
                fxaa_strength: post_fxaa_strength,
                contact_ao_strength: post_contact_ao_strength,
                contact_ao_radius: post_contact_ao_radius,
                contact_ao_depth_scale: post_contact_ao_depth_scale,
                contact_ao_detail_strength: post_contact_ao_detail_strength,
                contact_ao_detail_radius: post_contact_ao_detail_radius,
                contact_ao_normal_bias: post_contact_ao_normal_bias,
                contact_ao_quality: post_contact_ao_quality,
            };
            PostPassSetup::upload_uniforms(&mut post_setup);
        }
        let main_node = frame_graph
            .node(RenderPassKind::MainOpaque)
            .ok_or_else(|| {
                format!(
                    "voplay: missing frame graph node {}",
                    RenderPassKind::MainOpaque.name()
                )
            })?;
        let transparent_node = frame_graph
            .node(RenderPassKind::MainTransparent)
            .ok_or_else(|| {
                format!(
                    "voplay: missing frame graph node {}",
                    RenderPassKind::MainTransparent.name()
                )
            })?;
        let water_node = frame_graph.node(RenderPassKind::Water).ok_or_else(|| {
            format!(
                "voplay: missing frame graph node {}",
                RenderPassKind::Water.name()
            )
        })?;
        let post_node = frame_graph.node(RenderPassKind::Post).ok_or_else(|| {
            format!(
                "voplay: missing frame graph node {}",
                RenderPassKind::Post.name()
            )
        })?;
        let overlay_node = frame_graph.node(RenderPassKind::Overlay).ok_or_else(|| {
            format!(
                "voplay: missing frame graph node {}",
                RenderPassKind::Overlay.name()
            )
        })?;
        let backend_node = frame_graph
            .node(RenderPassKind::BackendSubmit)
            .ok_or_else(|| {
                format!(
                    "voplay: missing frame graph node {}",
                    RenderPassKind::BackendSubmit.name()
                )
            })?;
        {
            let mut dispatcher = FramePassDispatcher {
                renderer: self,
                encoder: Some(encoder),
                output: Some(output),
                surface_view: &view,
                frame: &frame,
                camera_alignment: align,
                clear_color,
                camera3d_uniform: camera3d_uniform.as_ref(),
                camera3d_state,
                skybox_cubemap_id,
                light_uniform: &mut light_uniform,
                planned_model_draws: &planned_model_draws,
                primitive_depth_draws: &mut primitive_depth_draws,
                primitive_depth_chunks: &primitive_depth_chunks,
                primitive_shadow_draws: &mut primitive_shadow_draws,
                primitive_shadow_chunks: &primitive_shadow_chunks,
                retained_scene_draws: &retained_scene_draws,
                planned_primitive_draws: &planned_primitive_draws,
                planned_primitive_chunks: &planned_primitive_chunks,
                planned_water_draws: &planned_water_draws,
                planned_water_chunks: &planned_water_chunks,
                projected_decal_atlas_bindings: &projected_decal_atlas_bindings,
                shadow_resolution,
                shadow_quality,
                shadow_distance,
                shadow_fade,
                shadow_softness,
                shadow_strength,
                main_aux_targets_enabled,
                aspect,
                perf_enabled,
                perf: &mut perf,
                primitive_depth_draw_calls: &mut primitive_depth_draw_calls,
                primitive_shadow_draw_calls: &mut primitive_shadow_draw_calls,
                primitive_main_stats: &mut primitive_main_stats,
                primitive_transparent_stats: &mut primitive_transparent_stats,
                primitive_main_submitted: &mut primitive_main_submitted,
                primitive_water_stats: &mut primitive_water_stats,
                shadow_active: &mut shadow_active,
            };
            main_pass_ms = frame_graph
                .executor()
                .execute_node(&main_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            frame_graph
                .executor()
                .execute_node(&transparent_node, true, &mut dispatcher)?;
            water_pass_ms = frame_graph
                .executor()
                .execute_node(&water_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            post_pass_ms = frame_graph
                .executor()
                .execute_node(&post_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            overlay_pass_ms = frame_graph
                .executor()
                .execute_node(&overlay_node, true, &mut dispatcher)?
                .map(|diagnostic| diagnostic.elapsed_ms)
                .unwrap_or(0.0);
            frame_graph
                .executor()
                .execute_node(&backend_node, true, &mut dispatcher)?;
        }
        perf.main_pass_ms = main_pass_ms;
        if primitive_main_submitted {
            primitive_main_draw_calls = primitive_main_stats.batch_count;
        }
        perf.water_pass_ms = water_pass_ms;
        perf.post_pass_ms = post_pass_ms;
        perf.overlay_pass_ms = overlay_pass_ms;
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
            perf.primitive_draws =
                primitive_main_draw_calls.saturating_add(primitive_transparent_stats.batch_count);
            perf.water_draws = primitive_water_stats.batch_count;
            perf.water_instances = primitive_water_stats.instance_count;
            perf.water_triangles = primitive_water_stats.triangle_count;
            perf.primitive_chunks = saturating_u32(render_batch_plan.visible_chunks.len());
            perf.retained_scene_upserts = scene_upsert_count;
            perf.retained_scene_removals = scene_removal_count;
            perf.resident_chunk_rebuilds =
                resident_chunk_rebuild_count.saturating_add(render_batch_plan.resident_rebuilds);
            perf.culled_objects = render_batch_plan
                .frustum_culled_chunks
                .saturating_add(render_batch_plan.distance_culled_chunks);
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
            instance_count =
                instance_count.saturating_add(primitive_transparent_stats.instance_count);
            triangle_count =
                triangle_count.saturating_add(primitive_transparent_stats.triangle_count);
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
                + planned_projected_decals.len() * std::mem::size_of::<PostDecalGpu>();
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
        }
        let perf_packet_ms = self.update_last_perf_packet(perf_enabled, &perf);
        self.last_frame_pipeline = RenderFramePipeline::from_frame_metrics(
            debug_frame_count.min(u32::MAX as u64) as u32,
            decode_stage.command_count,
            decode_stage.scene_mutation_count,
            decode_stage.overlay_command_count,
            decode_stage.elapsed_ms,
            render_batch_plan.visible_objects,
            saturating_u32(render_batch_plan.visible_chunks.len()),
            material_group_count,
            perf.diagnostic_flags,
            &self.last_frame_graph_report,
            frame_graph_build_ms,
            self.last_frame_graph_report.total_pass_ms,
            RENDERER_PERF_PAYLOAD_VERSION,
            perf_packet_ms,
        );
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

    fn acquire_surface_texture(
        &mut self,
        perf_enabled: bool,
    ) -> Result<(wgpu::SurfaceTexture, wgpu::TextureView, f64), String> {
        let acquire_start = if perf_enabled { Some(perf_now()) } else { None };
        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.surface.get_current_texture().map_err(|error| {
                    format!(
                        "voplay: get_current_texture recovery after lost/outdated failed: {error}"
                    )
                })?
            }
            Err(wgpu::SurfaceError::Timeout) => {
                return Err(
                    "voplay: get_current_texture timeout; frame skipped before submit".to_string(),
                );
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err(
                    "voplay: get_current_texture out of memory; renderer cannot recover"
                        .to_string(),
                );
            }
            Err(error) => return Err(format!("voplay: get_current_texture failed: {error}")),
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok((output, view, elapsed_ms_opt(acquire_start)))
    }
}
