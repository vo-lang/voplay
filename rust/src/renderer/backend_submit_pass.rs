use super::*;

pub(super) struct BackendSubmitPassExecutor;

pub(super) struct BackendSubmitPassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: Option<wgpu::CommandEncoder>,
    pub(super) output: Option<wgpu::SurfaceTexture>,
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
}

impl BackendSubmitPassExecutor {
    pub(super) fn execute(ctx: &mut BackendSubmitPassContext<'_>) -> Result<f64, String> {
        let queue_submit_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let encoder = ctx
            .encoder
            .take()
            .ok_or_else(|| "voplay: backend submit pass missing command encoder".to_string())?;
        ctx.renderer.queue.submit(std::iter::once(encoder.finish()));
        ctx.perf.queue_submit_cpu_ms = elapsed_ms_opt(queue_submit_start);

        let present_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let output = ctx
            .output
            .take()
            .ok_or_else(|| "voplay: backend submit pass missing surface texture".to_string())?;
        output.present();
        ctx.perf.present_cpu_ms = elapsed_ms_opt(present_start);
        Ok(ctx.perf.queue_submit_cpu_ms + ctx.perf.present_cpu_ms)
    }

    pub(super) fn workload() -> RenderPassWorkload {
        RenderPassWorkload::default()
    }
}
