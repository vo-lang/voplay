use super::*;
use super::pass_dispatch::RenderPassResources;
use crate::pipeline3d::PrimitiveSubmitter;
use std::cmp::Ordering;

pub(super) struct MainTransparentPassExecutor;

#[derive(Clone, Copy)]
pub(super) struct RenderDrawItem {
    pass: RenderPassKind,
    depth: f32,
    stable_index: usize,
    primitive: PrimitiveDraw,
}

impl RenderDrawItem {
    fn translucent_primitive(
        camera_pos: Vec3,
        stable_index: usize,
        primitive: PrimitiveDraw,
    ) -> Self {
        let position = Vec3::new(
            primitive.model_uniform.model[3][0],
            primitive.model_uniform.model[3][1],
            primitive.model_uniform.model[3][2],
        );
        let delta = position - camera_pos;
        Self {
            pass: RenderPassKind::MainTransparent,
            depth: delta.dot(delta),
            stable_index,
            primitive,
        }
    }
}

fn sorted_transparent_draw_items(
    camera: &Camera3DUniform,
    draws: &[PrimitiveDraw],
) -> Vec<RenderDrawItem> {
    let camera_pos = Vec3::new(
        camera.camera_pos[0],
        camera.camera_pos[1],
        camera.camera_pos[2],
    );
    let mut items = draws
        .iter()
        .copied()
        .enumerate()
        .map(|(index, primitive)| {
            RenderDrawItem::translucent_primitive(camera_pos, index, primitive)
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        b.depth
            .partial_cmp(&a.depth)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.stable_index.cmp(&b.stable_index))
    });
    items
}

fn accumulate_stats(total: &mut PrimitiveDrawStats, next: PrimitiveDrawStats) {
    total.batch_count = total.batch_count.saturating_add(next.batch_count);
    total.instance_count = total.instance_count.saturating_add(next.instance_count);
    total.triangle_count = total.triangle_count.saturating_add(next.triangle_count);
}

pub(super) struct MainTransparentPassContext<'a, 'r> {
    pub(super) resources: &'a mut RenderPassResources<'r>,
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
        ctx: &mut MainTransparentPassContext<'_, '_>,
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
            .resources
            .targets
            .post_color_view()
            .ok_or_else(|| "voplay: missing post color target".to_string())?;
        let main_color_view = if MAIN_SAMPLE_COUNT > 1 {
            ctx.resources
                .targets
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
            depth_stencil_attachment: ctx.resources.targets.depth_view().map(|depth_view| {
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
        ctx.resources.primitive_pipeline.set_camera_and_lights(
            &ctx.resources.queue,
            cam3d,
            ctx.light_uniform,
        );
        let shadow_view = ctx.resources.pipeline_shadow.shadow_texture_view();
        let sorted_items = sorted_transparent_draw_items(cam3d, ctx.primitive_draws);
        let sorted_primitive_draws = sorted_items
            .iter()
            .filter(|item| item.pass == RenderPassKind::MainTransparent)
            .map(|item| item.primitive)
            .collect::<Vec<_>>();
        let mut primitive_stats = PrimitiveDrawStats::default();
        if !sorted_primitive_draws.is_empty() || !ctx.primitive_chunks.is_empty() {
            let primitive_submit_plan = PrimitiveSubmitter::draw(
                crate::primitive_pipeline::PrimitiveRenderFilter::Translucent,
            );
            let _primitive_submit_report = (
                primitive_submit_plan.owner,
                primitive_submit_plan.report,
            );
            let stats = ctx.resources.primitive_pipeline.draw(
                &ctx.resources.device,
                &ctx.resources.queue,
                &mut render_pass,
                &sorted_primitive_draws,
                ctx.primitive_chunks,
                &ctx.resources.model_manager,
                &ctx.resources.texture_manager,
                shadow_view,
                false,
                primitive_submit_plan.filter,
            );
            accumulate_stats(&mut primitive_stats, stats);
        }
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
