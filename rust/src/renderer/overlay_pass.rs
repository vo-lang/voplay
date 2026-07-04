use super::*;
use crate::draw_list::Frame2D;

pub(super) struct OverlayPassExecutor;

pub(super) struct OverlayPassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) surface_view: &'a wgpu::TextureView,
    pub(super) frame: &'a Frame2D,
    pub(super) camera_alignment: u32,
    pub(super) perf_enabled: bool,
}

impl OverlayPassExecutor {
    pub(super) fn execute(ctx: &mut OverlayPassContext<'_>) -> Result<f64, String> {
        let overlay_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let mut overlay_pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_overlay_2d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for dc in &ctx.frame.draw_calls {
            let cam_offset = dc.camera_idx as u32 * ctx.camera_alignment;
            match &dc.kind {
                DrawCallKind::Shapes { start, count } => {
                    ctx.renderer.pipeline2d.draw_range(
                        &mut overlay_pass,
                        &ctx.renderer.camera_bind_group,
                        &[cam_offset],
                        *start,
                        *count,
                    );
                }
                DrawCallKind::Sprites {
                    texture_id,
                    start,
                    count,
                } => {
                    if let Some(tex) = ctx.renderer.texture_manager.get(*texture_id) {
                        ctx.renderer.pipeline_sprite.draw_range(
                            &mut overlay_pass,
                            &ctx.renderer.camera_bind_group,
                            &[cam_offset],
                            &tex.bind_group,
                            *start,
                            *count,
                        );
                    }
                }
            }
        }
        Ok(elapsed_ms_opt(overlay_start))
    }

    pub(super) fn workload(frame: &Frame2D) -> RenderPassWorkload {
        RenderPassWorkload {
            draw_calls: saturating_u32(frame.draw_calls.len()),
            batches: saturating_u32(frame.draw_calls.len()),
            instances: saturating_u32(frame.shapes.len() + frame.sprites.len()),
            triangles: 0,
            upload_bytes: 0,
        }
    }
}
