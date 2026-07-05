use super::*;
use crate::pipeline3d::PrimitiveSubmitter;

pub(super) struct MainTransparentPassExecutor;

pub(super) struct MainTransparentPassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) light_uniform: &'a LightUniform,
    pub(super) primitive_draws: &'a [PrimitiveDraw],
    pub(super) primitive_chunks: &'a [PrimitiveChunkRef],
    pub(super) perf_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MainTransparentPassResult {
    pub(super) elapsed_ms: f64,
    pub(super) primitive_stats: PrimitiveDrawStats,
}

impl MainTransparentPassExecutor {
    /// Executes sorted Translucent primitive batches after opaque color is ready.
    /// The selected primitive pipelines use depth_write_enabled: false.
    pub(super) fn execute(
        ctx: &mut MainTransparentPassContext<'_>,
    ) -> Result<MainTransparentPassResult, String> {
        let transparent_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let Some(cam3d) = ctx.camera3d_uniform else {
            return Ok(MainTransparentPassResult::default());
        };
        if ctx.primitive_draws.is_empty() && ctx.primitive_chunks.is_empty() {
            return Ok(MainTransparentPassResult::default());
        }
        let post_color_view = ctx
            .renderer
            .resources
            .post_color_view()
            .ok_or_else(|| "voplay: missing post color target".to_string())?;
        let main_color_view = if MAIN_SAMPLE_COUNT > 1 {
            ctx.renderer
                .resources
                .msaa_color_view()
                .ok_or_else(|| "voplay: missing MSAA color target".to_string())?
        } else {
            post_color_view
        };
        let resolve_target = if MAIN_SAMPLE_COUNT > 1 {
            Some(post_color_view)
        } else {
            None
        };
        let color_store = if MAIN_SAMPLE_COUNT > 1 {
            wgpu::StoreOp::Discard
        } else {
            wgpu::StoreOp::Store
        };
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: main_color_view,
            resolve_target,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: color_store,
            },
        })];
        let mut render_pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_main_transparent"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: ctx.renderer.resources.depth_view().map(|depth_view| {
                wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        ctx.renderer.primitive_pipeline.set_camera_and_lights(
            &ctx.renderer.queue,
            cam3d,
            ctx.light_uniform,
        );
        let shadow_view = ctx.renderer.pipeline_shadow.shadow_texture_view();
        let primitive_stats = ctx.renderer.primitive_pipeline.draw(
            &ctx.renderer.device,
            &ctx.renderer.queue,
            &mut render_pass,
            ctx.primitive_draws,
            ctx.primitive_chunks,
            &ctx.renderer.model_manager,
            &ctx.renderer.texture_manager,
            shadow_view,
            false,
            PrimitiveSubmitter::draw(crate::primitive_pipeline::PrimitiveRenderFilter::Translucent),
        );
        drop(render_pass);
        Ok(MainTransparentPassResult {
            elapsed_ms: elapsed_ms_opt(transparent_start),
            primitive_stats,
        })
    }

    pub(super) fn workload(stats: PrimitiveDrawStats) -> RenderPassWorkload {
        RenderPassWorkload {
            draw_calls: stats.batch_count,
            batches: stats.batch_count,
            instances: stats.instance_count,
            triangles: stats.triangle_count,
            upload_bytes: 0,
        }
    }
}
