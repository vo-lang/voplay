//! wgpu-based renderer for voplay.
//! Manages device, surface, camera, and all rendering pipelines (shapes, sprites).

use std::collections::HashMap;

use crate::draw_list::{DrawCallKind, DrawList2D};
use crate::font_manager::FontManager;
use crate::math3d::{self, Vec3};
use crate::model_loader::{
    LevelNode, MeshMaterial, MeshVertex, ModelGeometryData, ModelId, ModelManager,
    TerrainMaterialTuning,
};
use crate::pipeline2d::{CameraUniform, Pipeline2D};
use crate::pipeline3d::{
    Camera3DUniform, LightData, LightUniform, ModelDraw, ModelUniform, Pipeline3D,
};
use crate::pipeline_depth::PipelineDepth;
use crate::pipeline_post::{
    PipelinePost, PostDecalGpu, PostDecalUniform, PostUniform, MAX_POST_DECAL_ATLASES,
};
use crate::pipeline_shadow::PipelineShadow;
use crate::pipeline_skybox::PipelineSkybox;
use crate::pipeline_sprite::{PipelineSprite, SpriteInstance};
use crate::primitive_pipeline::{PrimitiveDrawStats, PrimitivePipeline};
use crate::primitive_scene::{
    PrimitiveChunkBatchInfo, PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate,
};
use crate::render_world::{
    RenderBatchPlanner, RenderBatchQualityProfile, RenderObjectUpdate, RenderWorld,
};
use crate::renderer_frame::{
    FrameGraph, FrameGraphReport, RenderFramePipeline, RenderPassKind, RenderPassWorkload,
    RenderResourceRegistry, RES_CAPTURE, RES_DEPTH, RES_MAIN_COLOR, RES_OVERLAY, RES_POST_COLOR,
    RES_READBACK, RES_RECEIVER_MASK, RES_SHADOW_MAP, RES_SURFACE_COLOR, RES_SURFACE_PROPS,
    RES_WATER_COLOR,
};
use crate::renderer_perf::{
    elapsed_ms_opt, encode_renderer_perf_packet, perf_now, saturating_u32, RendererPerfOverrides,
    RendererPerfStats, RENDERER_DIAG_DISABLE_BLOOM, RENDERER_DIAG_DISABLE_CONTACT_AO,
    RENDERER_DIAG_DISABLE_DECALS, RENDERER_DIAG_DISABLE_FXAA, RENDERER_DIAG_DISABLE_POST_EFFECTS,
    RENDERER_DIAG_DISABLE_PRIMITIVES, RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS,
    RENDERER_DIAG_DISABLE_SHADOWS, RENDERER_DIAG_DISABLE_SHARPEN, RENDERER_PERF_PAYLOAD_VERSION,
};
use crate::renderer_targets::{MAIN_SAMPLE_COUNT, RECEIVER_MASK_FORMAT, SURFACE_PROPS_FORMAT};
use crate::stream::{DrawCommand, StreamReader};
use crate::terrain::TerrainData;
use crate::texture::{TextureId, TextureManager, TexturePixelsData};

mod backend_submit_pass;
mod depth_pass;
mod frame_decode;
mod frame_orchestrator;
mod frame_submit;
mod main_opaque_pass;
mod main_transparent_pass;
mod overlay_pass;
mod pass_dispatch;
mod post_pass;
mod shadow_pass;
mod water_pass;

/// Maximum number of camera states per frame before buffer regrow.
const INITIAL_CAMERA_SLOTS: usize = 16;
const DECAL_RECEIVER_ALL: u32 = 3;
#[cfg(feature = "wasm")]
const CANVAS_METRICS_CHECK_INTERVAL_MS: f64 = 250.0;

fn shadow_cascade_count_for_quality(quality: u32) -> usize {
    match quality {
        4.. => 4,
        3 => 3,
        _ => 1,
    }
}

fn shadow_atlas_resolution(resolution: u32, cascade_count: usize) -> u32 {
    let resolution = resolution.max(1);
    if cascade_count <= 1 {
        return resolution;
    }
    let even = if resolution % 2 == 0 {
        resolution
    } else {
        resolution + 1
    };
    even.max(2)
}

fn compute_shadow_cascade_splits(near: f32, far: f32, cascade_count: usize) -> [f32; 4] {
    let near = near.max(0.01);
    let far = far.max(near + 0.1);
    let count = cascade_count.clamp(1, 4);
    let lambda = 0.58;
    let mut splits = [far; 4];
    for index in 0..count {
        let p = (index + 1) as f32 / count as f32;
        let uniform = near + (far - near) * p;
        let logarithmic = near * (far / near).powf(p);
        splits[index] = uniform * (1.0 - lambda) + logarithmic * lambda;
    }
    splits[count - 1] = far;
    splits
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectedDecalAtlasBinding {
    albedo_id: u32,
    normal_id: u32,
    roughness_id: u32,
    mask_id: u32,
}

fn raw_mesh_read_u32(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    if data.len().saturating_sub(*pos) < 4 {
        return Err("raw mesh payload ended before u32".to_string());
    }
    let value = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(value)
}

fn raw_mesh_read_f32(data: &[u8], pos: &mut usize) -> Result<f32, String> {
    if data.len().saturating_sub(*pos) < 4 {
        return Err("raw mesh payload ended before f32".to_string());
    }
    let value = f32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(value)
}

fn decode_raw_mesh(data: &[u8]) -> Result<(Vec<MeshVertex>, Vec<u32>, [f32; 4]), String> {
    let mut pos = 0usize;
    let version = raw_mesh_read_u32(data, &mut pos)?;
    if version != 1 {
        return Err(format!("raw mesh version {version} is not supported"));
    }
    let vertex_count = raw_mesh_read_u32(data, &mut pos)? as usize;
    let index_count = raw_mesh_read_u32(data, &mut pos)? as usize;
    if vertex_count == 0 || index_count == 0 {
        return Err("raw mesh must contain vertices and indices".to_string());
    }
    if index_count % 3 != 0 {
        return Err("raw mesh index count must be divisible by 3".to_string());
    }
    let base_color = [
        raw_mesh_read_f32(data, &mut pos)?,
        raw_mesh_read_f32(data, &mut pos)?,
        raw_mesh_read_f32(data, &mut pos)?,
        raw_mesh_read_f32(data, &mut pos)?,
    ];
    let expected_len = 28usize
        .saturating_add(vertex_count.saturating_mul(48))
        .saturating_add(index_count.saturating_mul(4));
    if data.len() != expected_len {
        return Err(format!(
            "raw mesh payload length mismatch: got {}, expected {}",
            data.len(),
            expected_len
        ));
    }
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        positions.push([
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
        ]);
        normals.push([
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
        ]);
        uvs.push([
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
        ]);
        colors.push([
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
            raw_mesh_read_f32(data, &mut pos)?,
        ]);
    }
    let mut indices = Vec::with_capacity(index_count);
    for _ in 0..index_count {
        let index = raw_mesh_read_u32(data, &mut pos)?;
        if index as usize >= vertex_count {
            return Err("raw mesh index is out of bounds".to_string());
        }
        indices.push(index);
    }
    let tangents = ModelManager::generate_tangents(&positions, &normals, &uvs, &indices);
    let vertices = positions
        .iter()
        .enumerate()
        .map(|(index, position)| MeshVertex {
            position: *position,
            normal: normals[index],
            uv: uvs[index],
            tangent: tangents[index],
            color: colors[index],
        })
        .collect();
    Ok((vertices, indices, base_color))
}

/// Renderer holds all wgpu state and rendering pipelines.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    resources: RenderResourceRegistry,
    post_uniform_buffer: wgpu::Buffer,
    post_decal_uniform_buffer: wgpu::Buffer,
    post_bind_group: Option<wgpu::BindGroup>,
    screen_width: f32,
    screen_height: f32,
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
    pipeline_depth: PipelineDepth,
    pipeline_shadow: PipelineShadow,
    pipeline_skybox: PipelineSkybox,
    pipeline_post: PipelinePost,
    model_manager: ModelManager,
    // Texture manager
    texture_manager: TextureManager,
    // Font manager for text rendering (TTF/OTF, dynamic glyph atlas)
    font_manager: FontManager,
    render_world: RenderWorld,
    primitive_shapes: HashMap<(u32, u32, u32), u32>,
    primitive_materials: HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
    #[cfg(feature = "wasm")]
    canvas_metrics_last_check_ms: f64,
    debug_frame_count: u64,
    last_perf_packet: Vec<u8>,
    last_frame_graph_report: FrameGraphReport,
    last_frame_pipeline: RenderFramePipeline,
    perf_stats_enabled: bool,
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
        let resources = RenderResourceRegistry::new_render_targets(
            &device,
            surface_config.width,
            surface_config.height,
            surface_config.format,
        );
        let screen_width = surface_config.width as f32;
        let screen_height = surface_config.height as f32;

        let camera_bgl = Self::create_camera_bgl(&device);
        let camera_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let (camera_buffer, camera_bind_group) = Self::create_camera_buffer_and_bg(
            &device,
            &camera_bgl,
            INITIAL_CAMERA_SLOTS,
            camera_alignment,
        );
        let mut texture_manager = TextureManager::new(&device);
        let pipeline2d =
            Pipeline2D::new_overlay(&device, &queue, surface_config.format, &camera_bgl);
        let tex_bgl = texture_manager.bind_group_layout();
        let cubemap_bgl = texture_manager.cubemap_bind_group_layout();
        let pipeline_sprite = PipelineSprite::new_overlay(
            &device,
            &queue,
            surface_config.format,
            &camera_bgl,
            tex_bgl,
        );
        #[cfg(feature = "wasm")]
        let debug_pipeline_errors = crate::externs::render::wasm_debug_enabled();
        #[cfg(feature = "wasm")]
        if debug_pipeline_errors {
            device.push_error_scope(wgpu::ErrorFilter::Validation);
        }
        let pipeline3d = Pipeline3D::new(
            &device,
            &queue,
            surface_config.format,
            RECEIVER_MASK_FORMAT,
            SURFACE_PROPS_FORMAT,
            MAIN_SAMPLE_COUNT,
        );
        let primitive_pipeline = PrimitivePipeline::new(
            &device,
            &queue,
            surface_config.format,
            RECEIVER_MASK_FORMAT,
            SURFACE_PROPS_FORMAT,
            MAIN_SAMPLE_COUNT,
        );
        #[cfg(feature = "wasm")]
        if debug_pipeline_errors {
            Self::pop_debug_error_scope(&device, "voplay pipeline3d create");
        }
        let pipeline_depth =
            PipelineDepth::new(&device, surface_config.width, surface_config.height);
        let pipeline_shadow = PipelineShadow::new(&device, 2048);
        let pipeline_skybox = PipelineSkybox::new(
            &device,
            surface_config.format,
            RECEIVER_MASK_FORMAT,
            SURFACE_PROPS_FORMAT,
            cubemap_bgl,
            MAIN_SAMPLE_COUNT,
        );
        let pipeline_post = PipelinePost::new(&device, &queue, surface_config.format);
        let post_uniform_buffer = PipelinePost::create_uniform_buffer(&device);
        let post_decal_uniform_buffer = PipelinePost::create_decal_uniform_buffer(&device);
        queue.write_buffer(
            &post_uniform_buffer,
            0,
            bytemuck::bytes_of(&PostUniform::for_size(
                surface_config.width,
                surface_config.height,
            )),
        );
        queue.write_buffer(
            &post_decal_uniform_buffer,
            0,
            bytemuck::bytes_of(&PostDecalUniform::empty()),
        );
        let fallback_decal_atlas = pipeline_post.decal_fallback_view();
        let fallback_decal_normal_atlas = pipeline_post.decal_normal_fallback_view();
        let fallback_decal_roughness_atlas = pipeline_post.decal_roughness_fallback_view();
        let fallback_decal_mask_atlas = pipeline_post.decal_mask_fallback_view();
        let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
            pipeline_depth.depth_texture_view()
        } else {
            resources
                .depth_view()
                .ok_or_else(|| "voplay: missing depth target during renderer build".to_string())?
        };
        let post_color_view = resources
            .post_color_view()
            .ok_or_else(|| "voplay: missing post target during renderer build".to_string())?;
        let receiver_mask_view = resources.receiver_mask_view().ok_or_else(|| {
            "voplay: missing receiver mask target during renderer build".to_string()
        })?;
        let surface_props_view = resources.surface_props_view().ok_or_else(|| {
            "voplay: missing surface props target during renderer build".to_string()
        })?;
        let post_bind_group = pipeline_post.create_bind_group(
            &device,
            post_color_view,
            post_depth_view,
            &post_uniform_buffer,
            &post_decal_uniform_buffer,
            [fallback_decal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES],
            receiver_mask_view,
            surface_props_view,
        );
        let model_manager = ModelManager::new();
        let mut font_manager = FontManager::new()?;
        font_manager.ensure_atlas(&mut texture_manager, &device, &queue);
        let draw_list = DrawList2D::new(screen_width, screen_height);
        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            resources,
            post_uniform_buffer,
            post_decal_uniform_buffer,
            post_bind_group: Some(post_bind_group),
            screen_width,
            screen_height,
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
            pipeline_depth,
            pipeline_shadow,
            pipeline_skybox,
            pipeline_post,
            model_manager,
            texture_manager,
            font_manager,
            render_world: RenderWorld::new(),
            primitive_shapes: HashMap::new(),
            primitive_materials: HashMap::new(),
            #[cfg(feature = "wasm")]
            canvas_metrics_last_check_ms: 0.0,
            debug_frame_count: 0,
            last_perf_packet: Vec::new(),
            last_frame_graph_report: FrameGraphReport::default(),
            last_frame_pipeline: RenderFramePipeline::default(),
            perf_stats_enabled: false,
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
        #[cfg(feature = "wasm")]
        self.update_canvas_metrics_forced();
    }

    fn set_logical_screen_size(&mut self, width: f32, height: f32) {
        if width > 0.0 && height > 0.0 {
            self.screen_width = width;
            self.screen_height = height;
        }
    }

    /// Resize the surface and depth buffer.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        #[cfg(not(feature = "wasm"))]
        self.set_logical_screen_size(width as f32, height as f32);
        if self.surface_config.width == width && self.surface_config.height == height {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        eprintln!("voplay: renderer resize {}x{}", width, height);
        self.surface.configure(&self.device, &self.surface_config);
        self.resources.recreate_render_targets(
            &self.device,
            width,
            height,
            self.surface_config.format,
        );
        if MAIN_SAMPLE_COUNT > 1 {
            self.pipeline_depth.resize(&self.device, width, height);
        }
        let fallback_decal_atlas = self.pipeline_post.decal_fallback_view();
        let fallback_decal_normal_atlas = self.pipeline_post.decal_normal_fallback_view();
        let fallback_decal_roughness_atlas = self.pipeline_post.decal_roughness_fallback_view();
        let fallback_decal_mask_atlas = self.pipeline_post.decal_mask_fallback_view();
        let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
            self.pipeline_depth.depth_texture_view()
        } else {
            self.resources
                .depth_view()
                .expect("voplay: missing depth target after resize")
        };
        let post_color_view = self
            .resources
            .post_color_view()
            .expect("voplay: missing post target after resize");
        let receiver_mask_view = self
            .resources
            .receiver_mask_view()
            .expect("voplay: missing receiver mask target after resize");
        let surface_props_view = self
            .resources
            .surface_props_view()
            .expect("voplay: missing surface props target after resize");
        let post_bind_group = self.pipeline_post.create_bind_group(
            &self.device,
            post_color_view,
            post_depth_view,
            &self.post_uniform_buffer,
            &self.post_decal_uniform_buffer,
            [fallback_decal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES],
            receiver_mask_view,
            surface_props_view,
        );
        self.post_bind_group = Some(post_bind_group);
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

    pub fn create_raw_mesh(&mut self, data: &[u8]) -> Result<ModelId, String> {
        let (vertices, indices, base_color) = decode_raw_mesh(data)?;
        Ok(self.model_manager.create_raw(
            &self.device,
            &self.queue,
            &vertices,
            &indices,
            base_color,
        ))
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
        terrain_tuning: TerrainMaterialTuning,
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
        let terrain_tuning = terrain_tuning.normalized()?;
        let material = MeshMaterial::terrain_splat(
            [1.0, 1.0, 1.0, 1.0],
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            layer_normal_scales,
            terrain_tuning,
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

    pub fn create_terrain_splat_model(
        &mut self,
        source_model_id: ModelId,
        control_texture_id: TextureId,
        layer_texture_ids: [TextureId; 4],
        layer_normal_texture_ids: [TextureId; 4],
        layer_metallic_roughness_texture_ids: [TextureId; 4],
        uv_scales: [f32; 4],
        layer_normal_scales: [f32; 4],
        terrain_tuning: TerrainMaterialTuning,
    ) -> Result<ModelId, String> {
        if uv_scales
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(format!(
                "terrain splat model uv scales must be finite and > 0, got {:?}",
                uv_scales
            ));
        }
        if layer_normal_scales
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(format!(
                "terrain splat model normal scales must be finite and >= 0, got {:?}",
                layer_normal_scales
            ));
        }
        let terrain_tuning = terrain_tuning.normalized()?;
        let material = MeshMaterial::terrain_splat(
            [1.0, 1.0, 1.0, 1.0],
            control_texture_id,
            layer_texture_ids,
            layer_normal_texture_ids,
            layer_metallic_roughness_texture_ids,
            uv_scales,
            layer_normal_scales,
            terrain_tuning,
        );
        self.model_manager
            .create_material_variant(source_model_id, material)
    }

    /// Free a loaded model by ID.
    pub fn free_model(&mut self, id: ModelId) {
        self.model_manager.free(id);
    }

    pub fn model_bounds(&self, model_id: ModelId) -> Option<([f32; 3], [f32; 3])> {
        let model = self.model_manager.get(model_id)?;
        Some((model.aabb_min, model.aabb_max))
    }

    pub fn get_model_geometry(&self, model_id: ModelId) -> Option<ModelGeometryData> {
        self.model_manager.geometry_data(model_id)
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

    pub fn load_texture_linear(&mut self, path: &str) -> Result<TextureId, String> {
        self.texture_manager
            .load_file_with_srgb(&self.device, &self.queue, path, false)
    }

    /// Load a texture from encoded image bytes (PNG, JPEG, etc.).
    pub fn load_texture_bytes(&mut self, data: &[u8]) -> Result<TextureId, String> {
        self.texture_manager
            .load_image_bytes(&self.device, &self.queue, data)
    }

    pub fn load_texture_rgba(
        &mut self,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<TextureId, String> {
        self.load_texture_rgba_with_srgb(width, height, data, true)
    }

    pub fn load_texture_rgba_linear(
        &mut self,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<TextureId, String> {
        self.load_texture_rgba_with_srgb(width, height, data, false)
    }

    pub fn load_texture_rgba_with_srgb(
        &mut self,
        width: u32,
        height: u32,
        data: &[u8],
        srgb: bool,
    ) -> Result<TextureId, String> {
        if width == 0 || height == 0 {
            return Err("load_texture_rgba: width and height must be > 0".to_string());
        }
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return Err(format!(
                "load_texture_rgba: expected {} RGBA bytes, got {}",
                expected,
                data.len()
            ));
        }
        Ok(self.texture_manager.load_rgba_with_srgb(
            &self.device,
            &self.queue,
            width,
            height,
            data,
            srgb,
        ))
    }

    pub fn load_texture_bytes_linear(&mut self, data: &[u8]) -> Result<TextureId, String> {
        self.texture_manager
            .load_image_bytes_with_srgb(&self.device, &self.queue, data, false)
    }

    pub fn texture_pixels(&self, id: TextureId) -> Option<TexturePixelsData> {
        self.texture_manager.pixels(id)
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
    /// The game/UI coordinate system stays in CSS pixels so input, HUD, and
    /// touch controls share one logical screen. The backing buffer uses the
    /// effective render DPR published by the widget so large Retina canvases
    /// can stay inside the engine pixel budget without changing game layout.
    #[cfg(feature = "wasm")]
    fn update_canvas_metrics(&mut self) {
        self.update_canvas_metrics_with_policy(false);
    }

    #[cfg(feature = "wasm")]
    fn update_canvas_metrics_forced(&mut self) {
        self.update_canvas_metrics_with_policy(true);
    }

    #[cfg(feature = "wasm")]
    fn update_canvas_metrics_with_policy(&mut self, force: bool) {
        use wasm_bindgen::JsCast;
        let now_ms = js_sys::Date::now();
        if !force
            && self.canvas_metrics_last_check_ms > 0.0
            && now_ms - self.canvas_metrics_last_check_ms < CANVAS_METRICS_CHECK_INTERVAL_MS
        {
            return;
        }
        self.canvas_metrics_last_check_ms = now_ms;
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
        let native_dpr = window.device_pixel_ratio();
        let dpr = js_sys::Reflect::get(
            window.as_ref(),
            &wasm_bindgen::JsValue::from_str("__voplayRenderDevicePixelRatio"),
        )
        .ok()
        .and_then(|value| value.as_f64())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(native_dpr)
        .min(native_dpr);
        let min_dpr = if native_dpr >= 1.0 { 1.0 } else { native_dpr };
        let dpr = dpr.max(min_dpr);
        let css_w = canvas.client_width().max(1) as f64;
        let css_h = canvas.client_height().max(1) as f64;
        self.set_logical_screen_size(css_w as f32, css_h as f32);
        let w = (css_w * dpr) as u32;
        let h = (css_h * dpr) as u32;
        // Detect backing buffer mismatch (happens after DOM reparent or external resize).
        let backing_w = canvas.width();
        let backing_h = canvas.height();
        let size_changed = w != self.surface_config.width || h != self.surface_config.height;
        let backing_dirty = backing_w != w || backing_h != h;
        if size_changed || backing_dirty {
            eprintln!(
                "voplay: update_canvas_metrics backing {}x{} (was {}x{}, canvas {}x{})",
                w, h, self.surface_config.width, self.surface_config.height, backing_w, backing_h
            );
            canvas.set_width(w);
            canvas.set_height(h);
            self.resize(w, h);
        }
    }

    /// Execute a frame's draw command stream.
    pub fn submit_frame(&mut self, data: &[u8]) -> Result<(), String> {
        self.submit_frame_inner(data)
    }

    pub fn last_perf_packet(&self) -> &[u8] {
        &self.last_perf_packet
    }

    pub(crate) fn update_last_perf_packet(
        &mut self,
        perf_enabled: bool,
        perf: &RendererPerfStats,
    ) -> f64 {
        if !perf_enabled {
            self.last_perf_packet.clear();
            return 0.0;
        }
        let perf_packet_start = Some(perf_now());
        self.last_perf_packet = encode_renderer_perf_packet(perf);
        elapsed_ms_opt(perf_packet_start)
    }

    pub(crate) fn last_frame_graph_report(&self) -> &FrameGraphReport {
        &self.last_frame_graph_report
    }

    pub(crate) fn last_frame_pipeline(&self) -> &RenderFramePipeline {
        &self.last_frame_pipeline
    }

    pub fn set_perf_stats_enabled(&mut self, enabled: bool) {
        self.perf_stats_enabled = enabled;
        if !enabled {
            self.last_perf_packet.clear();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_graph_report_records_real_pass_schedule() {
        let mut graph = FrameGraph::single_view(11, 0);
        graph.declare_target(RES_SURFACE_COLOR, true);
        graph.declare_target(RES_MAIN_COLOR, true);
        graph.declare_target(RES_DEPTH, true);
        graph.declare_target(RES_SHADOW_MAP, true);
        graph.plan_pass(RenderPassKind::DepthPrepass, &[], &[RES_DEPTH], false);
        graph.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            true,
        );
        graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_SHADOW_MAP],
            &[RES_MAIN_COLOR, RES_DEPTH],
            true,
        );
        graph.plan_pass(
            RenderPassKind::BackendSubmit,
            &[RES_MAIN_COLOR],
            &[RES_SURFACE_COLOR],
            true,
        );
        assert!(!graph.has_pass(RenderPassKind::DepthPrepass));
        graph.record_pass(RenderPassKind::DepthPrepass, 9.0);
        graph.record_pass(RenderPassKind::Shadow, 1.0);
        graph.record_pass(RenderPassKind::MainOpaque, 3.0);
        graph.record_pass(RenderPassKind::BackendSubmit, 0.6);
        let report = graph.report();
        assert_eq!(report.frame_id, 11);
        assert_eq!(report.pass_count, 3);
        assert_eq!(report.slowest_pass, "main-opaque");
        assert_eq!(report.target_count, 4);
        assert!(report.resource_count >= 4);
    }
}
