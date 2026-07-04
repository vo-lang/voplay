use super::*;

pub(super) struct PostPassExecutor;

pub(super) struct PostPassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) surface_view: &'a wgpu::TextureView,
    pub(super) projected_decal_atlas_bindings: &'a [ProjectedDecalAtlasBinding],
    pub(super) perf_enabled: bool,
}

impl PostPassExecutor {
    pub(super) fn execute(ctx: &mut PostPassContext<'_>) -> Result<f64, String> {
        let post_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let post_color_view = ctx
            .renderer
            .resources
            .post_color_view()
            .ok_or_else(|| "voplay: missing post color target".to_string())?;
        let receiver_mask_view = ctx
            .renderer
            .resources
            .receiver_mask_view()
            .ok_or_else(|| "voplay: missing receiver mask target".to_string())?;
        let surface_props_view = ctx
            .renderer
            .resources
            .surface_props_view()
            .ok_or_else(|| "voplay: missing surface props target".to_string())?;
        let dynamic_post_bind_group;
        let post_bind_group = if ctx.projected_decal_atlas_bindings.is_empty() {
            ctx.renderer
                .post_bind_group
                .as_ref()
                .ok_or_else(|| "voplay: missing post bind group".to_string())?
        } else {
            let fallback_decal_atlas = ctx.renderer.pipeline_post.decal_fallback_view();
            let fallback_decal_normal_atlas =
                ctx.renderer.pipeline_post.decal_normal_fallback_view();
            let fallback_decal_roughness_atlas =
                ctx.renderer.pipeline_post.decal_roughness_fallback_view();
            let fallback_decal_mask_atlas = ctx.renderer.pipeline_post.decal_mask_fallback_view();
            let mut decal_atlas_views = [fallback_decal_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_normal_atlas_views =
                [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_roughness_atlas_views =
                [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES];
            let mut decal_mask_atlas_views = [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES];
            for (slot, binding) in ctx.projected_decal_atlas_bindings.iter().enumerate() {
                if let Some(texture) = ctx.renderer.texture_manager.get(binding.albedo_id) {
                    decal_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.renderer.texture_manager.get(binding.normal_id) {
                    decal_normal_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.renderer.texture_manager.get(binding.roughness_id) {
                    decal_roughness_atlas_views[slot] = &texture.view;
                }
                if let Some(texture) = ctx.renderer.texture_manager.get(binding.mask_id) {
                    decal_mask_atlas_views[slot] = &texture.view;
                }
            }
            let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
                ctx.renderer.pipeline_depth.depth_texture_view()
            } else {
                ctx.renderer
                    .resources
                    .depth_view()
                    .ok_or_else(|| "voplay: missing depth target".to_string())?
            };
            dynamic_post_bind_group = ctx.renderer.pipeline_post.create_bind_group(
                &ctx.renderer.device,
                post_color_view,
                post_depth_view,
                &ctx.renderer.post_uniform_buffer,
                &ctx.renderer.post_decal_uniform_buffer,
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
        ctx.renderer
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
