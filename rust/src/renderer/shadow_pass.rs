use super::*;
use super::pass_dispatch::RenderPassResources;

pub(super) struct ShadowPassExecutor;

pub(super) struct ShadowPassContext<'a, 'r> {
    pub(super) resources: &'a mut RenderPassResources<'r>,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)>,
    pub(super) light_uniform: &'a mut LightUniform,
    pub(super) model_draws: &'a [ModelDraw],
    pub(super) primitive_shadow_draws: &'a mut Vec<PrimitiveDraw>,
    pub(super) primitive_shadow_chunks: &'a [PrimitiveChunkRef],
    pub(super) retained_scene_draws: &'a [u32],
    pub(super) shadow_resolution: u32,
    pub(super) shadow_quality: u32,
    pub(super) shadow_distance: f32,
    pub(super) shadow_fade: f32,
    pub(super) shadow_softness: f32,
    pub(super) shadow_strength: f32,
    pub(super) aspect: f32,
    pub(super) perf_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ShadowPassResult {
    pub(super) elapsed_ms: f64,
    pub(super) active: bool,
    pub(super) primitive_draw_calls: u32,
}

impl ShadowPassExecutor {
    pub(super) fn execute(ctx: &mut ShadowPassContext<'_, '_>) -> Result<ShadowPassResult, String> {
        let shadow_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let mut active = false;
        if let Some(cam3d) = ctx.camera3d_uniform {
            if ctx.light_uniform.count[0] > 0
                && ctx.light_uniform.lights[0].position_or_dir[3] == 0.0
            {
                let shadow_to_light = Vec3::new(
                    ctx.light_uniform.lights[0].position_or_dir[0],
                    ctx.light_uniform.lights[0].position_or_dir[1],
                    ctx.light_uniform.lights[0].position_or_dir[2],
                );
                let shadow_dir = (-shadow_to_light).normalize();
                if shadow_dir.length() > 0.0 {
                    let mut cascade_count = shadow_cascade_count_for_quality(ctx.shadow_quality);
                    if ctx.camera3d_state.is_none() {
                        cascade_count = 1;
                    }
                    let shadow_atlas_size =
                        shadow_atlas_resolution(ctx.shadow_resolution, cascade_count);
                    let tile_resolution = if cascade_count > 1 {
                        (shadow_atlas_size / 2).max(1)
                    } else {
                        shadow_atlas_size
                    };
                    if ctx.resources.pipeline_shadow.size() != shadow_atlas_size {
                        ctx.resources.clear_texture_bind_group_caches();
                        ctx.resources
                            .pipeline_shadow
                            .resize(&ctx.resources.device, shadow_atlas_size);
                    }
                    let mut shadow_cascade_vps = [math3d::MAT4_IDENTITY; 4];
                    let mut shadow_cascade_splits = [0.0; 4];
                    let shadow_vp = if let Some((eye, target, up, fov, near, camera_far)) =
                        ctx.camera3d_state
                    {
                        let shadow_far = if ctx.shadow_distance > 0.0 {
                            ctx.shadow_distance.min(camera_far).max(near + 0.1)
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
                                        ctx.aspect,
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
                                ctx.aspect,
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
                        if !ctx.primitive_shadow_draws.is_empty()
                            || !ctx.primitive_shadow_chunks.is_empty()
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
                                for scene_id in ctx.retained_scene_draws {
                                    ctx.resources
                                        .render_world
                                        .collect_scene_primitive_shadow_objects_for_light_view(
                                            *scene_id,
                                            ctx.camera3d_uniform,
                                            &light_camera,
                                            &mut cascade_shadow_draws,
                                        );
                                    ctx.resources
                                        .render_world
                                        .collect_scene_primitive_shadow_chunks_for_light_view(
                                            *scene_id,
                                            ctx.camera3d_uniform,
                                            &light_camera,
                                            ctx.primitive_shadow_chunks,
                                            &mut cascade_shadow_chunks,
                                        );
                                }
                                if !cascade_shadow_chunks.is_empty() {
                                    ctx.resources
                                        .primitive_pipeline
                                        .append_resident_shadow_draws(
                                            &cascade_shadow_chunks,
                                            &mut cascade_shadow_draws,
                                        );
                                }
                                cascade_primitive_shadow_draws.push(cascade_shadow_draws);
                                cascade_primitive_shadow_chunks.push(Vec::new());
                            }
                        }
                        let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                        ctx.resources.pipeline_shadow.render_shadow_cascade_pass(
                            &ctx.resources.device,
                            ctx.encoder,
                            &ctx.resources.queue,
                            &shadow_cascade_vps[..cascade_count],
                            ctx.model_draws,
                            ctx.primitive_shadow_draws,
                            &cascade_primitive_shadow_draws,
                            empty_primitive_chunks,
                            &cascade_primitive_shadow_chunks,
                            &ctx.resources.primitive_pipeline,
                            &ctx.resources.model_manager,
                        );
                    } else {
                        let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                        if !ctx.primitive_shadow_chunks.is_empty() {
                            ctx.resources
                                .primitive_pipeline
                                .append_resident_shadow_draws(
                                    ctx.primitive_shadow_chunks,
                                    ctx.primitive_shadow_draws,
                                );
                        }
                        ctx.resources.pipeline_shadow.render_shadow_pass(
                            &ctx.resources.device,
                            ctx.encoder,
                            &ctx.resources.queue,
                            &shadow_vp,
                            ctx.model_draws,
                            ctx.primitive_shadow_draws,
                            empty_primitive_chunks,
                            &ctx.resources.primitive_pipeline,
                            &ctx.resources.model_manager,
                        );
                    }
                    ctx.light_uniform.shadow_vp = shadow_vp;
                    ctx.light_uniform.shadow_cascade_vp = shadow_cascade_vps;
                    ctx.light_uniform.shadow_cascade_splits = shadow_cascade_splits;
                    ctx.light_uniform.shadow_params =
                        [1.0, 0.002, ctx.shadow_softness, ctx.shadow_strength];
                    ctx.light_uniform.shadow_params2 = [
                        ctx.shadow_distance,
                        ctx.shadow_fade,
                        ctx.shadow_quality as f32,
                        cascade_count as f32,
                    ];
                    ctx.light_uniform.count[2] = 0;
                    active = true;
                }
            }
        }
        Ok(ShadowPassResult {
            elapsed_ms: elapsed_ms_opt(shadow_start),
            active,
            primitive_draw_calls: ctx.resources.pipeline_shadow.last_primitive_batch_count(),
        })
    }

    pub(super) fn workload() -> RenderPassWorkload {
        RenderPassWorkload::default()
    }
}
