//! wgpu-based renderer for voplay.
//! Manages device, surface, camera, and all rendering pipelines (shapes, sprites).

use crate::draw_list::{DrawList2D, DrawCallKind};
use crate::font_manager::FontManager;
use crate::math3d;
use crate::model_loader::{ModelId, ModelManager};
use crate::pipeline2d::{Pipeline2D, CameraUniform};
use crate::pipeline3d::{Pipeline3D, Camera3DUniform, ModelUniform, LightUniform, LightData, ModelDraw};
use crate::pipeline_sprite::{PipelineSprite, SpriteInstance};
use crate::texture::{TextureId, TextureManager};
use crate::stream::{DrawCommand, StreamReader};

/// Maximum number of camera states per frame before buffer regrow.
const INITIAL_CAMERA_SLOTS: usize = 16;

/// Renderer holds all wgpu state and rendering pipelines.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    depth_view: Option<wgpu::TextureView>,
    // Camera (shared by all 2D pipelines, dynamic offset)
    camera_bgl: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    camera_slot_capacity: usize,
    camera_alignment: u32,
    // Pipelines
    pipeline2d: Pipeline2D,
    pipeline_sprite: PipelineSprite,
    // Unified 2D draw list (replaces ShapeBatch + SpriteBatch)
    draw_list: DrawList2D,
    // 3D pipeline
    pipeline3d: Pipeline3D,
    model_manager: ModelManager,
    // Texture manager
    texture_manager: TextureManager,
    // Font manager for text rendering (TTF/OTF, dynamic glyph atlas)
    font_manager: FontManager,
}

impl Renderer {
    /// Create shared camera bind group layout (dynamic offset).
    fn create_camera_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<CameraUniform>() as u64,
                    ),
                },
                count: None,
            }],
        })
    }

    /// Create (or recreate) the camera buffer and bind group for `slot_count` slots.
    fn create_camera_buffer_and_bg(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        slot_count: usize,
        alignment: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let total_size = slot_count as u64 * alignment as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_camera"),
            size: total_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_camera_bg"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<CameraUniform>() as u64),
                }),
            }],
        });
        (buffer, bind_group)
    }

    /// Create a new renderer from an existing wgpu instance + surface.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| "voplay: no suitable GPU adapter found".to_string())?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("voplay"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
            }, None)
            .await
            .map_err(|e| format!("voplay: request_device failed: {}", e))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let depth_view = Self::create_depth_view(&device, width, height);
        let w = width as f32;
        let h = height as f32;

        let camera_bgl = Self::create_camera_bgl(&device);
        let camera_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let (camera_buffer, camera_bind_group) = Self::create_camera_buffer_and_bg(
            &device, &camera_bgl, INITIAL_CAMERA_SLOTS, camera_alignment,
        );
        let mut texture_manager = TextureManager::new(&device);
        let pipeline2d = Pipeline2D::new(&device, &queue, format, &camera_bgl);
        let tex_bgl = texture_manager.bind_group_layout();
        let pipeline_sprite = PipelineSprite::new(
            &device, &queue, format, &camera_bgl, tex_bgl,
        );
        let pipeline3d = Pipeline3D::new(&device, &queue, format, tex_bgl);
        let model_manager = ModelManager::new();
        let mut font_manager = FontManager::new();
        font_manager.ensure_atlas(&mut texture_manager, &device, &queue);
        let draw_list = DrawList2D::new(w, h);

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view: Some(depth_view),
            camera_bgl,
            camera_buffer,
            camera_bind_group,
            camera_slot_capacity: INITIAL_CAMERA_SLOTS,
            camera_alignment,
            pipeline2d,
            pipeline_sprite,
            draw_list,
            pipeline3d,
            model_manager,
            texture_manager,
            font_manager,
        })
    }

    /// Create renderer with pre-existing device + queue + surface (for wasm-integrated path).
    pub fn from_parts(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> Self {
        let depth_view = Self::create_depth_view(&device, surface_config.width, surface_config.height);
        let w = surface_config.width as f32;
        let h = surface_config.height as f32;

        let camera_bgl = Self::create_camera_bgl(&device);
        let camera_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let (camera_buffer, camera_bind_group) = Self::create_camera_buffer_and_bg(
            &device, &camera_bgl, INITIAL_CAMERA_SLOTS, camera_alignment,
        );
        let mut texture_manager = TextureManager::new(&device);
        let pipeline2d = Pipeline2D::new(&device, &queue, surface_config.format, &camera_bgl);
        let tex_bgl = texture_manager.bind_group_layout();
        let pipeline_sprite = PipelineSprite::new(
            &device, &queue, surface_config.format, &camera_bgl, tex_bgl,
        );
        let pipeline3d = Pipeline3D::new(&device, &queue, surface_config.format, tex_bgl);
        let model_manager = ModelManager::new();
        let mut font_manager = FontManager::new();
        font_manager.ensure_atlas(&mut texture_manager, &device, &queue);
        let draw_list = DrawList2D::new(w, h);

        Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view: Some(depth_view),
            camera_bgl,
            camera_buffer,
            camera_bind_group,
            camera_slot_capacity: INITIAL_CAMERA_SLOTS,
            camera_alignment,
            pipeline2d,
            pipeline_sprite,
            draw_list,
            pipeline3d,
            model_manager,
            texture_manager,
            font_manager,
        }
    }

    fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        depth_texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Resize the surface and depth buffer.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.depth_view = Some(Self::create_depth_view(&self.device, width, height));
    }

    // --- Model management ---

    /// Load a model from a file path. Returns ModelId.
    pub fn load_model(&mut self, path: &str) -> Result<ModelId, String> {
        self.model_manager.load_file(&self.device, &self.queue, &mut self.texture_manager, path)
    }

    /// Load a model from raw glTF/GLB bytes. Returns ModelId.
    pub fn load_model_bytes(&mut self, data: &[u8]) -> Result<ModelId, String> {
        self.model_manager.load_bytes(&self.device, &self.queue, &mut self.texture_manager, data, None)
    }

    /// Free a loaded model by ID.
    pub fn free_model(&mut self, id: ModelId) {
        self.model_manager.free(id);
    }

    // --- Font management ---

    /// Load a font from a file path. Returns FontId.
    pub fn load_font(&mut self, path: &str) -> Result<crate::font_manager::FontId, String> {
        self.font_manager.load_file(path)
    }

    /// Load a font from raw TTF/OTF bytes. Returns FontId.
    pub fn load_font_bytes(&mut self, data: Vec<u8>) -> Result<crate::font_manager::FontId, String> {
        self.font_manager.load_bytes(data)
    }

    /// Free a loaded font by ID.
    pub fn free_font(&mut self, id: crate::font_manager::FontId) {
        self.font_manager.free(id);
    }

    /// Measure text dimensions (width, height) using the specified font.
    pub fn measure_text(&self, font_id: crate::font_manager::FontId, text: &str, size: f32) -> (f32, f32) {
        self.font_manager.measure_text(font_id, text, size)
    }

    // --- Texture management ---

    /// Load a texture from a file path. Returns TextureId.
    pub fn load_texture(&mut self, path: &str) -> Result<TextureId, String> {
        self.texture_manager.load_file(&self.device, &self.queue, path)
    }

    /// Load a texture from encoded image bytes (PNG, JPEG, etc.).
    pub fn load_texture_bytes(&mut self, data: &[u8]) -> Result<TextureId, String> {
        self.texture_manager.load_image_bytes(&self.device, &self.queue, data)
    }

    /// Free a texture by ID.
    pub fn free_texture(&mut self, id: TextureId) {
        self.texture_manager.free(id);
    }

    /// Execute a frame's draw command stream.
    pub fn submit_frame(&mut self, data: &[u8]) -> Result<(), String> {
        let output = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("voplay: get_current_texture: {}", e))?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });

        let w = self.surface_config.width as f32;
        let h = self.surface_config.height as f32;

        // Reset draw list for this frame
        let mut clear_color = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        self.draw_list.clear();
        self.draw_list.set_screen_space(w, h);

        // 3D state for this frame
        let mut camera3d_uniform: Option<Camera3DUniform> = None;
        let mut light_uniform = LightUniform {
            ambient: [0.1, 0.1, 0.1, 1.0],
            count: [0, 0, 0, 0],
            lights: [LightData { position_or_dir: [0.0; 4], color_intensity: [0.0; 4] }; 8],
        };
        let mut model_draws: Vec<ModelDraw> = Vec::new();
        let aspect = w / h;

        // Decode command stream into the unified draw list
        let mut reader = StreamReader::new(data);
        while let Some(cmd) = reader.next_command() {
            match cmd {
                DrawCommand::Clear { r, g, b, a } => {
                    clear_color = wgpu::Color {
                        r: r as f64, g: g as f64, b: b as f64, a: a as f64,
                    };
                }
                DrawCommand::SetCamera2D { x, y, zoom, rotation } => {
                    self.draw_list.set_camera_2d(w, h, x, y, zoom, rotation);
                }
                DrawCommand::ResetCamera => {
                    self.draw_list.reset_camera();
                }
                DrawCommand::SetLayer { z } => {
                    self.draw_list.set_layer(z);
                }
                DrawCommand::DrawRect { x, y, w, h, r, g, b, a } => {
                    self.draw_list.push_rect(x, y, w, h, [r, g, b, a]);
                }
                DrawCommand::DrawCircle { cx, cy, radius, r, g, b, a } => {
                    self.draw_list.push_circle(cx, cy, radius, [r, g, b, a]);
                }
                DrawCommand::DrawLine { x1, y1, x2, y2, thickness, r, g, b, a } => {
                    self.draw_list.push_line(x1, y1, x2, y2, thickness, [r, g, b, a]);
                }
                DrawCommand::SetFont { font_id } => {
                    self.font_manager.set_current(font_id);
                }
                DrawCommand::DrawText { x, y, size, r, g, b, a, text } => {
                    let draws = self.font_manager.layout_text(&text, x, y, size, r, g, b, a);
                    for draw in draws {
                        self.draw_list.push_sprite(draw.texture_id, draw.instance);
                    }
                }
                DrawCommand::DrawSprite {
                    tex_id, src_x, src_y, src_w, src_h,
                    dst_x, dst_y, dst_w, dst_h,
                    flip_x, flip_y, rotation,
                    r, g, b, a,
                } => {
                    let (u0, v0, u1, v1) = if let Some(tex) = self.texture_manager.get(tex_id) {
                        if src_w == 0.0 && src_h == 0.0 {
                            // src_w/src_h == 0 means "use full texture"
                            (0.0, 0.0, 1.0, 1.0)
                        } else {
                            let tw = tex.width as f32;
                            let th = tex.height as f32;
                            (src_x / tw, src_y / th, (src_x + src_w) / tw, (src_y + src_h) / th)
                        }
                    } else {
                        (0.0, 0.0, 1.0, 1.0)
                    };
                    self.draw_list.push_sprite(tex_id, SpriteInstance {
                        dst_rect: [dst_x, dst_y, dst_w, dst_h],
                        src_rect: [u0, v0, u1, v1],
                        color: [r, g, b, a],
                        params: [
                            rotation,
                            if flip_x { 1.0 } else { 0.0 },
                            if flip_y { 1.0 } else { 0.0 },
                            0.0,
                        ],
                    });
                }
                // --- 3D commands ---
                DrawCommand::SetCamera3D {
                    eye_x, eye_y, eye_z,
                    target_x, target_y, target_z,
                    up_x, up_y, up_z,
                    fov, near, far,
                } => {
                    let v = math3d::look_at_rh(
                        [eye_x, eye_y, eye_z],
                        [target_x, target_y, target_z],
                        [up_x, up_y, up_z],
                    );
                    let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                    let view_proj = math3d::mat4_mul(&proj, &v);
                    camera3d_uniform = Some(Camera3DUniform {
                        view_proj,
                        camera_pos: [eye_x, eye_y, eye_z],
                        _pad: 0.0,
                    });
                }
                DrawCommand::SetLights3D { ambient_r, ambient_g, ambient_b, lights } => {
                    light_uniform.ambient = [ambient_r, ambient_g, ambient_b, 1.0];
                    let count = lights.len().min(8);
                    light_uniform.count = [count as u32, 0, 0, 0];
                    for (i, l) in lights.iter().take(8).enumerate() {
                        let (pos_or_dir, w_type) = if l.light_type == 0 {
                            ([l.dx, l.dy, l.dz], 0.0f32)
                        } else {
                            ([l.px, l.py, l.pz], 1.0f32)
                        };
                        light_uniform.lights[i] = LightData {
                            position_or_dir: [pos_or_dir[0], pos_or_dir[1], pos_or_dir[2], w_type],
                            color_intensity: [l.cr, l.cg, l.cb, l.intensity],
                        };
                    }
                }
                DrawCommand::DrawModel {
                    model_id, px, py, pz, qx, qy, qz, qw, sx, sy, sz,
                } => {
                    let model_mat = math3d::model_matrix(px, py, pz, qx, qy, qz, qw, sx, sy, sz);
                    let normal_mat = math3d::transpose_upper3x3(&model_mat);
                    let base_color = self.model_manager.get(model_id)
                        .and_then(|m| m.meshes.first())
                        .map(|mesh| mesh.material.base_color)
                        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                    model_draws.push(ModelDraw {
                        model_id,
                        model_uniform: ModelUniform {
                            model: model_mat,
                            normal_matrix: normal_mat,
                            base_color,
                        },
                    });
                }
                DrawCommand::DrawBillboard {
                    tex_id, src_x, src_y, src_w, src_h,
                    world_x, world_y, world_z, w: bw, h: bh,
                } => {
                    // Project 3D world position to screen coordinates using the current 3D camera
                    if let Some(ref cam) = camera3d_uniform {
                        let clip = math3d::mat4_mul_vec4(&cam.view_proj, [world_x, world_y, world_z, 1.0]);
                        if clip[3] > 0.0 {
                            let ndc_x = clip[0] / clip[3];
                            let ndc_y = clip[1] / clip[3];
                            // NDC → screen: x = (ndc+1)/2 * w, y = (1-ndc)/2 * h
                            let screen_x = (ndc_x + 1.0) * 0.5 * w - bw * 0.5;
                            let screen_y = (1.0 - ndc_y) * 0.5 * h - bh * 0.5;

                            let (u0, v0, u1, v1) = if let Some(tex) = self.texture_manager.get(tex_id) {
                                if src_w == 0.0 && src_h == 0.0 {
                                    (0.0, 0.0, 1.0, 1.0)
                                } else {
                                    let tw = tex.width as f32;
                                    let th = tex.height as f32;
                                    (src_x / tw, src_y / th, (src_x + src_w) / tw, (src_y + src_h) / th)
                                }
                            } else {
                                (0.0, 0.0, 1.0, 1.0)
                            };

                            self.draw_list.push_sprite(tex_id, SpriteInstance {
                                dst_rect: [screen_x, screen_y, bw, bh],
                                src_rect: [u0, v0, u1, v1],
                                color: [1.0, 1.0, 1.0, 1.0],
                                params: [0.0, 0.0, 0.0, 0.0],
                            });
                        }
                    }
                }
            }
        }

        // Flush font atlas (re-upload if new glyphs were rasterized)
        self.font_manager.ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        // Resolve draw list: sort by (layer, order), produce draw calls
        let frame = self.draw_list.resolve();

        // Upload all camera uniforms into the dynamic offset buffer
        let align = self.camera_alignment;
        let cam_count = frame.cameras.len();
        if cam_count > self.camera_slot_capacity {
            let new_cap = cam_count.next_power_of_two();
            let (buf, bg) = Self::create_camera_buffer_and_bg(
                &self.device, &self.camera_bgl, new_cap, align,
            );
            self.camera_buffer = buf;
            self.camera_bind_group = bg;
            self.camera_slot_capacity = new_cap;
        }
        for (i, cam) in frame.cameras.iter().enumerate() {
            let offset = i as u64 * align as u64;
            self.queue.write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(cam));
        }

        // Upload sorted 2D instance data
        self.pipeline2d.upload_instances(&self.device, &self.queue, &frame.shapes);
        self.pipeline_sprite.upload_instances(&self.device, &self.queue, &frame.sprites);

        // Render pass
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay_main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: self.depth_view.as_ref().map(|dv| {
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

            // Draw 3D models first (depth tested)
            if !model_draws.is_empty() {
                if let Some(ref cam3d) = camera3d_uniform {
                    self.pipeline3d.set_camera_and_lights(&self.queue, cam3d, &light_uniform);
                    self.pipeline3d.draw_models(
                        &self.queue,
                        &mut render_pass,
                        &model_draws,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                }
            }

            // Draw 2D content in layer-sorted order
            for dc in &frame.draw_calls {
                let cam_offset = dc.camera_idx as u32 * align;
                match &dc.kind {
                    DrawCallKind::Shapes { start, count } => {
                        self.pipeline2d.draw_range(
                            &mut render_pass,
                            &self.camera_bind_group,
                            &[cam_offset],
                            *start,
                            *count,
                        );
                    }
                    DrawCallKind::Sprites { texture_id, start, count } => {
                        if let Some(tex) = self.texture_manager.get(*texture_id) {
                            self.pipeline_sprite.draw_range(
                                &mut render_pass,
                                &self.camera_bind_group,
                                &[cam_offset],
                                &tex.bind_group,
                                *start,
                                *count,
                            );
                        }
                    }
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
