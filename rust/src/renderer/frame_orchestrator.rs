use super::frame_2d_upload::Frame2DUploadContext;
use super::frame_decode::FrameDecodeOutput;
use super::frame_graph_plan::{FrameGraphPlanDesc, FrameGraphPlanOutput};
use super::frame_pass_sequence::FramePassSequenceContext;
use super::frame_perf_finalize::{begin_frame_perf, FramePerfFinalizeContext};
use super::frame_workload_plan::FrameWorkloadPlanContext;
use super::*;

impl Renderer {
    pub(super) fn run_frame_orchestrator(&mut self, data: &[u8]) -> Result<(), String> {
        let perf_enabled = self.perf_stats_enabled;
        let perf_overrides = RendererPerfOverrides::current();
        let frame_start = if perf_enabled { Some(perf_now()) } else { None };
        self.debug_frame_count = self.debug_frame_count.wrapping_add(1);
        let debug_frame_count = self.debug_frame_count;
        let mut perf = begin_frame_perf(perf_enabled, debug_frame_count, perf_overrides.flags());
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
            transaction,
            view: frame_view,
            mut shadow,
            mut post,
            mut scene,
            counts,
        } = self
            .decode_frame_commands(
                data,
                screen_w,
                screen_h,
                aspect,
                debug_frame_count,
                perf_enabled,
            )
            .map_err(|error| error.to_string())?;
        let transaction_report = self.apply_frame_transaction(transaction);
        let resident_chunk_rebuild_count = transaction_report.resident_chunk_rebuild_count;
        perf.decode_ms = decode_stage.elapsed_ms;
        let command_count = decode_stage.command_count;

        let frame_id = debug_frame_count.min(u32::MAX as u64) as u32;
        let mut workload = self.prepare_frame_workload_plan(FrameWorkloadPlanContext {
            perf_enabled,
            perf: &mut perf,
            perf_overrides,
            view: &frame_view,
            shadow: &mut shadow,
            post: &mut post,
            scene: &mut scene,
            frame_id,
        });
        self.font_manager
            .ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        let frame = self.draw_list.resolve();
        let align = self.upload_frame_2d_instances(Frame2DUploadContext {
            frame: &frame,
            debug_frame_count,
            data_len: data.len(),
            command_count,
            camera3d_enabled: frame_view.camera3d_uniform.is_some(),
            model_command_count: counts.model_command_count,
            scene_upsert_count: counts.scene_upsert_count,
            scene_draw_count: counts.scene_draw_count,
            planned_model_draw_count: workload.planned_model_draws.len(),
            planned_primitive_draw_count: workload.planned_primitive_draws.len(),
            planned_primitive_chunk_count: workload.planned_primitive_chunks.len(),
            skybox_count: counts.skybox_count,
            planned_projected_decal_count: workload.planned_projected_decals.len(),
            diagnostic_flags: perf_overrides.flags(),
            rect_count: counts.rect_count,
            circle_count: counts.circle_count,
            line_count: counts.line_count,
            text_count: counts.text_count,
            sprite_count: counts.sprite_count,
            clear_color: frame_view.clear_color,
        });

        let FrameGraphPlanOutput {
            mut frame_graph,
            build_ms: frame_graph_build_ms,
        } = self.build_frame_graph_plan(FrameGraphPlanDesc {
            frame_id,
            diagnostic_flags: perf_overrides.flags(),
            perf_enabled,
            post_depth_active: workload.post_depth_active,
            depth_prepass_active: workload.depth_prepass_active,
            shadow_enabled: shadow.shadow_enabled,
            camera3d_enabled: frame_view.camera3d_uniform.is_some(),
            model_draws_empty: scene.model_draws.is_empty(),
            primitive_shadow_draws_empty: workload.primitive_shadow_draws.is_empty(),
            primitive_shadow_chunks_empty: workload.primitive_shadow_chunks.is_empty(),
            transparent_pass_active: workload.transparent_pass_active,
            water_pass_active: workload.water_pass_active,
        })?;
        let (output, view, surface_acquire_ms) = self.acquire_surface_texture(perf_enabled)?;
        perf.surface_acquire_ms = surface_acquire_ms;
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });
        let pass_result = self.execute_frame_pass_sequence(FramePassSequenceContext {
            frame_graph: &mut frame_graph,
            encoder: Some(encoder),
            output: Some(output),
            surface_view: &view,
            frame: &frame,
            camera_alignment: align,
            aspect,
            perf_enabled,
            perf: &mut perf,
            view: &frame_view,
            shadow: &shadow,
            post: &post,
            scene: &mut scene,
            workload: &mut workload,
        })?;
        perf.depth_pass_ms = pass_result.timings.depth_pass_ms;
        perf.shadow_pass_ms = pass_result.timings.shadow_pass_ms;
        self.finalize_frame_perf(FramePerfFinalizeContext {
            perf_enabled,
            perf: &mut perf,
            frame_start,
            frame_graph: &frame_graph,
            frame: &frame,
            model_draws: &scene.model_draws,
            render_batch_plan: &workload.render_batch_plan,
            light_uniform: &scene.light_uniform,
            primitive_main_stats: pass_result.stats.primitive_main,
            primitive_transparent_stats: pass_result.stats.primitive_transparent,
            primitive_water_stats: pass_result.stats.primitive_water,
            primitive_main_submitted: pass_result.stats.primitive_main_submitted,
            primitive_shadow_draw_calls: pass_result.stats.primitive_shadow_draw_calls,
            primitive_depth_draw_calls: pass_result.stats.primitive_depth_draw_calls,
            shadow_active: pass_result.timings.shadow_active,
            text_count: counts.text_count,
            sprite_count: counts.sprite_count,
            scene_upsert_count: counts.scene_upsert_count,
            scene_removal_count: counts.scene_removal_count,
            resident_chunk_rebuild_count,
            post_bloom_strength: post.post_bloom_strength,
            post_sharpen_strength: post.post_sharpen_strength,
            post_fxaa_strength: post.post_fxaa_strength,
            contact_ao_active: workload.contact_ao_active,
            projected_decals_active: workload.projected_decals_active,
            depth_prepass_active: workload.depth_prepass_active,
            planned_projected_decal_count: workload.planned_projected_decals.len(),
            debug_frame_count,
            decode_command_count: decode_stage.command_count,
            decode_scene_mutation_count: decode_stage.scene_mutation_count,
            decode_overlay_command_count: decode_stage.overlay_command_count,
            decode_elapsed_ms: decode_stage.elapsed_ms,
            material_group_count: workload.material_group_count,
            frame_graph_build_ms,
            main_pass_ms: pass_result.timings.main_pass_ms,
            water_pass_ms: pass_result.timings.water_pass_ms,
            post_pass_ms: pass_result.timings.post_pass_ms,
            overlay_pass_ms: pass_result.timings.overlay_pass_ms,
        });
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
