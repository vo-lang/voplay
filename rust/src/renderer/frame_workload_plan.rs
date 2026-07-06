use super::*;
use crate::render_world::RenderBatchPlan;

pub(super) struct FrameWorkloadPlanContext<'a> {
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
    pub(super) perf_overrides: RendererPerfOverrides,
    pub(super) shadow_enabled: &'a mut bool,
    pub(super) shadow_strength: &'a mut f32,
    pub(super) shadow_quality: &'a mut u32,
    pub(super) post_bloom_strength: &'a mut f32,
    pub(super) post_sharpen_strength: &'a mut f32,
    pub(super) post_fxaa_strength: &'a mut f32,
    pub(super) post_contact_ao_strength: &'a mut f32,
    pub(super) post_contact_ao_quality: &'a mut u32,
    pub(super) model_draws: &'a mut Vec<ModelDraw>,
    pub(super) projected_decals: &'a mut Vec<PostDecalGpu>,
    pub(super) projected_decal_atlas_bindings: &'a mut Vec<ProjectedDecalAtlasBinding>,
    pub(super) retained_scene_draws: &'a [u32],
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) frame_id: u32,
}

pub(super) struct FrameWorkloadPlan {
    pub(super) primitive_depth_draws: Vec<PrimitiveDraw>,
    pub(super) primitive_shadow_draws: Vec<PrimitiveDraw>,
    pub(super) primitive_depth_chunks: Vec<PrimitiveChunkRef>,
    pub(super) primitive_shadow_chunks: Vec<PrimitiveChunkRef>,
    pub(super) render_batch_plan: RenderBatchPlan,
    pub(super) planned_model_draws: Vec<ModelDraw>,
    pub(super) planned_primitive_draws: Vec<PrimitiveDraw>,
    pub(super) planned_primitive_chunks: Vec<PrimitiveChunkRef>,
    pub(super) planned_water_draws: Vec<PrimitiveDraw>,
    pub(super) planned_water_chunks: Vec<PrimitiveChunkRef>,
    pub(super) planned_projected_decals: Vec<PostDecalGpu>,
    pub(super) contact_ao_active: bool,
    pub(super) projected_decals_active: bool,
    pub(super) post_depth_active: bool,
    pub(super) depth_prepass_active: bool,
    pub(super) material_group_count: u32,
    pub(super) water_pass_active: bool,
    pub(super) transparent_pass_active: bool,
}

impl Renderer {
    pub(super) fn prepare_frame_workload_plan(
        &mut self,
        mut context: FrameWorkloadPlanContext<'_>,
    ) -> FrameWorkloadPlan {
        apply_frame_perf_overrides(&mut context);
        let contact_ao_active =
            *context.post_contact_ao_strength > 0.001 && *context.post_contact_ao_quality > 0;
        let primitives_enabled = !context.perf_overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVES);
        let primitive_shadows_enabled = primitives_enabled
            && *context.shadow_enabled
            && !context
                .perf_overrides
                .has(RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS);
        let mut primitive_draws = Vec::new();
        let mut primitive_chunks = Vec::new();
        let mut primitive_chunk_info = Vec::new();
        self.collect_frame_scene_workloads(
            &mut context,
            primitives_enabled,
            &mut primitive_draws,
            &mut primitive_chunks,
            &mut primitive_chunk_info,
        );
        let mut render_batch_plan = build_frame_batch_plan(
            context.frame_id,
            context.model_draws,
            context.projected_decals,
            &primitive_draws,
            &primitive_chunks,
            &primitive_chunk_info,
            context.camera3d_uniform,
            &self.model_manager,
        );
        let planned_model_draws = render_batch_plan.model_batches(context.model_draws);
        let planned_primitive_draws = render_batch_plan.primitive_draw_batches(&primitive_draws);
        let planned_primitive_chunks = render_batch_plan.primitive_chunk_batches(&primitive_chunks);
        let planned_water_draws = render_batch_plan.water_draw_batches(&primitive_draws);
        let planned_water_chunks = render_batch_plan.water_chunk_batches(&primitive_chunks);
        let planned_projected_decals = render_batch_plan.decal_batches(context.projected_decals);
        let projected_decals_active = !planned_projected_decals.is_empty();
        let post_depth_active = contact_ao_active || projected_decals_active;
        let depth_prepass_active = MAIN_SAMPLE_COUNT > 1 && post_depth_active;
        let (primitive_depth_draws, primitive_depth_chunks) = self.collect_frame_depth_workloads(
            depth_prepass_active,
            context.retained_scene_draws,
            context.camera3d_uniform,
            &planned_primitive_chunks,
        );
        let (primitive_shadow_draws, primitive_shadow_chunks) = self
            .collect_frame_shadow_workloads(
                primitive_shadows_enabled,
                context.retained_scene_draws,
                context.camera3d_uniform,
                &planned_primitive_chunks,
            );
        let material_group_count = material_group_count(&render_batch_plan);
        let water_pass_active = primitives_enabled
            && context.camera3d_uniform.is_some()
            && self
                .primitive_pipeline
                .has_water_surface(&planned_water_draws, &planned_water_chunks);
        let transparent_pass_active = primitives_enabled
            && context.camera3d_uniform.is_some()
            && self.primitive_pipeline.has_translucent_surface(
                &planned_primitive_draws,
                &planned_primitive_chunks,
                &self.model_manager,
                &self.texture_manager,
            );
        FrameWorkloadPlan {
            primitive_depth_draws,
            primitive_shadow_draws,
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
        }
    }

    fn collect_frame_scene_workloads(
        &mut self,
        context: &mut FrameWorkloadPlanContext<'_>,
        primitives_enabled: bool,
        primitive_draws: &mut Vec<PrimitiveDraw>,
        primitive_chunks: &mut Vec<PrimitiveChunkRef>,
        primitive_chunk_info: &mut Vec<PrimitiveChunkBatchInfo>,
    ) {
        let scene_update_start = if context.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        for scene_id in context.retained_scene_draws {
            self.render_world
                .collect_scene_draws(*scene_id, context.model_draws);
            if primitives_enabled {
                self.render_world
                    .collect_scene_primitive_draws_with_chunk_info(
                        *scene_id,
                        None,
                        primitive_draws,
                        primitive_chunks,
                        primitive_chunk_info,
                    );
            }
        }
        context.perf.scene_update_ms = elapsed_ms_opt(scene_update_start);
    }

    fn collect_frame_depth_workloads(
        &self,
        active: bool,
        scene_draws: &[u32],
        camera: Option<&Camera3DUniform>,
        planned_chunks: &[PrimitiveChunkRef],
    ) -> (Vec<PrimitiveDraw>, Vec<PrimitiveChunkRef>) {
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        if active {
            for scene_id in scene_draws {
                self.render_world.collect_scene_primitive_depth_draws(
                    *scene_id,
                    camera,
                    &mut draws,
                    &mut chunks,
                );
            }
            chunks.retain(|chunk| planned_chunks.contains(chunk));
        }
        (draws, chunks)
    }

    fn collect_frame_shadow_workloads(
        &self,
        active: bool,
        scene_draws: &[u32],
        camera: Option<&Camera3DUniform>,
        planned_chunks: &[PrimitiveChunkRef],
    ) -> (Vec<PrimitiveDraw>, Vec<PrimitiveChunkRef>) {
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        if active {
            for scene_id in scene_draws {
                self.render_world
                    .collect_scene_primitive_shadow_objects(*scene_id, camera, &mut draws);
                self.render_world
                    .collect_scene_primitive_shadow_chunks_from_candidates(
                        *scene_id,
                        camera,
                        planned_chunks,
                        &mut chunks,
                    );
            }
        }
        (draws, chunks)
    }
}

fn apply_frame_perf_overrides(context: &mut FrameWorkloadPlanContext<'_>) {
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_SHADOWS) {
        *context.shadow_enabled = false;
        *context.shadow_strength = 0.0;
        *context.shadow_quality = 0;
    }
    if context
        .perf_overrides
        .has(RENDERER_DIAG_DISABLE_POST_EFFECTS)
    {
        *context.post_bloom_strength = 0.0;
        *context.post_sharpen_strength = 0.0;
        *context.post_fxaa_strength = 0.0;
        *context.post_contact_ao_strength = 0.0;
        *context.post_contact_ao_quality = 0;
        context.projected_decals.clear();
        context.projected_decal_atlas_bindings.clear();
        return;
    }
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_BLOOM) {
        *context.post_bloom_strength = 0.0;
    }
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_SHARPEN) {
        *context.post_sharpen_strength = 0.0;
    }
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_FXAA) {
        *context.post_fxaa_strength = 0.0;
    }
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_CONTACT_AO) {
        *context.post_contact_ao_strength = 0.0;
        *context.post_contact_ao_quality = 0;
    }
    if context.perf_overrides.has(RENDERER_DIAG_DISABLE_DECALS) {
        context.projected_decals.clear();
        context.projected_decal_atlas_bindings.clear();
    }
}

fn build_frame_batch_plan(
    frame_id: u32,
    model_draws: &[ModelDraw],
    projected_decals: &[PostDecalGpu],
    primitive_draws: &[PrimitiveDraw],
    primitive_chunks: &[PrimitiveChunkRef],
    primitive_chunk_info: &[PrimitiveChunkBatchInfo],
    camera: Option<&Camera3DUniform>,
    model_manager: &ModelManager,
) -> RenderBatchPlan {
    let terrain_batch_inputs =
        RenderBatchPlanner::terrain_inputs(frame_id, model_draws, model_manager);
    let decal_batch_inputs = RenderBatchPlanner::decal_inputs(frame_id, projected_decals);
    RenderBatchPlanner::build(
        frame_id,
        0,
        model_draws,
        &terrain_batch_inputs,
        primitive_draws,
        primitive_chunks,
        primitive_chunk_info,
        &decal_batch_inputs,
        camera,
        RenderBatchQualityProfile::default(),
    )
}

fn material_group_count(render_batch_plan: &RenderBatchPlan) -> u32 {
    let mut material_groups = Vec::<u32>::new();
    for chunk in &render_batch_plan.visible_chunks {
        if !material_groups.contains(&chunk.material_group) {
            material_groups.push(chunk.material_group);
        }
    }
    saturating_u32(material_groups.len())
}
