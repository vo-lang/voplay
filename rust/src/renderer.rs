//! wgpu-based renderer for voplay.
//! Manages device, surface, camera, and all rendering pipelines (shapes, sprites).

use std::collections::HashMap;

use crate::draw_list::{DrawCallKind, DrawList2D};
use crate::font_manager::FontManager;
use crate::math3d::{self, Vec3};
use crate::model_loader::{LevelNode, MeshMaterial, ModelId, ModelManager};
use crate::pipeline2d::{CameraUniform, Pipeline2D};
use crate::pipeline3d::{
    Camera3DUniform, LightData, LightUniform, ModelDraw, ModelUniform, Pipeline3D,
};
use crate::pipeline_shadow::PipelineShadow;
use crate::pipeline_skybox::PipelineSkybox;
use crate::pipeline_sprite::{PipelineSprite, SpriteInstance};
use crate::primitive_pipeline::PrimitivePipeline;
use crate::primitive_scene::{PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate};
use crate::render_world::{RenderObjectUpdate, RenderWorld};
use crate::stream::{DrawCommand, StreamReader};
use crate::terrain::TerrainData;
use crate::texture::{TextureId, TextureManager};

/// Maximum number of camera states per frame before buffer regrow.
const INITIAL_CAMERA_SLOTS: usize = 16;

/// Renderer holds all wgpu state and rendering pipelines.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    depth_view: Option<wgpu::TextureView>,
    // Canvas element id (WASM only) for auto-resize on layout changes.
    canvas_id: Option<String>,
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
    primitive_pipeline: PrimitivePipeline,
    pipeline_shadow: PipelineShadow,
    pipeline_skybox: PipelineSkybox,
    model_manager: ModelManager,
    // Texture manager
    texture_manager: TextureManager,
    // Font manager for text rendering (TTF/OTF, dynamic glyph atlas)
    font_manager: FontManager,
    render_world: RenderWorld,
    primitive_shapes: HashMap<(u32, u32, u32), u32>,
    primitive_materials: HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
    debug_frame_count: u64,
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
                        std::mem::size_of::<CameraUniform>() as u64
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

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        surface_config: wgpu::SurfaceConfiguration,
        canvas_id: Option<String>,
    ) -> Result<Self, String> {
        let depth_view =
            Self::create_depth_view(&device, surface_config.width, surface_config.height);
        let w = surface_config.width as f32;
        let h = surface_config.height as f32;

        let camera_bgl = Self::create_camera_bgl(&device);
        let camera_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let (camera_buffer, camera_bind_group) = Self::create_camera_buffer_and_bg(
            &device,
            &camera_bgl,
            INITIAL_CAMERA_SLOTS,
            camera_alignment,
        );
        let mut texture_manager = TextureManager::new(&device);
        let pipeline2d = Pipeline2D::new(&device, &queue, surface_config.format, &camera_bgl);
        let tex_bgl = texture_manager.bind_group_layout();
        let cubemap_bgl = texture_manager.cubemap_bind_group_layout();
        let pipeline_sprite =
            PipelineSprite::new(&device, &queue, surface_config.format, &camera_bgl, tex_bgl);
        #[cfg(feature = "wasm")]
        let debug_pipeline_errors = crate::externs::render::wasm_debug_enabled();
        #[cfg(feature = "wasm")]
        if debug_pipeline_errors {
            device.push_error_scope(wgpu::ErrorFilter::Validation);
        }
        let pipeline3d = Pipeline3D::new(&device, &queue, surface_config.format);
        let primitive_pipeline = PrimitivePipeline::new(&device, &queue, surface_config.format);
        #[cfg(feature = "wasm")]
        if debug_pipeline_errors {
            Self::pop_debug_error_scope(&device, "voplay pipeline3d create");
        }
        let pipeline_shadow = PipelineShadow::new(&device, 2048);
        let pipeline_skybox = PipelineSkybox::new(&device, surface_config.format, cubemap_bgl);
        let model_manager = ModelManager::new();
        let mut font_manager = FontManager::new()?;
        font_manager.ensure_atlas(&mut texture_manager, &device, &queue);
        let draw_list = DrawList2D::new(w, h);
        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            depth_view: Some(depth_view),
            canvas_id,
            camera_bgl,
            camera_buffer,
            camera_bind_group,
            camera_slot_capacity: INITIAL_CAMERA_SLOTS,
            camera_alignment,
            pipeline2d,
            pipeline_sprite,
            draw_list,
            pipeline3d,
            primitive_pipeline,
            pipeline_shadow,
            pipeline_skybox,
            model_manager,
            texture_manager,
            font_manager,
            render_world: RenderWorld::new(),
            primitive_shapes: HashMap::new(),
            primitive_materials: HashMap::new(),
            debug_frame_count: 0,
        })
    }

    /// Create a new renderer from an existing wgpu instance + surface.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        no_vsync: bool,
    ) -> Result<Self, String> {
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
        {
            Some(adapter) => adapter,
            None => {
                Self::debug_renderer_status(
                    "voplay renderer high-performance adapter unavailable; trying fallback adapter",
                );
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::LowPower,
                        compatible_surface: Some(&surface),
                        force_fallback_adapter: true,
                    })
                    .await
                    .ok_or_else(|| "voplay: no suitable GPU adapter found".to_string())?
            }
        };
        let adapter_info = adapter.get_info();
        Self::debug_renderer_status(&format!(
            "voplay renderer adapter backend={:?} device={} name={} driver={}",
            adapter_info.backend, adapter_info.device, adapter_info.name, adapter_info.driver
        ));

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("voplay"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("voplay: request_device failed: {}", e))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);
        Self::debug_renderer_status(&format!(
            "voplay renderer surface format={:?} alpha={:?} size={}x{}",
            format,
            surface_caps.alpha_modes[0],
            width.max(1),
            height.max(1)
        ));

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: if no_vsync {
                wgpu::PresentMode::AutoNoVsync
            } else {
                wgpu::PresentMode::AutoVsync
            },
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &surface_config);

        Self::build(device, queue, surface, surface_config, None)
    }

    /// Create renderer with pre-existing device + queue + surface (for wasm-integrated path).
    pub fn from_parts(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        surface_config: wgpu::SurfaceConfiguration,
    ) -> Result<Self, String> {
        Self::build(device, queue, surface, surface_config, None)
    }

    /// Set the canvas element id for automatic resize detection (WASM only).
    pub fn set_canvas_id(&mut self, id: String) {
        self.canvas_id = Some(id);
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
        if self.surface_config.width == width && self.surface_config.height == height {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        eprintln!("voplay: renderer resize {}x{}", width, height);
        self.surface.configure(&self.device, &self.surface_config);
        self.depth_view = Some(Self::create_depth_view(&self.device, width, height));
    }

    // --- Model management ---

    /// Load a model from a file path. Returns ModelId.
    pub fn load_model(&mut self, path: &str) -> Result<ModelId, String> {
        self.model_manager
            .load_file(&self.device, &self.queue, &mut self.texture_manager, path)
    }

    /// Load a model from raw glTF/GLB bytes. Returns ModelId.
    pub fn load_model_bytes(&mut self, data: &[u8]) -> Result<ModelId, String> {
        self.model_manager.load_bytes(
            &self.device,
            &self.queue,
            &mut self.texture_manager,
            data,
            None,
        )
    }

    pub fn load_level(&mut self, path: &str) -> Result<Vec<LevelNode>, String> {
        self.model_manager.load_level_file(
            &self.device,
            &self.queue,
            &mut self.texture_manager,
            path,
        )
    }

    pub fn create_plane(&mut self, width: f32, depth: f32, sub_x: u32, sub_z: u32) -> ModelId {
        self.model_manager
            .create_plane(&self.device, &self.queue, width, depth, sub_x, sub_z)
    }

    pub fn create_cube(&mut self) -> ModelId {
        self.model_manager.create_cube(&self.device, &self.queue)
    }

    pub fn create_rounded_box(&mut self, bevel_radius: f32, segments: u32) -> ModelId {
        self.model_manager
            .create_rounded_box(&self.device, &self.queue, bevel_radius, segments)
    }

    pub fn create_sphere(&mut self, segments: u32) -> ModelId {
        self.model_manager
            .create_sphere(&self.device, &self.queue, segments)
    }

    pub fn create_cylinder(&mut self, segments: u32) -> ModelId {
        self.model_manager
            .create_cylinder(&self.device, &self.queue, segments)
    }

    pub fn create_cone(&mut self, segments: u32) -> ModelId {
        self.model_manager
            .create_cone(&self.device, &self.queue, segments)
    }

    pub fn create_wedge(&mut self) -> ModelId {
        self.model_manager.create_wedge(&self.device, &self.queue)
    }

    pub fn create_capsule(&mut self, segments: u32, half_height: f32, radius: f32) -> ModelId {
        self.model_manager
            .create_capsule(&self.device, &self.queue, segments, half_height, radius)
    }

    pub fn create_terrain(
        &mut self,
        image_data: &[u8],
        scale_x: f32,
        scale_y: f32,
        scale_z: f32,
        uv_scale: f32,
        texture_id: Option<TextureId>,
        normal_texture_id: Option<TextureId>,
        metallic_roughness_texture_id: Option<TextureId>,
        normal_scale: f32,
        roughness: f32,
        metallic: f32,
    ) -> Result<TerrainData, String> {
        let mut material = MeshMaterial::standard([1.0, 1.0, 1.0, 1.0], texture_id, uv_scale);
        material.normal_texture_id = normal_texture_id;
        material.metallic_roughness_texture_id = metallic_roughness_texture_id;
        if normal_scale > 0.0 {
            material.normal_scale = normal_scale;
        }
        if roughness > 0.0 {
            material.roughness = roughness.clamp(0.04, 1.0);
        }
        material.metallic = metallic.clamp(0.0, 1.0);
        crate::terrain::generate_terrain(
            &self.device,
            &self.queue,
            &mut self.model_manager,
            image_data,
            scale_x,
            scale_y,
            scale_z,
            material,
        )
    }

    pub fn create_terrain_splat(
        &mut self,
        image_data: &[u8],
        scale_x: f32,
        scale_y: f32,
        scale_z: f32,
        control_texture_id: TextureId,
        layer_texture_ids: [TextureId; 4],
        layer_normal_texture_ids: [TextureId; 4],
        layer_metallic_roughness_texture_ids: [TextureId; 4],
        uv_scales: [f32; 4],
        layer_normal_scales: [f32; 4],
    ) -> Result<TerrainData, String> {
        if uv_scales
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(format!(
                "terrain splat uv scales must be finite and > 0, got {:?}",
                uv_scales
            ));
        }
        if layer_normal_scales
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(format!(
                "terrain splat normal scales must be finite and >= 0, got {:?}",
                layer_normal_scales
            ));
        }
        let material = MeshMaterial::terrain_splat(
            [1.0, 1.0, 1.0, 1.0],
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            layer_normal_scales,
        );
        crate::terrain::generate_terrain(
            &self.device,
            &self.queue,
            &mut self.model_manager,
            image_data,
            scale_x,
            scale_y,
            scale_z,
            material,
        )
    }

    /// Free a loaded model by ID.
    pub fn free_model(&mut self, id: ModelId) {
        self.model_manager.free(id);
    }

    pub fn model_bounds(&self, model_id: ModelId) -> Option<([f32; 3], [f32; 3])> {
        let model = self.model_manager.get(model_id)?;
        Some((model.aabb_min, model.aabb_max))
    }

    pub fn get_model_mesh_data(&self, model_id: ModelId) -> Option<(Vec<[f32; 3]>, Vec<u32>)> {
        let model = self.model_manager.get(model_id)?;
        Some((model.cpu_positions.clone(), model.cpu_indices.clone()))
    }

    pub fn get_model_animation_info(
        &self,
        model_id: ModelId,
    ) -> Option<crate::animation::ModelAnimationInfo> {
        self.model_manager.animation_info(model_id)
    }

    pub fn tick_animations(
        &mut self,
        world_id: u32,
        dt: f32,
        entity_models: &std::collections::HashMap<u32, ModelId>,
    ) {
        crate::animation::with_world(world_id, |world| {
            world.tick(dt, &self.model_manager, entity_models)
        });
    }

    pub fn animation_progress(&self, world_id: u32, target_id: u32, model_id: ModelId) -> f32 {
        crate::animation::with_world(world_id, |world| {
            world.progress(target_id, &self.model_manager, model_id)
        })
    }

    // --- Font management ---

    /// Load a font from a file path. Returns FontId.
    pub fn load_font(&mut self, path: &str) -> Result<crate::font_manager::FontId, String> {
        self.font_manager.load_file(path)
    }

    /// Load a font from raw TTF/OTF bytes. Returns FontId.
    pub fn load_font_bytes(
        &mut self,
        data: Vec<u8>,
    ) -> Result<crate::font_manager::FontId, String> {
        self.font_manager.load_bytes(data)
    }

    /// Free a loaded font by ID.
    pub fn free_font(&mut self, id: crate::font_manager::FontId) {
        self.font_manager.free(id);
    }

    /// Measure text dimensions (width, height) using the specified font.
    pub fn measure_text(
        &self,
        font_id: crate::font_manager::FontId,
        text: &str,
        size: f32,
    ) -> (f32, f32) {
        self.font_manager.measure_text(font_id, text, size)
    }

    // --- Texture management ---

    /// Load a texture from a file path. Returns TextureId.
    pub fn load_texture(&mut self, path: &str) -> Result<TextureId, String> {
        self.texture_manager
            .load_file(&self.device, &self.queue, path)
    }

    /// Load a texture from encoded image bytes (PNG, JPEG, etc.).
    pub fn load_texture_bytes(&mut self, data: &[u8]) -> Result<TextureId, String> {
        self.texture_manager
            .load_image_bytes(&self.device, &self.queue, data)
    }

    pub fn load_cubemap(&mut self, paths: [&str; 6]) -> Result<u32, String> {
        self.texture_manager
            .load_cubemap_files(&self.device, &self.queue, paths)
    }

    pub fn load_cubemap_bytes(&mut self, faces: [&[u8]; 6]) -> Result<u32, String> {
        self.texture_manager
            .load_cubemap_image_bytes(&self.device, &self.queue, faces)
    }

    /// Free a texture by ID.
    pub fn free_texture(&mut self, id: TextureId) {
        self.texture_manager.free(id);
        self.clear_texture_bind_group_caches();
    }

    pub fn free_cubemap(&mut self, id: u32) {
        self.texture_manager.free_cubemap(id);
    }

    fn fog_billboard_color(
        tint: [f32; 4],
        world_pos: Vec3,
        camera: &Camera3DUniform,
        lights: &LightUniform,
    ) -> [f32; 4] {
        let fog_mode = lights.count[1];
        if fog_mode == 0 {
            return tint;
        }
        let dx = world_pos.x - camera.camera_pos[0];
        let dy = world_pos.y - camera.camera_pos[1];
        let dz = world_pos.z - camera.camera_pos[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        let start = lights.fog_params[0];
        let end = lights.fog_params[1];
        let density = lights.fog_params[2];
        let factor = if fog_mode == 1 {
            ((end - dist) / (end - start)).clamp(0.0, 1.0)
        } else if fog_mode == 2 {
            (-density * dist).exp()
        } else {
            let dd = density * dist;
            (-(dd * dd)).exp()
        };
        [
            lights.fog_color[0] + (tint[0] - lights.fog_color[0]) * factor,
            lights.fog_color[1] + (tint[1] - lights.fog_color[1]) * factor,
            lights.fog_color[2] + (tint[2] - lights.fog_color[2]) * factor,
            tint[3],
        ]
    }

    fn clear_texture_bind_group_caches(&mut self) {
        self.pipeline3d.clear_texture_bind_group_cache();
        self.primitive_pipeline.clear_texture_bind_group_cache();
    }

    /// Check if the canvas CSS size has changed and reconfigure the surface.
    /// Standard WebGPU practice: the backing buffer must match the CSS layout size.
    /// Also detects DOM reparenting (canvas backing buffer mismatch) and forces
    /// a reconfigure even if the target size hasn't changed, because moving a
    /// canvas in the DOM can invalidate the WebGPU surface.
    #[cfg(feature = "wasm")]
    fn auto_resize_from_canvas(&mut self) {
        use wasm_bindgen::JsCast;
        let canvas_id = match self.canvas_id {
            Some(ref id) => id.clone(),
            None => return,
        };
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(el) = document.get_element_by_id(&canvas_id) else {
            return;
        };
        let Ok(canvas) = el.dyn_into::<web_sys::HtmlCanvasElement>() else {
            return;
        };
        let dpr = window.device_pixel_ratio();
        let css_w = canvas.client_width().max(1) as f64;
        let css_h = canvas.client_height().max(1) as f64;
        let w = (css_w * dpr) as u32;
        let h = (css_h * dpr) as u32;
        // Detect backing buffer mismatch (happens after DOM reparent or external resize).
        let backing_w = canvas.width();
        let backing_h = canvas.height();
        let size_changed = w != self.surface_config.width || h != self.surface_config.height;
        let backing_dirty = backing_w != w || backing_h != h;
        if size_changed || backing_dirty {
            eprintln!(
                "voplay: auto_resize_from_canvas {}x{} (was {}x{}, backing {}x{})",
                w, h, self.surface_config.width, self.surface_config.height, backing_w, backing_h
            );
            canvas.set_width(w);
            canvas.set_height(h);
            self.resize(w, h);
        }
    }

    /// Execute a frame's draw command stream.
    pub fn submit_frame(&mut self, data: &[u8]) -> Result<(), String> {
        self.debug_frame_count = self.debug_frame_count.wrapping_add(1);
        let debug_frame_count = self.debug_frame_count;
        #[cfg(feature = "wasm")]
        let debug_scope_frame = Self::debug_should_log_frame(debug_frame_count);

        #[cfg(feature = "wasm")]
        self.auto_resize_from_canvas();
        #[cfg(feature = "wasm")]
        if debug_scope_frame {
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        }

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
        let mut clear_color = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        self.draw_list.clear();
        self.draw_list.set_screen_space(w, h);

        // 3D state for this frame
        let mut camera3d_uniform: Option<Camera3DUniform> = None;
        let mut camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)> = None;
        let mut skybox_cubemap_id: Option<u32> = None;
        let mut shadow_enabled = false;
        let mut shadow_resolution = 2048u32;
        let mut shadow_strength = 1.0f32;
        let mut light_uniform = LightUniform {
            ambient: [0.1, 0.1, 0.1, 1.0],
            ambient_ground: [0.1, 0.1, 0.1, 1.0],
            count: [0, 0, 0, 0],
            lights: [LightData {
                position_or_dir: [0.0; 4],
                color_intensity: [0.0; 4],
            }; 8],
            fog_color: [0.0, 0.0, 0.0, 1.0],
            fog_params: [0.0, 0.0, 0.0, 0.0],
            shadow_vp: math3d::MAT4_IDENTITY,
            shadow_params: [0.0, 0.002, 1.0 / 2048.0, 1.0],
            color_params: [1.0, 1.0, 1.0, 0.0],
            debug_params: [0, 0, 0, 0],
        };
        let mut model_draws: Vec<ModelDraw> = Vec::new();
        let mut primitive_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut retained_scene_draws: Vec<u32> = Vec::new();
        let mut command_count = 0u32;
        let mut rect_count = 0u32;
        let mut circle_count = 0u32;
        let mut line_count = 0u32;
        let mut text_count = 0u32;
        let mut sprite_count = 0u32;
        let mut model_command_count = 0u32;
        let mut scene_upsert_count = 0u32;
        let mut scene_draw_count = 0u32;
        let mut skybox_count = 0u32;
        let aspect = w / h;

        // Decode command stream into the unified draw list
        let mut reader = StreamReader::new(data);
        while let Some(cmd) = reader.next_command() {
            command_count += 1;
            match cmd {
                DrawCommand::Clear { r, g, b, a } => {
                    clear_color = wgpu::Color {
                        r: r as f64,
                        g: g as f64,
                        b: b as f64,
                        a: a as f64,
                    };
                }
                DrawCommand::SetCamera2D {
                    x,
                    y,
                    zoom,
                    rotation,
                } => {
                    self.draw_list.set_camera_2d(w, h, x, y, zoom, rotation);
                }
                DrawCommand::ResetCamera => {
                    self.draw_list.reset_camera();
                }
                DrawCommand::SetLayer { z } => {
                    self.draw_list.set_layer(z);
                }
                DrawCommand::DrawRect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                    a,
                } => {
                    rect_count += 1;
                    self.draw_list.push_rect(x, y, w, h, [r, g, b, a]);
                }
                DrawCommand::DrawCircle {
                    cx,
                    cy,
                    radius,
                    r,
                    g,
                    b,
                    a,
                } => {
                    circle_count += 1;
                    self.draw_list.push_circle(cx, cy, radius, [r, g, b, a]);
                }
                DrawCommand::DrawLine {
                    x1,
                    y1,
                    x2,
                    y2,
                    thickness,
                    r,
                    g,
                    b,
                    a,
                } => {
                    line_count += 1;
                    self.draw_list
                        .push_line(x1, y1, x2, y2, thickness, [r, g, b, a]);
                }
                DrawCommand::SetFont { font_id } => {
                    self.font_manager.set_current(font_id);
                }
                DrawCommand::DrawText {
                    x,
                    y,
                    size,
                    r,
                    g,
                    b,
                    a,
                    text,
                } => {
                    text_count += 1;
                    let draws = self.font_manager.layout_text(&text, x, y, size, r, g, b, a);
                    for draw in draws {
                        self.draw_list.push_sprite(draw.texture_id, draw.instance);
                    }
                }
                DrawCommand::DrawSprite {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    dst_x,
                    dst_y,
                    dst_w,
                    dst_h,
                    flip_x,
                    flip_y,
                    rotation,
                    r,
                    g,
                    b,
                    a,
                } => {
                    sprite_count += 1;
                    let (u0, v0, u1, v1) = if let Some(tex) = self.texture_manager.get(tex_id) {
                        if src_w == 0.0 && src_h == 0.0 {
                            // src_w/src_h == 0 means "use full texture"
                            (0.0, 0.0, 1.0, 1.0)
                        } else {
                            let tw = tex.width as f32;
                            let th = tex.height as f32;
                            (
                                src_x / tw,
                                src_y / th,
                                (src_x + src_w) / tw,
                                (src_y + src_h) / th,
                            )
                        }
                    } else {
                        (0.0, 0.0, 1.0, 1.0)
                    };
                    self.draw_list.push_sprite(
                        tex_id,
                        SpriteInstance {
                            dst_rect: [dst_x, dst_y, dst_w, dst_h],
                            src_rect: [u0, v0, u1, v1],
                            color: [r, g, b, a],
                            params: [
                                rotation,
                                if flip_x { 1.0 } else { 0.0 },
                                if flip_y { 1.0 } else { 0.0 },
                                0.0,
                            ],
                        },
                    );
                }
                // --- 3D commands ---
                DrawCommand::SetCamera3D {
                    eye,
                    target,
                    up,
                    fov,
                    near,
                    far,
                } => {
                    camera3d_state = Some((eye, target, up, fov, near, far));
                    let v = math3d::look_at_rh(eye, target, up);
                    let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                    let view_proj = math3d::mat4_mul(&proj, &v);
                    camera3d_uniform = Some(Camera3DUniform {
                        view_proj,
                        camera_pos: eye.to_array(),
                        _pad: 0.0,
                    });
                }
                DrawCommand::SetLights3D {
                    ambient_r,
                    ambient_g,
                    ambient_b,
                    ambient_ground_r,
                    ambient_ground_g,
                    ambient_ground_b,
                    lights,
                } => {
                    light_uniform.ambient = [ambient_r, ambient_g, ambient_b, 1.0];
                    light_uniform.ambient_ground =
                        [ambient_ground_r, ambient_ground_g, ambient_ground_b, 1.0];
                    let count = lights.len().min(8);
                    light_uniform.count[0] = count as u32;
                    for (i, l) in lights.iter().take(8).enumerate() {
                        let (v, w_type) = if l.light_type == 0 {
                            (l.direction, 0.0f32)
                        } else {
                            (l.position, 1.0f32)
                        };
                        light_uniform.lights[i] = LightData {
                            position_or_dir: [v.x, v.y, v.z, w_type],
                            color_intensity: [l.color.x, l.color.y, l.color.z, l.intensity],
                        };
                    }
                }
                DrawCommand::SetFog3D {
                    mode,
                    color,
                    start,
                    end,
                    density,
                } => {
                    light_uniform.count[1] = mode as u32;
                    light_uniform.fog_color = [color.x, color.y, color.z, 1.0];
                    light_uniform.fog_params = [start, end, density, 0.0];
                }
                DrawCommand::SetColorGrading3D {
                    tone_map,
                    exposure,
                    contrast,
                    saturation,
                } => {
                    light_uniform.color_params = [
                        exposure.max(0.0),
                        contrast.max(0.0),
                        saturation.max(0.0),
                        tone_map as f32,
                    ];
                }
                DrawCommand::SetShadow3D {
                    enabled,
                    resolution,
                    strength,
                } => {
                    shadow_enabled = enabled;
                    shadow_resolution = resolution.max(1);
                    shadow_strength = strength.clamp(0.0, 1.0);
                }
                DrawCommand::SetRenderDebug3D { mode } => {
                    light_uniform.debug_params[0] = mode.min(7) as u32;
                }
                DrawCommand::DrawSkybox { cubemap_id } => {
                    skybox_count += 1;
                    skybox_cubemap_id = Some(cubemap_id);
                }
                DrawCommand::DrawModel {
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    animation_world_id,
                    animation_target_id,
                } => {
                    model_command_count += 1;
                    let model_mat = math3d::model_matrix(pos, rot, scale);
                    let normal_mat = math3d::normal_matrix(&model_mat);
                    model_draws.push(ModelDraw {
                        model_id,
                        model_uniform: ModelUniform {
                            model: model_mat,
                            normal_matrix: normal_mat,
                            base_color: [1.0, 1.0, 1.0, 1.0],
                            material_params: [1.0, 1.0, 1.0, 1.0],
                            emissive_color: [0.0, 0.0, 0.0, 0.0],
                            texture_flags: [0.0, 0.0, 0.0, 0.0],
                        },
                        material,
                        animation_world_id,
                        animation_target_id,
                    });
                }
                DrawCommand::Scene3DUpsertObject {
                    scene_id,
                    object_id,
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    visible,
                    animation_world_id,
                    animation_target_id,
                } => {
                    scene_upsert_count += 1;
                    self.render_world.upsert_object(RenderObjectUpdate {
                        scene_id,
                        object_id,
                        model_id,
                        pos,
                        rot,
                        scale,
                        material,
                        visible,
                        animation_world_id,
                        animation_target_id,
                    });
                }
                DrawCommand::Scene3DDestroyObject {
                    scene_id,
                    object_id,
                } => {
                    self.render_world.destroy_object(scene_id, object_id);
                }
                DrawCommand::Scene3DClear { scene_id } => {
                    self.render_world.clear_scene(scene_id);
                    self.primitive_pipeline.clear_scene(scene_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, _, _), _| *shape_scene != scene_id);
                    self.primitive_materials
                        .retain(|(material_scene, _, _), _| *material_scene != scene_id);
                }
                DrawCommand::Scene3DDraw { scene_id } => {
                    scene_draw_count += 1;
                    retained_scene_draws.push(scene_id);
                }
                DrawCommand::Primitive3DUpsertInstance {
                    scene_id,
                    layer_id,
                    object_id,
                    model_id,
                    pos,
                    rot,
                    scale,
                    material,
                    visible,
                } => {
                    scene_upsert_count += 1;
                    let update = PrimitiveObjectUpdate {
                        scene_id,
                        layer_id,
                        object_id,
                        model_id,
                        pos,
                        rot,
                        scale,
                        material,
                        visible,
                    };
                    self.primitive_pipeline.upsert_instance(
                        &self.device,
                        &self.queue,
                        update,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world.upsert_primitive_instance(update);
                }
                DrawCommand::Primitive3DDestroyInstance {
                    scene_id,
                    layer_id,
                    object_id,
                } => {
                    self.primitive_pipeline.destroy_instance(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        object_id,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .destroy_primitive_instance(scene_id, layer_id, object_id);
                }
                DrawCommand::Primitive3DClearLayer { scene_id, layer_id } => {
                    self.primitive_pipeline.clear_layer(scene_id, layer_id);
                    self.render_world.clear_primitive_layer(scene_id, layer_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, shape_layer, _), _| {
                            *shape_scene != scene_id || *shape_layer != layer_id
                        });
                    self.primitive_materials
                        .retain(|(material_scene, material_layer, _), _| {
                            *material_scene != scene_id || *material_layer != layer_id
                        });
                }
                DrawCommand::Primitive3DDestroyLayer { scene_id, layer_id } => {
                    self.primitive_pipeline.clear_layer(scene_id, layer_id);
                    self.render_world
                        .destroy_primitive_layer(scene_id, layer_id);
                    self.primitive_shapes
                        .retain(|(shape_scene, shape_layer, _), _| {
                            *shape_scene != scene_id || *shape_layer != layer_id
                        });
                    self.primitive_materials
                        .retain(|(material_scene, material_layer, _), _| {
                            *material_scene != scene_id || *material_layer != layer_id
                        });
                }
                DrawCommand::Primitive3DReplaceChunk {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| PrimitiveObjectUpdate {
                            scene_id,
                            layer_id,
                            object_id: instance.object_id,
                            model_id: instance.model_id,
                            pos: instance.pos,
                            rot: instance.rot,
                            scale: instance.scale,
                            material: instance.material,
                            visible: instance.visible,
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DReplaceChunkRefs {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| {
                            let material = self
                                .primitive_materials
                                .get(&(scene_id, layer_id, instance.material_id))
                                .copied()
                                .unwrap_or_default();
                            PrimitiveObjectUpdate {
                                scene_id,
                                layer_id,
                                object_id: instance.object_id,
                                model_id: instance.model_id,
                                pos: instance.pos,
                                rot: instance.rot,
                                scale: instance.scale,
                                material,
                                visible: instance.visible,
                            }
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DReplaceChunkKeys {
                    scene_id,
                    layer_id,
                    chunk_id,
                    instances,
                } => {
                    scene_upsert_count += instances.len() as u32;
                    let updates: Vec<PrimitiveObjectUpdate> = instances
                        .into_iter()
                        .map(|instance| {
                            let model_id = self
                                .primitive_shapes
                                .get(&(scene_id, layer_id, instance.shape_id))
                                .copied()
                                .unwrap_or_default();
                            let material = self
                                .primitive_materials
                                .get(&(scene_id, layer_id, instance.material_id))
                                .copied()
                                .unwrap_or_default();
                            let mut material = material;
                            if instance.tint != [0.0, 0.0, 0.0, 0.0] {
                                material.base_color[0] *= instance.tint[0];
                                material.base_color[1] *= instance.tint[1];
                                material.base_color[2] *= instance.tint[2];
                                material.base_color[3] *= instance.tint[3];
                            }
                            PrimitiveObjectUpdate {
                                scene_id,
                                layer_id,
                                object_id: instance.object_id,
                                model_id,
                                pos: instance.pos,
                                rot: instance.rot,
                                scale: instance.scale,
                                material,
                                visible: instance.visible,
                            }
                        })
                        .collect();
                    self.primitive_pipeline.replace_chunk(
                        &self.device,
                        &self.queue,
                        scene_id,
                        layer_id,
                        chunk_id,
                        &updates,
                        &self.model_manager,
                        &self.texture_manager,
                    );
                    self.render_world
                        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
                }
                DrawCommand::Primitive3DUpsertMaterials {
                    scene_id,
                    layer_id,
                    materials,
                } => {
                    for material in materials {
                        self.primitive_materials.insert(
                            (scene_id, layer_id, material.material_id),
                            material.material,
                        );
                    }
                }
                DrawCommand::Primitive3DUpsertShapes {
                    scene_id,
                    layer_id,
                    shapes,
                } => {
                    for shape in shapes {
                        self.primitive_shapes
                            .insert((scene_id, layer_id, shape.shape_id), shape.model_id);
                    }
                }
                DrawCommand::Primitive3DSetChunkVisible {
                    scene_id,
                    layer_id,
                    chunk_id,
                    visible,
                } => {
                    self.render_world
                        .set_primitive_chunk_visible(scene_id, layer_id, chunk_id, visible);
                }
                DrawCommand::DrawBillboard {
                    tex_id,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    world_pos,
                    w: bw,
                    h: bh,
                    tint,
                } => {
                    // Project 3D world position to screen coordinates using the current 3D camera
                    if let Some(ref cam) = camera3d_uniform {
                        let clip = math3d::mat4_mul_vec4(
                            &cam.view_proj,
                            [world_pos.x, world_pos.y, world_pos.z, 1.0],
                        );
                        if clip[3] > 0.0 {
                            let ndc_x = clip[0] / clip[3];
                            let ndc_y = clip[1] / clip[3];
                            // NDC → screen: x = (ndc+1)/2 * w, y = (1-ndc)/2 * h
                            let screen_x = (ndc_x + 1.0) * 0.5 * w - bw * 0.5;
                            let screen_y = (1.0 - ndc_y) * 0.5 * h - bh * 0.5;

                            let (u0, v0, u1, v1) =
                                if let Some(tex) = self.texture_manager.get(tex_id) {
                                    if src_w == 0.0 && src_h == 0.0 {
                                        (0.0, 0.0, 1.0, 1.0)
                                    } else {
                                        let tw = tex.width as f32;
                                        let th = tex.height as f32;
                                        (
                                            src_x / tw,
                                            src_y / th,
                                            (src_x + src_w) / tw,
                                            (src_y + src_h) / th,
                                        )
                                    }
                                } else {
                                    (0.0, 0.0, 1.0, 1.0)
                                };
                            let color =
                                Self::fog_billboard_color(tint, world_pos, cam, &light_uniform);

                            self.draw_list.push_sprite(
                                tex_id,
                                SpriteInstance {
                                    dst_rect: [screen_x, screen_y, bw, bh],
                                    src_rect: [u0, v0, u1, v1],
                                    color,
                                    params: [0.0, 0.0, 0.0, 0.0],
                                },
                            );
                        }
                    }
                }
            }
        }
        for scene_id in retained_scene_draws {
            self.render_world
                .collect_scene_draws(scene_id, &mut model_draws);
            self.render_world.collect_scene_primitive_draws(
                scene_id,
                camera3d_uniform.as_ref(),
                &mut primitive_draws,
                &mut primitive_chunks,
            );
        }

        // Flush font atlas (re-upload if new glyphs were rasterized)
        self.font_manager
            .ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        // Resolve draw list: sort by (layer, order), produce draw calls
        let frame = self.draw_list.resolve();
        Self::debug_submit_status(
            debug_frame_count,
            &format!(
                "voplay submit #{} bytes={} cmds={} cam3d={} modelCmds={} sceneUpserts={} sceneDraws={} models={} primitives={} primitiveChunks={} skybox={} 2d(rect/circ/line/text/sprite)={}/{}/{}/{}/{} resolved(shapes/sprites/calls/cams)={}/{}/{}/{} clear={:.2},{:.2},{:.2}",
                debug_frame_count,
                data.len(),
                command_count,
                camera3d_uniform.is_some(),
                model_command_count,
                scene_upsert_count,
                scene_draw_count,
                model_draws.len(),
                primitive_draws.len(),
                primitive_chunks.len(),
                skybox_count,
                rect_count,
                circle_count,
                line_count,
                text_count,
                sprite_count,
                frame.shapes.len(),
                frame.sprites.len(),
                frame.draw_calls.len(),
                frame.cameras.len(),
                clear_color.r,
                clear_color.g,
                clear_color.b,
            ),
        );

        // Upload all camera uniforms into the dynamic offset buffer
        let align = self.camera_alignment;
        let cam_count = frame.cameras.len();
        if cam_count > self.camera_slot_capacity {
            let new_cap = cam_count.next_power_of_two();
            let (buf, bg) =
                Self::create_camera_buffer_and_bg(&self.device, &self.camera_bgl, new_cap, align);
            self.camera_buffer = buf;
            self.camera_bind_group = bg;
            self.camera_slot_capacity = new_cap;
        }
        for (i, cam) in frame.cameras.iter().enumerate() {
            let offset = i as u64 * align as u64;
            self.queue
                .write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(cam));
        }

        // Upload sorted 2D instance data
        self.pipeline2d
            .upload_instances(&self.device, &self.queue, &frame.shapes);
        self.pipeline_sprite
            .upload_instances(&self.device, &self.queue, &frame.sprites);

        let mut shadow_active = false;
        if shadow_enabled && !model_draws.is_empty() {
            if let Some(ref cam3d) = camera3d_uniform {
                if light_uniform.count[0] > 0 && light_uniform.lights[0].position_or_dir[3] == 0.0 {
                    let shadow_to_light = Vec3::new(
                        light_uniform.lights[0].position_or_dir[0],
                        light_uniform.lights[0].position_or_dir[1],
                        light_uniform.lights[0].position_or_dir[2],
                    );
                    let shadow_dir = (-shadow_to_light).normalize();
                    if shadow_dir.length() > 0.0 {
                        if self.pipeline_shadow.size() != shadow_resolution {
                            self.clear_texture_bind_group_caches();
                            self.pipeline_shadow.resize(&self.device, shadow_resolution);
                        }
                        let inv_view_proj =
                            math3d::mat4_inverse(&cam3d.view_proj).ok_or_else(|| {
                                "voplay: failed to invert camera view projection for shadow mapping"
                                    .to_string()
                            })?;
                        let shadow_vp = math3d::compute_shadow_vp(&inv_view_proj, shadow_dir);
                        self.pipeline_shadow.render_shadow_pass(
                            &self.device,
                            &mut encoder,
                            &self.queue,
                            &shadow_vp,
                            &model_draws,
                            &self.model_manager,
                        );
                        light_uniform.shadow_vp = shadow_vp;
                        light_uniform.shadow_params =
                            [1.0, 0.002, 1.0 / shadow_resolution as f32, shadow_strength];
                        light_uniform.count[2] = 0;
                        shadow_active = true;
                    }
                }
            }
        }
        if !shadow_active {
            light_uniform.shadow_vp = math3d::MAT4_IDENTITY;
            light_uniform.shadow_params =
                [0.0, 0.002, 1.0 / shadow_resolution as f32, shadow_strength];
        }

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

            if let (Some(cubemap_id), Some((eye, target, up, fov, near, far))) =
                (skybox_cubemap_id, camera3d_state)
            {
                if let Some(cubemap) = self.texture_manager.get_cubemap(cubemap_id) {
                    let view_rot = math3d::view_rotation_only(eye, target, up);
                    let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                    let vp = math3d::mat4_mul(&proj, &view_rot);
                    let inv_vp = math3d::mat4_inverse(&vp).unwrap_or(math3d::MAT4_IDENTITY);
                    self.pipeline_skybox.set_camera(&self.queue, &inv_vp);
                    self.pipeline_skybox.draw(&mut render_pass, cubemap);
                }
            }

            // Draw 3D models first (depth tested)
            if !model_draws.is_empty() {
                if let Some(ref cam3d) = camera3d_uniform {
                    self.pipeline3d
                        .set_camera_and_lights(&self.queue, cam3d, &light_uniform);
                    let shadow_view = self.pipeline_shadow.shadow_texture_view();
                    self.pipeline3d.draw_models(
                        &self.device,
                        &self.queue,
                        &mut render_pass,
                        &model_draws,
                        &self.model_manager,
                        &self.texture_manager,
                        shadow_view,
                    );
                }
            }

            if !primitive_draws.is_empty() || !primitive_chunks.is_empty() {
                if let Some(ref cam3d) = camera3d_uniform {
                    self.primitive_pipeline.set_camera_and_lights(
                        &self.queue,
                        cam3d,
                        &light_uniform,
                    );
                    let shadow_view = self.pipeline_shadow.shadow_texture_view();
                    self.primitive_pipeline.draw(
                        &self.device,
                        &self.queue,
                        &mut render_pass,
                        &primitive_draws,
                        &primitive_chunks,
                        &self.model_manager,
                        &self.texture_manager,
                        shadow_view,
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
                    DrawCallKind::Sprites {
                        texture_id,
                        start,
                        count,
                    } => {
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
        #[cfg(feature = "wasm")]
        if debug_scope_frame {
            let error_future = self.device.pop_error_scope();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(error) = error_future.await {
                    crate::externs::render::wasm_debug(&format!(
                        "voplay gpu validation #{}: {}",
                        debug_frame_count, error
                    ));
                }
            });
        }

        Ok(())
    }

    fn debug_submit_status(frame_count: u64, message: &str) {
        if Self::debug_should_log_frame(frame_count) {
            Self::debug_renderer_status(message);
        }
    }

    fn debug_should_log_frame(frame_count: u64) -> bool {
        if !(frame_count <= 4 || frame_count % 60 == 0) {
            return false;
        }
        #[cfg(feature = "wasm")]
        {
            return crate::externs::render::wasm_debug_enabled();
        }
        #[cfg(not(feature = "wasm"))]
        {
            false
        }
    }

    fn debug_renderer_status(message: &str) {
        #[cfg(feature = "wasm")]
        {
            crate::externs::render::wasm_debug(message);
        }
        #[cfg(not(feature = "wasm"))]
        {
            log::debug!("{}", message);
        }
    }

    #[cfg(feature = "wasm")]
    fn pop_debug_error_scope(device: &wgpu::Device, label: &'static str) {
        let error_future = device.pop_error_scope();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(error) = error_future.await {
                crate::externs::render::wasm_debug(&format!("{} validation: {}", label, error));
            }
        });
    }
}
