//! wgpu-based renderer for voplay.
//! Phase 1: device initialization + 2D shape pipeline (rect, circle, line).

use crate::pipeline2d::{Pipeline2D, ShapeBatch};
use crate::stream::{DrawCommand, StreamReader};

/// Renderer holds all wgpu state.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    depth_view: Option<wgpu::TextureView>,
    pipeline2d: Pipeline2D,
    shape_batch: ShapeBatch,
}

impl Renderer {
    /// Create a new renderer from an existing wgpu instance + surface.
    /// The surface MUST have been created from the same instance.
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
        let pipeline2d = Pipeline2D::new(&device, &queue, format);
        let shape_batch = ShapeBatch::new(w, h);

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view: Some(depth_view),
            pipeline2d,
            shape_batch,
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
        let pipeline2d = Pipeline2D::new(&device, &queue, surface_config.format);
        let shape_batch = ShapeBatch::new(w, h);
        Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view: Some(depth_view),
            pipeline2d,
            shape_batch,
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
        self.shape_batch.set_screen_space(width as f32, height as f32);
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

        // Decode command stream into shape batch
        let mut clear_color = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        self.shape_batch.clear();
        self.shape_batch.set_screen_space(w, h);

        let mut reader = StreamReader::new(data);
        while let Some(cmd) = reader.next_command() {
            match cmd {
                DrawCommand::Clear { r, g, b, a } => {
                    clear_color = wgpu::Color {
                        r: r as f64, g: g as f64, b: b as f64, a: a as f64,
                    };
                }
                DrawCommand::SetCamera2D { x, y, zoom, rotation } => {
                    self.shape_batch.set_camera_2d(w, h, x, y, zoom, rotation);
                }
                DrawCommand::ResetCamera => {
                    self.shape_batch.set_screen_space(w, h);
                }
                DrawCommand::SetLayer { .. } => {
                    // TODO: Phase 1.5 — z-ordering via depth or sort
                }
                DrawCommand::DrawRect { x, y, w, h, r, g, b, a } => {
                    self.shape_batch.push_rect(x, y, w, h, [r, g, b, a]);
                }
                DrawCommand::DrawCircle { cx, cy, radius, r, g, b, a } => {
                    self.shape_batch.push_circle(cx, cy, radius, [r, g, b, a]);
                }
                DrawCommand::DrawLine { x1, y1, x2, y2, thickness, r, g, b, a } => {
                    self.shape_batch.push_line(x1, y1, x2, y2, thickness, [r, g, b, a]);
                }
                DrawCommand::DrawText { .. } => {
                    // TODO: Phase 4 — text rendering
                }
            }
        }

        // Upload shape batch to GPU (before render pass borrows pipeline)
        self.pipeline2d.prepare(&self.device, &self.queue, &self.shape_batch);

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

            // Draw 2D shapes
            let shape_count = self.shape_batch.instance_count() as u32;
            self.pipeline2d.draw(&mut render_pass, shape_count);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
