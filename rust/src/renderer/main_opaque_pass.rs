use super::*;
use crate::primitive_pipeline::PrimitiveRenderFilter;

pub(super) struct MainOpaquePassExecutor;

pub(super) struct MainOpaquePassContext<'a> {
    pub(super) renderer: &'a mut Renderer,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) clear_color: wgpu::Color,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)>,
    pub(super) skybox_cubemap_id: Option<u32>,
    pub(super) light_uniform: &'a LightUniform,
    pub(super) model_draws: &'a [ModelDraw],
    pub(super) primitive_draws: &'a [PrimitiveDraw],
    pub(super) primitive_chunks: &'a [PrimitiveChunkRef],
    pub(super) main_aux_targets_enabled: bool,
    pub(super) aspect: f32,
    pub(super) perf_enabled: bool,
    pub(super) perf: &'a mut RendererPerfStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MainOpaquePassResult {
    pub(super) elapsed_ms: f64,
    pub(super) primitive_stats: PrimitiveDrawStats,
}

impl MainOpaquePassExecutor {
    pub(super) fn execute(
        ctx: &mut MainOpaquePassContext<'_>,
    ) -> Result<MainOpaquePassResult, String> {
        let main_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        let main_setup_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
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
                    load: wgpu::LoadOp::Clear(ctx.clear_color),
                    store: color_store,
                },
            }),
            main_receiver_mask_view.map(|view| wgpu::RenderPassColorAttachment {
                view,
                resolve_target: receiver_mask_resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: aux_store,
                },
            }),
            main_surface_props_view.map(|view| wgpu::RenderPassColorAttachment {
                view,
                resolve_target: surface_props_resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: aux_store,
                },
            }),
        ];
        let mut render_pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voplay_main"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: ctx.renderer.resources.depth_view().map(|dv| {
                wgpu::RenderPassDepthStencilAttachment {
                    view: dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        ctx.perf.main_pass_setup_ms = elapsed_ms_opt(main_setup_start);

        if let (Some(cubemap_id), Some((eye, target, up, fov, near, far))) =
            (ctx.skybox_cubemap_id, ctx.camera3d_state)
        {
            if let Some(cubemap) = ctx.renderer.texture_manager.get_cubemap(cubemap_id) {
                let skybox_start = if ctx.perf_enabled {
                    Some(perf_now())
                } else {
                    None
                };
                let view_rot = math3d::view_rotation_only(eye, target, up);
                let proj = math3d::perspective_rh_zo(fov.to_radians(), ctx.aspect, near, far);
                let vp = math3d::mat4_mul(&proj, &view_rot);
                let inv_vp = math3d::mat4_inverse(&vp).unwrap_or(math3d::MAT4_IDENTITY);
                ctx.renderer
                    .pipeline_skybox
                    .set_camera(&ctx.renderer.queue, &inv_vp);
                ctx.renderer.pipeline_skybox.draw(
                    &mut render_pass,
                    cubemap,
                    ctx.main_aux_targets_enabled,
                );
                ctx.perf.main_skybox_ms += elapsed_ms_opt(skybox_start);
            }
        }

        if !ctx.model_draws.is_empty() {
            if let Some(cam3d) = ctx.camera3d_uniform {
                let model_start = if ctx.perf_enabled {
                    Some(perf_now())
                } else {
                    None
                };
                ctx.renderer.pipeline3d.set_camera_and_lights(
                    &ctx.renderer.queue,
                    cam3d,
                    ctx.light_uniform,
                );
                let shadow_view = ctx.renderer.pipeline_shadow.shadow_texture_view();
                ctx.renderer.pipeline3d.draw_models(
                    &ctx.renderer.device,
                    &ctx.renderer.queue,
                    &mut render_pass,
                    ctx.model_draws,
                    &ctx.renderer.model_manager,
                    &ctx.renderer.texture_manager,
                    shadow_view,
                    ctx.main_aux_targets_enabled,
                );
                ctx.perf.main_model_ms += elapsed_ms_opt(model_start);
            }
        }

        let mut primitive_stats = PrimitiveDrawStats::default();
        if !ctx.primitive_draws.is_empty() || !ctx.primitive_chunks.is_empty() {
            if let Some(cam3d) = ctx.camera3d_uniform {
                let primitive_start = if ctx.perf_enabled {
                    Some(perf_now())
                } else {
                    None
                };
                ctx.renderer.primitive_pipeline.set_camera_and_lights(
                    &ctx.renderer.queue,
                    cam3d,
                    ctx.light_uniform,
                );
                let shadow_view = ctx.renderer.pipeline_shadow.shadow_texture_view();
                primitive_stats = ctx.renderer.primitive_pipeline.draw(
                    &ctx.renderer.device,
                    &ctx.renderer.queue,
                    &mut render_pass,
                    ctx.primitive_draws,
                    ctx.primitive_chunks,
                    &ctx.renderer.model_manager,
                    &ctx.renderer.texture_manager,
                    shadow_view,
                    ctx.main_aux_targets_enabled,
                    PrimitiveRenderFilter::Main,
                );
                ctx.perf.main_primitive_ms += elapsed_ms_opt(primitive_start);
            }
        }
        let main_close_start = if ctx.perf_enabled {
            Some(perf_now())
        } else {
            None
        };
        drop(render_pass);
        ctx.perf.main_pass_close_ms = elapsed_ms_opt(main_close_start);
        Ok(MainOpaquePassResult {
            elapsed_ms: elapsed_ms_opt(main_start),
            primitive_stats,
        })
    }

    pub(super) fn workload(
        model_draw_count: usize,
        primitive_draw_count: usize,
        primitive_chunk_count: usize,
    ) -> RenderPassWorkload {
        RenderPassWorkload {
            draw_calls: saturating_u32(model_draw_count + primitive_draw_count),
            batches: saturating_u32(model_draw_count + primitive_chunk_count),
            instances: saturating_u32(model_draw_count + primitive_draw_count),
            triangles: 0,
            upload_bytes: 0,
        }
    }
}
