use super::frame_2d_upload::Frame2DUploadContext;
use super::frame_decode::FrameDecodeOutput;
use super::frame_graph_plan::{FrameGraphPlanDesc, FrameGraphPlanOutput};
use super::frame_pass_sequence::FramePassSequenceContext;
use super::frame_perf_finalize::{begin_frame_perf, FramePerfFinalizeContext};
use super::frame_workload_plan::{FrameWorkloadPlan, FrameWorkloadPlanContext};
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
            transaction,
        } = self.decode_frame_commands(
            data,
            screen_w,
            screen_h,
            aspect,
            debug_frame_count,
            perf_enabled,
        );
        let transaction_report = self.apply_frame_transaction(transaction);
        let resident_chunk_rebuild_count = transaction_report.resident_chunk_rebuild_count;
        perf.decode_ms = decode_stage.elapsed_ms;
        let command_count = decode_stage.command_count;

        let frame_id = debug_frame_count.min(u32::MAX as u64) as u32;
        let FrameWorkloadPlan {
            mut primitive_depth_draws,
            mut primitive_shadow_draws,
            primitive_depth_chunks,
            primitive_shadow_chunks,
            render_batch_plan,
            planned_model_draws,
            planned_primitive_draws,
            planned_primitive_chunks,
            planned_water_draws,
            planned_water_chunks,
            planned_projected_decals,
            contact_ao_active,
            projected_decals_active,
            post_depth_active,
            depth_prepass_active,
            material_group_count,
            water_pass_active,
            transparent_pass_active,
        } = self.prepare_frame_workload_plan(FrameWorkloadPlanContext {
            perf_enabled,
            perf: &mut perf,
            perf_overrides,
            shadow_enabled: &mut shadow_enabled,
            shadow_strength: &mut shadow_strength,
            shadow_quality: &mut shadow_quality,
            post_bloom_strength: &mut post_bloom_strength,
            post_sharpen_strength: &mut post_sharpen_strength,
            post_fxaa_strength: &mut post_fxaa_strength,
            post_contact_ao_strength: &mut post_contact_ao_strength,
            post_contact_ao_quality: &mut post_contact_ao_quality,
            model_draws: &mut model_draws,
            projected_decals: &mut projected_decals,
            projected_decal_atlas_bindings: &mut projected_decal_atlas_bindings,
            retained_scene_draws: &retained_scene_draws,
            camera3d_uniform: camera3d_uniform.as_ref(),
            frame_id,
        });
        let mut primitive_depth_draw_calls = 0u32;
        let mut primitive_shadow_draw_calls = 0u32;
        let mut primitive_main_submitted = false;
        let mut primitive_main_stats = PrimitiveDrawStats::default();
        let mut primitive_transparent_stats = PrimitiveDrawStats::default();
        let mut primitive_water_stats = PrimitiveDrawStats::default();

        self.font_manager
            .ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        let frame = self.draw_list.resolve();
        let align = self.upload_frame_2d_instances(Frame2DUploadContext {
            frame: &frame,
            debug_frame_count,
            data_len: data.len(),
            command_count,
            camera3d_enabled: camera3d_uniform.is_some(),
            model_command_count,
            scene_upsert_count,
            scene_draw_count,
            planned_model_draw_count: planned_model_draws.len(),
            planned_primitive_draw_count: planned_primitive_draws.len(),
            planned_primitive_chunk_count: planned_primitive_chunks.len(),
            skybox_count,
            planned_projected_decal_count: planned_projected_decals.len(),
            diagnostic_flags: perf_overrides.flags(),
            rect_count,
            circle_count,
            line_count,
            text_count,
            sprite_count,
            clear_color,
        });

        let FrameGraphPlanOutput {
            mut frame_graph,
            nodes: frame_graph_nodes,
            build_ms: frame_graph_build_ms,
        } = self.build_frame_graph_plan(FrameGraphPlanDesc {
            frame_id,
            diagnostic_flags: perf_overrides.flags(),
            perf_enabled,
            post_depth_active,
            depth_prepass_active,
            shadow_enabled,
            camera3d_enabled: camera3d_uniform.is_some(),
            model_draws_empty: model_draws.is_empty(),
            primitive_shadow_draws_empty: primitive_shadow_draws.is_empty(),
            primitive_shadow_chunks_empty: primitive_shadow_chunks.is_empty(),
            transparent_pass_active,
            water_pass_active,
        })?;
        let (output, view, surface_acquire_ms) = self.acquire_surface_texture(perf_enabled)?;
        perf.surface_acquire_ms = surface_acquire_ms;
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });
        let pass_timings = self.execute_frame_pass_sequence(FramePassSequenceContext {
            frame_graph: &mut frame_graph,
            nodes: &frame_graph_nodes,
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
            projected_decals: &planned_projected_decals,
            projected_decal_atlas_binding_count: projected_decal_atlas_bindings.len() as u32,
            shadow_resolution,
            shadow_quality,
            shadow_distance,
            shadow_fade,
            shadow_softness,
            shadow_strength,
            post_depth_active,
            aspect,
            perf_enabled,
            perf: &mut perf,
            primitive_depth_draw_calls: &mut primitive_depth_draw_calls,
            primitive_shadow_draw_calls: &mut primitive_shadow_draw_calls,
            primitive_main_stats: &mut primitive_main_stats,
            primitive_transparent_stats: &mut primitive_transparent_stats,
            primitive_main_submitted: &mut primitive_main_submitted,
            primitive_water_stats: &mut primitive_water_stats,
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
        })?;
        perf.depth_pass_ms = pass_timings.depth_pass_ms;
        perf.shadow_pass_ms = pass_timings.shadow_pass_ms;
        self.finalize_frame_perf(FramePerfFinalizeContext {
            perf_enabled,
            perf: &mut perf,
            frame_start,
            frame_graph: &frame_graph,
            frame: &frame,
            model_draws: &model_draws,
            render_batch_plan: &render_batch_plan,
            light_uniform: &light_uniform,
            primitive_main_stats,
            primitive_transparent_stats,
            primitive_water_stats,
            primitive_main_submitted,
            primitive_shadow_draw_calls,
            primitive_depth_draw_calls,
            shadow_active: pass_timings.shadow_active,
            text_count,
            sprite_count,
            scene_upsert_count,
            scene_removal_count,
            resident_chunk_rebuild_count,
            post_bloom_strength,
            post_sharpen_strength,
            post_fxaa_strength,
            contact_ao_active,
            projected_decals_active,
            depth_prepass_active,
            planned_projected_decal_count: planned_projected_decals.len(),
            debug_frame_count,
            decode_command_count: decode_stage.command_count,
            decode_scene_mutation_count: decode_stage.scene_mutation_count,
            decode_overlay_command_count: decode_stage.overlay_command_count,
            decode_elapsed_ms: decode_stage.elapsed_ms,
            material_group_count,
            frame_graph_build_ms,
            main_pass_ms: pass_timings.main_pass_ms,
            water_pass_ms: pass_timings.water_pass_ms,
            post_pass_ms: pass_timings.post_pass_ms,
            overlay_pass_ms: pass_timings.overlay_pass_ms,
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
pub(super) struct FrameSubmitOrchestrator;
impl FrameSubmitOrchestrator {
    pub(super) fn run(renderer: &mut Renderer, data: &[u8]) -> Result<(), String> {
        renderer.run_frame_orchestrator(data)
    }
}
