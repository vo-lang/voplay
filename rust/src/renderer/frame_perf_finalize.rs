use super::*;
use crate::draw_list::Frame2D;
use crate::render_world::RenderBatchPlan;
use crate::renderer_perf::PerfInstant;

pub(super) fn begin_frame_perf(
    perf_enabled: bool,
    debug_frame_count: u64,
    diagnostic_flags: u32,
) -> RendererPerfStats {
    if !perf_enabled {
        return RendererPerfStats::default();
    }
    RendererPerfStats {
        frame_id: debug_frame_count.min(u32::MAX as u64) as u32,
        display_tick: debug_frame_count.min(u32::MAX as u64) as u32,
        diagnostic_flags,
        ..RendererPerfStats::default()
    }
}

pub(super) struct FramePerfFinalizeContext<'a> {
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
    pub(super) frame_start: Option<PerfInstant>,
    pub(super) frame_graph: &'a FrameGraph,
    pub(super) frame: &'a Frame2D,
    pub(super) model_draws: &'a [ModelDraw],
    pub(super) render_batch_plan: &'a RenderBatchPlan,
    pub(super) light_uniform: &'a LightUniform,
    pub(super) primitive_main_stats: PrimitiveDrawStats,
    pub(super) primitive_transparent_stats: PrimitiveDrawStats,
    pub(super) primitive_water_stats: PrimitiveDrawStats,
    pub(super) primitive_main_submitted: bool,
    pub(super) primitive_shadow_draw_calls: u32,
    pub(super) primitive_depth_draw_calls: u32,
    pub(super) shadow_active: bool,
    pub(super) text_count: u32,
    pub(super) sprite_count: u32,
    pub(super) scene_upsert_count: u32,
    pub(super) scene_removal_count: u32,
    pub(super) resident_chunk_rebuild_count: u32,
    pub(super) post_bloom_strength: f32,
    pub(super) post_sharpen_strength: f32,
    pub(super) post_fxaa_strength: f32,
    pub(super) contact_ao_active: bool,
    pub(super) projected_decals_active: bool,
    pub(super) depth_prepass_active: bool,
    pub(super) planned_projected_decal_count: usize,
    pub(super) debug_frame_count: u64,
    pub(super) decode_command_count: u32,
    pub(super) decode_scene_mutation_count: u32,
    pub(super) decode_overlay_command_count: u32,
    pub(super) decode_elapsed_ms: f64,
    pub(super) material_group_count: u32,
    pub(super) frame_graph_build_ms: f64,
    pub(super) main_pass_ms: f64,
    pub(super) water_pass_ms: f64,
    pub(super) post_pass_ms: f64,
    pub(super) overlay_pass_ms: f64,
}

impl Renderer {
    pub(super) fn finalize_frame_perf(&mut self, mut context: FramePerfFinalizeContext<'_>) {
        let primitive_main_draw_calls = if context.primitive_main_submitted {
            context.primitive_main_stats.batch_count
        } else {
            0
        };
        self.last_frame_graph_report = context.frame_graph.report();
        if context.perf_enabled {
            self.populate_frame_perf_counters(&mut context, primitive_main_draw_calls);
        }
        let perf = context.perf;
        perf.main_pass_ms = context.main_pass_ms;
        perf.water_pass_ms = context.water_pass_ms;
        perf.post_pass_ms = context.post_pass_ms;
        perf.overlay_pass_ms = context.overlay_pass_ms;
        let perf_packet_ms = self.update_last_perf_packet(context.perf_enabled, perf);
        self.last_frame_pipeline = RenderFramePipeline::from_frame_metrics(RenderFrameMetrics {
            frame_id: context.debug_frame_count.min(u32::MAX as u64) as u32,
            command_count: context.decode_command_count,
            scene_mutation_count: context.decode_scene_mutation_count,
            overlay_command_count: context.decode_overlay_command_count,
            decode_ms: context.decode_elapsed_ms,
            visible_object_count: context.render_batch_plan.visible_objects,
            visible_chunk_count: saturating_u32(context.render_batch_plan.visible_chunks.len()),
            material_group_count: context.material_group_count,
            diagnostic_flags: perf.diagnostic_flags,
            graph_report: &self.last_frame_graph_report,
            graph_build_ms: context.frame_graph_build_ms,
            graph_execute_ms: self.last_frame_graph_report.total_pass_ms,
            perf_payload_version: RENDERER_PERF_PAYLOAD_VERSION,
            perf_packet_ms,
        });
    }

    fn populate_frame_perf_counters(
        &mut self,
        context: &mut FramePerfFinalizeContext<'_>,
        primitive_main_draw_calls: u32,
    ) {
        let primitive_draws = primitive_main_draw_calls
            .saturating_add(context.primitive_transparent_stats.batch_count);
        let water_draws = context.primitive_water_stats.batch_count;
        let primitive_chunks = saturating_u32(context.render_batch_plan.visible_chunks.len());
        let resident_chunk_rebuilds = context
            .resident_chunk_rebuild_count
            .saturating_add(context.render_batch_plan.resident_rebuilds);
        let culled_objects = context
            .render_batch_plan
            .frustum_culled_chunks
            .saturating_add(context.render_batch_plan.distance_culled_chunks);
        let shadow_cascades = if context.shadow_active {
            context.light_uniform.shadow_params2[3].max(1.0) as u32
        } else {
            0
        };
        let post_effects = 1
            + (context.post_bloom_strength > 0.0) as u32
            + (context.post_sharpen_strength > 0.0) as u32
            + (context.post_fxaa_strength > 0.0) as u32
            + context.contact_ao_active as u32
            + context.projected_decals_active as u32;
        let mesh_stats = self.frame_mesh_perf_stats(context.model_draws);
        let instance_count = mesh_stats
            .instances
            .saturating_add(context.primitive_main_stats.instance_count)
            .saturating_add(context.primitive_transparent_stats.instance_count)
            .saturating_add(context.primitive_water_stats.instance_count);
        let triangle_count = mesh_stats
            .triangles
            .saturating_add(context.primitive_main_stats.triangle_count)
            .saturating_add(context.primitive_transparent_stats.triangle_count)
            .saturating_add(context.primitive_water_stats.triangle_count);
        let draw_calls = self.frame_draw_call_count(
            context,
            mesh_stats.model_mesh_draws,
            primitive_draws,
            water_draws,
            shadow_cascades,
        );
        let upload_bytes = frame_upload_bytes(context);
        let perf = &mut *context.perf;
        perf.submit_frame_ms = elapsed_ms_opt(context.frame_start);
        perf.graph_pass_count = self.last_frame_graph_report.pass_count;
        perf.graph_resource_count = self.last_frame_graph_report.resource_count;
        perf.graph_target_count = self.last_frame_graph_report.target_count;
        perf.graph_ready_target_count = self.last_frame_graph_report.ready_target_count;
        perf.graph_transient_target_count = self.last_frame_graph_report.transient_target_count;
        perf.graph_persistent_target_count = self.last_frame_graph_report.persistent_target_count;
        perf.graph_external_target_count = self.last_frame_graph_report.external_target_count;
        perf.graph_missing_read_count = self.last_frame_graph_report.missing_read_count;
        perf.graph_resize_generation = self.last_frame_graph_report.resize_generation;
        perf.graph_target_creates = self.last_frame_graph_report.resource_churn.target_creates;
        perf.graph_target_reuses = self.last_frame_graph_report.resource_churn.target_reuses;
        perf.graph_target_recreates = self.last_frame_graph_report.resource_churn.target_recreates;
        perf.graph_alias_reuses = self.last_frame_graph_report.resource_churn.alias_reuses;
        perf.graph_skipped_passes = self.last_frame_graph_report.skipped_pass_count;
        perf.graph_failures = self.last_frame_graph_report.failure_count;
        let mut render_skips = self.last_frame_graph_report.skip_stats;
        render_skips.merge(context.render_batch_plan.skips);
        perf.render_skips = render_skips;
        perf.text_draws = context.text_count;
        perf.sprite_draws = context.sprite_count;
        perf.primitive_draws = primitive_draws;
        perf.water_draws = water_draws;
        perf.water_instances = context.primitive_water_stats.instance_count;
        perf.water_triangles = context.primitive_water_stats.triangle_count;
        perf.primitive_chunks = primitive_chunks;
        perf.retained_scene_upserts = context.scene_upsert_count;
        perf.retained_scene_removals = context.scene_removal_count;
        perf.resident_chunk_rebuilds = resident_chunk_rebuilds;
        perf.culled_objects = culled_objects;
        perf.shadow_cascades = shadow_cascades;
        perf.post_effects = post_effects;
        perf.visible_objects = context.render_batch_plan.visible_objects;
        perf.model_draws = mesh_stats.model_mesh_draws;
        perf.skinned_draws = mesh_stats.skinned_mesh_draws;
        perf.instances = instance_count;
        perf.triangles = triangle_count;
        perf.draw_calls = draw_calls;
        perf.upload_bytes = upload_bytes;
        self.log_slow_frame_if_needed(perf);
    }

    fn frame_mesh_perf_stats(&self, model_draws: &[ModelDraw]) -> FrameMeshPerfStats {
        let mut stats = FrameMeshPerfStats::default();
        for draw in model_draws {
            let Some(gpu_model) = self.model_manager.get(draw.model_id) else {
                continue;
            };
            for mesh in &gpu_model.meshes {
                stats.model_mesh_draws = stats.model_mesh_draws.saturating_add(1);
                if mesh.skinned {
                    stats.skinned_mesh_draws = stats.skinned_mesh_draws.saturating_add(1);
                }
                stats.instances = stats.instances.saturating_add(1);
                stats.triangles = stats.triangles.saturating_add(mesh.index_count / 3);
            }
        }
        stats
    }

    fn frame_draw_call_count(
        &self,
        context: &FramePerfFinalizeContext<'_>,
        model_mesh_draws: u32,
        primitive_draws: u32,
        water_draws: u32,
        shadow_cascades: u32,
    ) -> u32 {
        saturating_u32(context.frame.draw_calls.len())
            .saturating_add(model_mesh_draws)
            .saturating_add(primitive_draws)
            .saturating_add(water_draws)
            .saturating_add(
                shadow_cascades
                    .saturating_mul(model_mesh_draws)
                    .saturating_add(context.primitive_shadow_draw_calls),
            )
            .saturating_add(if context.depth_prepass_active {
                model_mesh_draws + context.primitive_depth_draw_calls
            } else {
                0
            })
    }

    fn log_slow_frame_if_needed(&self, perf: &RendererPerfStats) {
        if perf.submit_frame_ms < 16.0 {
            return;
        }
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

#[derive(Default)]
struct FrameMeshPerfStats {
    model_mesh_draws: u32,
    skinned_mesh_draws: u32,
    instances: u32,
    triangles: u32,
}

fn frame_upload_bytes(context: &FramePerfFinalizeContext<'_>) -> u32 {
    let camera_upload = context.frame.cameras.len() * std::mem::size_of::<CameraUniform>();
    let shape_upload =
        context.frame.shapes.len() * std::mem::size_of::<crate::pipeline2d::ShapeInstance>();
    let sprite_upload = context.frame.sprites.len() * std::mem::size_of::<SpriteInstance>();
    let post_upload = std::mem::size_of::<PostUniform>()
        + std::mem::size_of::<PostDecalUniform>()
        + context.planned_projected_decal_count * std::mem::size_of::<PostDecalGpu>();
    saturating_u32(camera_upload + shape_upload + sprite_upload + post_upload)
}
