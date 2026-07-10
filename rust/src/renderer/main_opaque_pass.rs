use super::*;

pub(super) struct MainOpaquePassExecutor;

pub(super) struct MainOpaquePassContext<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) targets: &'a RenderResourceRegistry,
    pub(super) mesh_pipeline: &'a mut Pipeline3D,
    pub(super) primitive_pipeline: &'a mut PrimitivePipeline,
    pub(super) shadow_pipeline: &'a PipelineShadow,
    pub(super) skybox_pipeline: &'a mut PipelineSkybox,
    pub(super) models: &'a ModelManager,
    pub(super) textures: &'a TextureManager,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) clear_color: wgpu::Color,
    pub(super) camera3d_uniform: Option<&'a Camera3DUniform>,
    pub(super) camera3d_state: Option<Camera3DState>,
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
    pub(super) mesh_stats: MeshDrawStats,
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
            .targets
            .post_color_view()
            .ok_or_else(|| "voplay: missing post color target".to_string())?;
        let main_color_view = if MAIN_SAMPLE_COUNT > 1 {
            ctx.targets
                .msaa_color_view()
                .ok_or_else(|| "voplay: missing MSAA color target".to_string())?
        } else {
            post_color_view
        };
        let receiver_mask_view = if ctx.main_aux_targets_enabled {
            Some(
                ctx.targets
                    .receiver_mask_view()
                    .ok_or_else(|| "voplay: missing receiver mask target".to_string())?,
            )
        } else {
            None
        };
        let surface_props_view = if ctx.main_aux_targets_enabled {
            Some(
                ctx.targets
                    .surface_props_view()
                    .ok_or_else(|| "voplay: missing surface props target".to_string())?,
            )
        } else {
            None
        };
        let main_receiver_mask_view = if ctx.main_aux_targets_enabled {
            Some(if MAIN_SAMPLE_COUNT > 1 {
                ctx.targets
                    .msaa_receiver_mask_view()
                    .ok_or_else(|| "voplay: missing MSAA receiver mask target".to_string())?
            } else {
                receiver_mask_view
                    .ok_or_else(|| "voplay: missing receiver mask target".to_string())?
            })
        } else {
            None
        };
        let main_surface_props_view = if ctx.main_aux_targets_enabled {
            Some(if MAIN_SAMPLE_COUNT > 1 {
                ctx.targets
                    .msaa_surface_props_view()
                    .ok_or_else(|| "voplay: missing MSAA surface props target".to_string())?
            } else {
                surface_props_view
                    .ok_or_else(|| "voplay: missing surface props target".to_string())?
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
        let color_store = wgpu::StoreOp::Store;
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
            depth_stencil_attachment: ctx.targets.depth_view().map(|dv| {
                wgpu::RenderPassDepthStencilAttachment {
                    view: dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: depth_attachment_store_contract(RenderPassKind::MainOpaque)
                            .wgpu_store_op(),
                    }),
                    stencil_ops: None,
                }
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        ctx.perf.main_pass_setup_ms = elapsed_ms_opt(main_setup_start);

        let mut mesh_stats = MeshDrawStats::default();
        if let (Some(cubemap_id), Some(camera)) = (ctx.skybox_cubemap_id, ctx.camera3d_state) {
            if let Some(cubemap) = ctx.textures.get_cubemap(cubemap_id) {
                let skybox_start = if ctx.perf_enabled {
                    Some(perf_now())
                } else {
                    None
                };
                let view_rot = math3d::view_rotation_only(camera.eye, camera.target, camera.up);
                let proj = math3d::perspective_rh_zo(
                    camera.fov_degrees.to_radians(),
                    ctx.aspect,
                    camera.near,
                    camera.far,
                );
                let vp = math3d::mat4_mul(&proj, &view_rot);
                let inv_vp = math3d::mat4_inverse(&vp).unwrap_or(math3d::MAT4_IDENTITY);
                ctx.skybox_pipeline.set_camera(ctx.queue, &inv_vp);
                ctx.skybox_pipeline
                    .draw(&mut render_pass, cubemap, ctx.main_aux_targets_enabled);
                ctx.perf.main_skybox_ms += elapsed_ms_opt(skybox_start);
            } else {
                mesh_stats.skips.missing_textures =
                    mesh_stats.skips.missing_textures.saturating_add(1);
            }
        }

        if !ctx.model_draws.is_empty() {
            if let Some(cam3d) = ctx.camera3d_uniform {
                let model_start = if ctx.perf_enabled {
                    Some(perf_now())
                } else {
                    None
                };
                ctx.mesh_pipeline
                    .set_camera_and_lights(ctx.queue, cam3d, ctx.light_uniform);
                let shadow_view = ctx.shadow_pipeline.shadow_texture_view();
                mesh_stats = ctx.mesh_pipeline.draw_models(
                    ctx.device,
                    ctx.queue,
                    &mut render_pass,
                    ctx.model_draws,
                    ctx.models,
                    ctx.textures,
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
                ctx.primitive_pipeline
                    .set_camera_and_lights(ctx.queue, cam3d, ctx.light_uniform);
                let shadow_view = ctx.shadow_pipeline.shadow_texture_view();
                primitive_stats = ctx.primitive_pipeline.draw(
                    ctx.device,
                    ctx.queue,
                    &mut render_pass,
                    ctx.primitive_draws,
                    ctx.primitive_chunks,
                    ctx.models,
                    ctx.textures,
                    shadow_view,
                    ctx.main_aux_targets_enabled,
                    crate::primitive_pipeline::PrimitiveRenderFilter::Main,
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
            mesh_stats,
            primitive_stats,
        })
    }

    pub(super) fn workload(
        mesh_stats: MeshDrawStats,
        primitive_stats: PrimitiveDrawStats,
    ) -> RenderPassWorkload {
        let mut skips = mesh_stats.skips;
        skips.merge(primitive_stats.skips);
        RenderPassWorkload {
            draw_calls: mesh_stats
                .draw_calls
                .saturating_add(primitive_stats.batch_count),
            batches: mesh_stats
                .batches
                .saturating_add(primitive_stats.batch_count),
            instances: mesh_stats
                .instances
                .saturating_add(primitive_stats.instance_count),
            triangles: mesh_stats
                .triangles
                .saturating_add(primitive_stats.triangle_count),
            upload_bytes: mesh_stats
                .upload_bytes
                .saturating_add(primitive_stats.upload_bytes),
            skips,
        }
    }
}
