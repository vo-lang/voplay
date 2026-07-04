use super::*;
use crate::primitive_pipeline::PrimitiveRenderFilter;

pub(super) struct WaterPassExecutor;

pub(super) struct WaterPassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) light_uniform: &'a LightUniform,
    pub(super) primitive_draws: &'a [PrimitiveDraw],
    pub(super) primitive_chunks: &'a [PrimitiveChunkRef],
    pub(super) main_aux_targets_enabled: bool,
    pub(super) perf_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WaterPassResult {
    pub(super) elapsed_ms: f64,
    pub(super) stats: PrimitiveDrawStats,
}

impl WaterPassExecutor {
    pub(super) fn execute(ctx: &mut WaterPassContext<'_>) -> Result<WaterPassResult, String> {
        let water_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let Some(cam3d) = ctx.camera3d_uniform else {
            return Ok(WaterPassResult::default());
        };
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
        let receiver_mask_view = if ctx.main_aux_targets_enabled {
            Some(
                ctx.renderer
                    .resources
                    .receiver_mask_view()
                    .ok_or_else(|| "voplay: missing receiver mask target".to_string())?,
            )
        } else {
            None
        };
        let surface_props_view = if ctx.main_aux_targets_enabled {
            Some(
                ctx.renderer
                    .resources
                    .surface_props_view()
                    .ok_or_else(|| "voplay: missing surface props target".to_string())?,
            )
        } else {
            None
        };
        let main_receiver_mask_view = if ctx.main_aux_targets_enabled {
            Some(if MAIN_SAMPLE_COUNT > 1 {
                ctx.renderer
                    .resources
                    .msaa_receiver_mask_view()
                    .ok_or_else(|| "voplay: missing MSAA receiver mask target".to_string())?
            } else {
                receiver_mask_view.expect("receiver mask view present")
            })
        } else {
            None
        };
        let main_surface_props_view = if ctx.main_aux_targets_enabled {
            Some(if MAIN_SAMPLE_COUNT > 1 {
                ctx.renderer
                    .resources
                    .msaa_surface_props_view()
                    .ok_or_else(|| "voplay: missing MSAA surface props target".to_string())?
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
        let receiver_mask_resolve_target = if ctx.main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1
        {
            receiver_mask_view
        } else {
            None
        };
        let surface_props_resolve_target = if ctx.main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1
        {
            surface_props_view
        } else {
            None
        };
        let color_store = if MAIN_SAMPLE_COUNT > 1 {
            wgpu::StoreOp::Discard
        } else {
            wgpu::StoreOp::Store
        };
        let aux_store = if MAIN_SAMPLE_COUNT > 1 {
            wgpu::StoreOp::Discard
        } else {
            wgpu::StoreOp::Store
        };
        let color_attachments = [
            Some(wgpu::RenderPassColorAttachment {
                view: main_color_view,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: color_store,
                },
            }),
            main_receiver_mask_view.map(|view| wgpu::RenderPassColorAttachment {
                view,
                resolve_target: receiver_mask_resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: aux_store,
                },
            }),
            main_surface_props_view.map(|view| wgpu::RenderPassColorAttachment {
                view,
                resolve_target: surface_props_resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: aux_store,
                },
            }),
        ];
        let mut render_pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_water"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: ctx.renderer.resources.depth_view().map(|dv| {
                wgpu::RenderPassDepthStencilAttachment {
                    view: dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
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
        let stats = ctx.renderer.primitive_pipeline.draw(
            &ctx.renderer.device,
            &ctx.renderer.queue,
            &mut render_pass,
            ctx.primitive_draws,
            ctx.primitive_chunks,
            &ctx.renderer.model_manager,
            &ctx.renderer.texture_manager,
            shadow_view,
            ctx.main_aux_targets_enabled,
            PrimitiveRenderFilter::Water,
        );
        drop(render_pass);
        Ok(WaterPassResult {
            elapsed_ms: elapsed_ms_opt(water_start),
            stats,
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
