use super::*;
use super::pass_dispatch::RenderPassResources;
use crate::pipeline3d::DecalSubmitter;

pub(super) struct PostPassExecutor;
pub(super) struct PostPassSetup;

pub(super) struct PostPassSetupContext<'a, 'r> {
    pub(super) resources: &'a mut RenderPassResources<'r>,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)>,
    pub(super) light_uniform: &'a LightUniform,
    pub(super) projected_decals: &'a [PostDecalGpu],
    pub(super) projected_decal_atlas_binding_count: u32,
    pub(super) bloom_threshold: f32,
    pub(super) bloom_strength: f32,
    pub(super) sharpen_strength: f32,
    pub(super) fxaa_strength: f32,
    pub(super) contact_ao_strength: f32,
    pub(super) contact_ao_radius: f32,
    pub(super) contact_ao_depth_scale: f32,
    pub(super) contact_ao_detail_strength: f32,
    pub(super) contact_ao_detail_radius: f32,
    pub(super) contact_ao_normal_bias: f32,
    pub(super) contact_ao_quality: u32,
}

pub(super) struct PostPassContext<'a, 'r> {
    pub(super) resources: &'a mut RenderPassResources<'r>,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) surface_view: &'a wgpu::TextureView,
    pub(super) projected_decal_atlas_bindings: &'a [ProjectedDecalAtlasBinding],
    pub(super) perf_enabled: bool,
}

impl PostPassSetup {
    pub(super) fn upload_uniforms(ctx: &mut PostPassSetupContext<'_, '_>) {
        let mut post_uniform = PostUniform::from_settings(
            ctx.resources.surface_config.width,
            ctx.resources.surface_config.height,
            ctx.bloom_threshold,
            ctx.bloom_strength,
            ctx.sharpen_strength,
            ctx.fxaa_strength,
            ctx.contact_ao_strength,
            ctx.contact_ao_radius,
            ctx.contact_ao_depth_scale,
            ctx.contact_ao_detail_strength,
            ctx.contact_ao_detail_radius,
            ctx.contact_ao_normal_bias,
            ctx.contact_ao_quality,
        );
        let mut post_decal_light_vectors = [[0.0f32; 4]; 3];
        let mut post_decal_light_colors = [[0.0f32; 4]; 3];
        let mut post_decal_light_count = 0usize;
        for light in
            ctx.light_uniform.lights.iter().take(
                ctx.light_uniform.count[0].min(ctx.light_uniform.lights.len() as u32) as usize,
            )
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
        ctx.resources.queue.write_buffer(
            &ctx.resources.post_uniform_buffer,
            0,
            bytemuck::bytes_of(&post_uniform),
        );
        let post_inv_view_proj = ctx
            .camera3d_uniform
            .and_then(|camera| math3d::mat4_inverse(&camera.view_proj))
            .unwrap_or(math3d::MAT4_IDENTITY);
        let post_camera_pos = ctx
            .camera3d_state
            .map(|(eye, _, _, _, _, _)| eye.to_array())
            .unwrap_or([0.0, 0.0, 0.0]);
        let projected_decal_plan = DecalSubmitter::prepare(ctx.projected_decals);
        let _decal_submit_report = (
            projected_decal_plan.owner,
            projected_decal_plan.report,
        );
        let projected_decal_count = projected_decal_plan.prepared_count;
        let projected_decal_atlas_binding_count = ctx
            .projected_decal_atlas_binding_count
            .min(projected_decal_count as u32);
        ctx.resources.queue.write_buffer(
            &ctx.resources.post_decal_uniform_buffer,
            0,
            bytemuck::bytes_of(&PostDecalUniform::from_decals(
                post_inv_view_proj,
                post_camera_pos,
                ctx.projected_decals,
                projected_decal_atlas_binding_count,
            )),
        );
    }
}

impl PostPassExecutor {
    pub(super) fn execute(ctx: &mut PostPassContext<'_, '_>) -> Result<f64, String> {
        let post_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let post_color_view = ctx
            .resources
            .targets
            .post_color_view()
            .ok_or_else(|| "voplay: missing post color target".to_string())?;
        let receiver_mask_view = ctx
            .resources
            .targets
            .receiver_mask_view()
            .ok_or_else(|| "voplay: missing receiver mask target".to_string())?;
        let surface_props_view = ctx
            .resources
            .targets
            .surface_props_view()
            .ok_or_else(|| "voplay: missing surface props target".to_string())?;
        let dynamic_post_bind_group;
        let post_bind_group = if ctx.projected_decal_atlas_bindings.is_empty() {
            ctx.resources
                .post_bind_group
                .as_ref()
                .ok_or_else(|| "voplay: missing post bind group".to_string())?
        } else {
            let fallback_decal_atlas = ctx.resources.pipeline_post.decal_fallback_view();
            let fallback_decal_normal_atlas =
                ctx.resources.pipeline_post.decal_normal_fallback_view();
            let fallback_decal_roughness_atlas =
                ctx.resources.pipeline_post.decal_roughness_fallback_view();
            let fallback_decal_mask_atlas = ctx.resources.pipeline_post.decal_mask_fallback_view();
            let mut decal_atlas_views = [fallback_decal_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_normal_atlas_views =
                [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_roughness_atlas_views =
                [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_mask_atlas_views = [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES];
            for (slot, binding) in ctx.projected_decal_atlas_bindings.iter().enumerate() {
                if let Some(texture) = ctx.resources.texture_manager.get(binding.albedo_id) {
                    decal_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.resources.texture_manager.get(binding.normal_id) {
                    decal_normal_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.resources.texture_manager.get(binding.roughness_id) {
                    decal_roughness_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.resources.texture_manager.get(binding.mask_id) {
                    decal_mask_atlas_views[slot] = &texture.view;
                }
            }
            let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
                ctx.resources.pipeline_depth.depth_texture_view()
            } else {
                ctx.resources
                    .targets
                    .depth_view()
                    .ok_or_else(|| "voplay: missing depth target".to_string())?
            };
            dynamic_post_bind_group = ctx.resources.pipeline_post.create_bind_group(
                &ctx.resources.device,
                post_color_view,
                post_depth_view,
                &ctx.resources.post_uniform_buffer,
                &ctx.resources.post_decal_uniform_buffer,
                decal_atlas_views,
                decal_normal_atlas_views,
                decal_roughness_atlas_views,
                decal_mask_atlas_views,
                receiver_mask_view,
                surface_props_view,
            );
            &dynamic_post_bind_group
        };
        let mut post_pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.surface_view,
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
        ctx.resources
            .pipeline_post
            .draw(&mut post_pass, post_bind_group);
        Ok(elapsed_ms_opt(post_start))
    }

    pub(super) fn workload() -> RenderPassWorkload {
        RenderPassWorkload {
            draw_calls: 1,
            batches: 1,
            instances: 1,
            triangles: 2,
            upload_bytes: 0,
        }
    }
}
