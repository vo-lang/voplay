use super::pass_dispatch::RenderPassResources;
use super::*;

pub(super) struct DepthPassExecutor;

pub(super) struct DepthPassContext<'a, 'r> {
    pub(super) resources: &'a mut RenderPassResources<'r>,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) model_draws: &'a [ModelDraw],
    pub(super) primitive_depth_draws: &'a mut Vec<PrimitiveDraw>,
    pub(super) primitive_depth_chunks: &'a [PrimitiveChunkRef],
    pub(super) perf_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct DepthPassResult {
    pub(super) elapsed_ms: f64,
    pub(super) primitive_draw_calls: u32,
}

impl DepthPassExecutor {
    pub(super) fn execute(ctx: &mut DepthPassContext<'_, '_>) -> Result<DepthPassResult, String> {
        let depth_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let empty_model_draws: &[ModelDraw] = &[];
        let empty_primitive_draws: &[PrimitiveDraw] = &[];
        let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
        if !ctx.primitive_depth_chunks.is_empty() {
            ctx.resources
                .pipelines
                .primitive
                .append_resident_depth_draws(ctx.primitive_depth_chunks, ctx.primitive_depth_draws);
        }
        let (depth_model_draws, depth_primitive_draws, depth_view_proj) =
            if let Some(cam3d) = ctx.camera3d_uniform {
                (
                    ctx.model_draws,
                    &ctx.primitive_depth_draws[..],
                    cam3d.view_proj,
                )
            } else {
                (
                    empty_model_draws,
                    empty_primitive_draws,
                    math3d::MAT4_IDENTITY,
                )
            };
        ctx.resources.pipelines.depth.render_depth_pass(
            &ctx.resources.gpu.gpu_device,
            ctx.encoder,
            &ctx.resources.gpu.gpu_queue,
            &depth_view_proj,
            depth_model_draws,
            depth_primitive_draws,
            empty_primitive_chunks,
            &ctx.resources.pipelines.primitive,
            &ctx.resources.assets.models,
        );
        Ok(DepthPassResult {
            elapsed_ms: elapsed_ms_opt(depth_start),
            primitive_draw_calls: ctx.resources.pipelines.depth.last_primitive_batch_count(),
        })
    }

    pub(super) fn workload() -> RenderPassWorkload {
        RenderPassWorkload::default()
    }
}
