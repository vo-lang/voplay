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
use crate::primitive_pipeline::PrimitivePipeline;
use crate::primitive_scene::{PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate};
use crate::render_world::{RenderObjectUpdate, RenderWorld};
use crate::renderer_frame::{
    FrameGraph, FrameGraphReport, RenderPassKind, RES_DEPTH, RES_MAIN_COLOR, RES_OVERLAY,
    RES_POST_COLOR, RES_RECEIVER_MASK, RES_SHADOW_MAP, RES_SURFACE_COLOR, RES_SURFACE_PROPS,
    RES_WATER_COLOR,
};
use crate::renderer_perf::{
    elapsed_ms_opt, encode_renderer_perf_packet, perf_now, saturating_u32, RendererPerfOverrides,
    RendererPerfStats, RENDERER_DIAG_DISABLE_BLOOM, RENDERER_DIAG_DISABLE_CONTACT_AO,
    RENDERER_DIAG_DISABLE_DECALS, RENDERER_DIAG_DISABLE_FXAA, RENDERER_DIAG_DISABLE_POST_EFFECTS,
    RENDERER_DIAG_DISABLE_PRIMITIVES, RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS,
    RENDERER_DIAG_DISABLE_SHADOWS, RENDERER_DIAG_DISABLE_SHARPEN,
};
use crate::renderer_targets::{
    create_depth_view, create_msaa_color_view, create_msaa_receiver_mask_view,
    create_msaa_surface_props_view, create_post_color_view, create_receiver_mask_view,
    create_surface_props_view, MAIN_SAMPLE_COUNT, RECEIVER_MASK_FORMAT, SURFACE_PROPS_FORMAT,
};
use crate::stream::{DrawCommand, StreamReader};
use crate::terrain::TerrainData;
use crate::texture::{TextureId, TextureManager, TexturePixelsData};

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
    depth_view: Option<wgpu::TextureView>,
    msaa_color_view: Option<wgpu::TextureView>,
    post_color_view: Option<wgpu::TextureView>,
    msaa_receiver_mask_view: Option<wgpu::TextureView>,
    receiver_mask_view: Option<wgpu::TextureView>,
    msaa_surface_props_view: Option<wgpu::TextureView>,
    surface_props_view: Option<wgpu::TextureView>,
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
        let depth_view = create_depth_view(
            &device,
            surface_config.width,
            surface_config.height,
            MAIN_SAMPLE_COUNT,
        );
        let msaa_color_view = create_msaa_color_view(
            &device,
            surface_config.width,
            surface_config.height,
            surface_config.format,
            MAIN_SAMPLE_COUNT,
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
        let post_color_view = create_post_color_view(
            &device,
            surface_config.width,
            surface_config.height,
            surface_config.format,
        );
        let receiver_mask_view = create_receiver_mask_view(
            &device,
            surface_config.width,
            surface_config.height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_receiver_mask",
        );
        let msaa_receiver_mask_view = create_msaa_receiver_mask_view(
            &device,
            surface_config.width,
            surface_config.height,
            MAIN_SAMPLE_COUNT,
        );
        let surface_props_view = create_surface_props_view(
            &device,
            surface_config.width,
            surface_config.height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_surface_props",
        );
        let msaa_surface_props_view = create_msaa_surface_props_view(
            &device,
            surface_config.width,
            surface_config.height,
            MAIN_SAMPLE_COUNT,
        );
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
            &depth_view
        };
        let post_bind_group = pipeline_post.create_bind_group(
            &device,
            &post_color_view,
            post_depth_view,
            &post_uniform_buffer,
            &post_decal_uniform_buffer,
            [fallback_decal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES],
            &receiver_mask_view,
            &surface_props_view,
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
            depth_view: Some(depth_view),
            msaa_color_view,
            post_color_view: Some(post_color_view),
            msaa_receiver_mask_view,
            receiver_mask_view: Some(receiver_mask_view),
            msaa_surface_props_view,
            surface_props_view: Some(surface_props_view),
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
        self.depth_view = Some(create_depth_view(
            &self.device,
            width,
            height,
            MAIN_SAMPLE_COUNT,
        ));
        self.msaa_color_view = create_msaa_color_view(
            &self.device,
            width,
            height,
            self.surface_config.format,
            MAIN_SAMPLE_COUNT,
        );
        let post_color_view =
            create_post_color_view(&self.device, width, height, self.surface_config.format);
        let receiver_mask_view = create_receiver_mask_view(
            &self.device,
            width,
            height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_receiver_mask",
        );
        self.msaa_receiver_mask_view =
            create_msaa_receiver_mask_view(&self.device, width, height, MAIN_SAMPLE_COUNT);
        let surface_props_view = create_surface_props_view(
            &self.device,
            width,
            height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_surface_props",
        );
        self.msaa_surface_props_view =
            create_msaa_surface_props_view(&self.device, width, height, MAIN_SAMPLE_COUNT);
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
            self.depth_view
                .as_ref()
                .expect("voplay: missing depth target after resize")
        };
        let post_bind_group = self.pipeline_post.create_bind_group(
            &self.device,
            &post_color_view,
            post_depth_view,
            &self.post_uniform_buffer,
            &self.post_decal_uniform_buffer,
            [fallback_decal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES],
            [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES],
            &receiver_mask_view,
            &surface_props_view,
        );
        self.post_color_view = Some(post_color_view);
        self.receiver_mask_view = Some(receiver_mask_view);
        self.surface_props_view = Some(surface_props_view);
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
        let perf_enabled = self.perf_stats_enabled;
        let perf_overrides = RendererPerfOverrides::current();
        let frame_start = if perf_enabled { Some(perf_now()) } else { None };
        self.debug_frame_count = self.debug_frame_count.wrapping_add(1);
        let debug_frame_count = self.debug_frame_count;
        let mut perf = if perf_enabled {
            RendererPerfStats {
                frame_id: debug_frame_count.min(u32::MAX as u64) as u32,
                display_tick: debug_frame_count.min(u32::MAX as u64) as u32,
                ..RendererPerfStats::default()
            }
        } else {
            RendererPerfStats::default()
        };
        if perf_enabled {
            perf.diagnostic_flags = perf_overrides.flags();
        }
        #[cfg(feature = "wasm")]
        let debug_scope_frame = Self::debug_should_log_frame(debug_frame_count);

        #[cfg(feature = "wasm")]
        self.update_canvas_metrics();
        #[cfg(feature = "wasm")]
        if debug_scope_frame {
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        }

        let acquire_start = if perf_enabled { Some(perf_now()) } else { None };
        let output = self
            .surface
            .get_current_texture()
            .map_err(|e| format!("voplay: get_current_texture: {}", e))?;
        perf.surface_acquire_ms = elapsed_ms_opt(acquire_start);
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay_frame"),
            });

        let screen_w = self.screen_width;
        let screen_h = self.screen_height;

        // Reset draw list for this frame
        let mut clear_color = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        self.draw_list.clear();
        self.draw_list.set_screen_space(screen_w, screen_h);

        // 3D state for this frame
        let mut camera3d_uniform: Option<Camera3DUniform> = None;
        let mut camera3d_state: Option<(Vec3, Vec3, Vec3, f32, f32, f32)> = None;
        let mut skybox_cubemap_id: Option<u32> = None;
        let mut shadow_enabled = false;
        let mut shadow_resolution = 2048u32;
        let mut shadow_strength = 1.0f32;
        let mut shadow_softness = 1.0f32;
        let mut shadow_distance = 0.0f32;
        let mut shadow_fade = 0.0f32;
        let mut shadow_quality = 3u32;
        let mut post_bloom_threshold = 0.74f32;
        let mut post_bloom_strength = 0.105f32;
        let mut post_sharpen_strength = 0.055f32;
        let mut post_fxaa_strength = 0.82f32;
        let mut post_contact_ao_strength = 0.0f32;
        let mut post_contact_ao_radius = 2.5f32;
        let mut post_contact_ao_depth_scale = 70.0f32;
        let mut post_contact_ao_detail_strength = 0.18f32;
        let mut post_contact_ao_detail_radius = 0.95f32;
        let mut post_contact_ao_normal_bias = 0.015f32;
        let mut post_contact_ao_quality = 2u32;
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
            shadow_cascade_vp: [math3d::MAT4_IDENTITY; 4],
            shadow_cascade_splits: [0.0; 4],
            shadow_params: [0.0, 0.002, 1.0, 1.0],
            shadow_params2: [0.0, 0.0, 0.0, 0.0],
            color_params: [1.0, 1.0, 1.0, 0.0],
            debug_params: [0, debug_frame_count as u32, 0, 0],
        };
        let mut model_draws: Vec<ModelDraw> = Vec::new();
        let mut primitive_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_depth_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_shadow_draws: Vec<PrimitiveDraw> = Vec::new();
        let mut primitive_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_depth_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_shadow_chunks: Vec<PrimitiveChunkRef> = Vec::new();
        let mut primitive_main_draw_calls = 0u32;
        let mut primitive_depth_draw_calls = 0u32;
        let mut primitive_shadow_draw_calls = 0u32;
        let mut primitive_main_submitted = false;
        let mut projected_decals: Vec<PostDecalGpu> = Vec::new();
        let mut projected_decal_atlas_bindings: Vec<ProjectedDecalAtlasBinding> = Vec::new();
        let mut current_projected_decal_atlas_id: Option<u32> = None;
        let mut current_projected_decal_normal_atlas_id: Option<u32> = None;
        let mut current_projected_decal_roughness_atlas_id: Option<u32> = None;
        let mut current_projected_decal_mask_atlas_id: Option<u32> = None;
        let mut current_projected_decal_fade = [0.0f32, 0.0f32];
        let mut current_projected_decal_angle_fade = [0.0f32, 0.0f32];
        let mut current_projected_decal_receivers = DECAL_RECEIVER_ALL;
        let mut current_projected_decal_surface = [0.0f32, 0.72f32, 0.0f32];
        let mut retained_scene_draws: Vec<u32> = Vec::new();
        let mut command_count = 0u32;
        let mut rect_count = 0u32;
        let mut circle_count = 0u32;
        let mut line_count = 0u32;
        let mut text_count = 0u32;
        let mut sprite_count = 0u32;
        let mut model_command_count = 0u32;
        let mut projected_decal_count = 0u32;
        let mut scene_upsert_count = 0u32;
        let mut scene_removal_count = 0u32;
        let mut scene_draw_count = 0u32;
        let mut skybox_count = 0u32;
        let mut resident_chunk_rebuild_count = 0u32;
        let aspect = screen_w / screen_h;

        // Decode command stream into the unified draw list
        let decode_start = if perf_enabled { Some(perf_now()) } else { None };
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
                    self.draw_list
                        .set_camera_2d(screen_w, screen_h, x, y, zoom, rotation);
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
                    softness,
                    distance,
                    fade,
                    quality,
                } => {
                    shadow_quality = quality.min(4);
                    shadow_enabled = enabled && shadow_quality > 0;
                    shadow_resolution = resolution.max(1);
                    shadow_strength = strength.clamp(0.0, 1.0);
                    shadow_softness = softness.clamp(0.5, 4.0);
                    shadow_distance = distance.max(0.0);
                    shadow_fade = fade.max(0.0);
                }
                DrawCommand::SetRenderDebug3D { mode } => {
                    light_uniform.debug_params[0] = mode.min(12) as u32;
                }
                DrawCommand::SetPostProcess3D {
                    bloom_threshold,
                    bloom_strength,
                    sharpen_strength,
                    fxaa_strength,
                } => {
                    post_bloom_threshold = bloom_threshold.clamp(0.0, 1.0);
                    post_bloom_strength = bloom_strength.clamp(0.0, 2.0);
                    post_sharpen_strength = sharpen_strength.clamp(0.0, 1.0);
                    post_fxaa_strength = fxaa_strength.clamp(0.0, 1.5);
                }
                DrawCommand::SetContactAO3D {
                    strength,
                    radius,
                    depth_scale,
                    detail_strength,
                    detail_radius,
                    normal_bias,
                    quality,
                } => {
                    post_contact_ao_strength = strength.clamp(0.0, 1.5);
                    post_contact_ao_radius = radius.clamp(0.5, 8.0);
                    post_contact_ao_depth_scale = depth_scale.clamp(1.0, 400.0);
                    post_contact_ao_detail_strength = detail_strength.clamp(0.0, 1.0);
                    post_contact_ao_detail_radius = detail_radius.clamp(0.35, 3.0);
                    post_contact_ao_normal_bias = normal_bias.clamp(0.0, 0.08);
                    post_contact_ao_quality = quality.min(4);
                }
                DrawCommand::DrawSkybox { cubemap_id } => {
                    skybox_count += 1;
                    skybox_cubemap_id = Some(cubemap_id);
                }
                DrawCommand::DrawProjectedDecal3D {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                } => {
                    projected_decal_count += 1;
                    projected_decals.push(
                        PostDecalGpu::new(position.to_array(), yaw, width, length, depth, color)
                            .with_distance_fade(
                                current_projected_decal_fade[0],
                                current_projected_decal_fade[1],
                            )
                            .with_angle_fade(
                                current_projected_decal_angle_fade[0],
                                current_projected_decal_angle_fade[1],
                            )
                            .with_receiver_mask(current_projected_decal_receivers)
                            .with_surface_response(
                                current_projected_decal_surface[0],
                                current_projected_decal_surface[1],
                                current_projected_decal_surface[2],
                            ),
                    );
                }
                DrawCommand::SetProjectedDecalAtlas3D { atlas_id } => {
                    current_projected_decal_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalNormalAtlas3D { atlas_id } => {
                    current_projected_decal_normal_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalRoughnessAtlas3D { atlas_id } => {
                    current_projected_decal_roughness_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalMaskAtlas3D { atlas_id } => {
                    current_projected_decal_mask_atlas_id =
                        if atlas_id == 0 { None } else { Some(atlas_id) };
                }
                DrawCommand::SetProjectedDecalDistanceFade3D { start, end } => {
                    current_projected_decal_fade = if start >= 0.0 && end > start {
                        [start, end]
                    } else {
                        [0.0, 0.0]
                    };
                }
                DrawCommand::SetProjectedDecalAngleFade3D { start, end } => {
                    current_projected_decal_angle_fade = if start >= 0.0 && end > start {
                        [start.clamp(0.0, 1.0), end.clamp(0.0, 1.0)]
                    } else {
                        [0.0, 0.0]
                    };
                }
                DrawCommand::SetProjectedDecalReceiverMask3D { mask } => {
                    current_projected_decal_receivers = if mask == 0 {
                        DECAL_RECEIVER_ALL
                    } else {
                        mask.min(DECAL_RECEIVER_ALL)
                    };
                }
                DrawCommand::SetProjectedDecalSurfaceResponse3D {
                    normal_strength,
                    roughness,
                    roughness_strength,
                } => {
                    current_projected_decal_surface = [
                        normal_strength.clamp(0.0, 2.0),
                        if roughness > 0.0 {
                            roughness.clamp(0.04, 1.0)
                        } else {
                            0.72
                        },
                        roughness_strength.clamp(0.0, 1.0),
                    ];
                }
                DrawCommand::DrawProjectedDecal3DUV {
                    position,
                    yaw,
                    width,
                    length,
                    depth,
                    color,
                    uv_rect,
                } => {
                    let albedo_id = current_projected_decal_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let normal_id = current_projected_decal_normal_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let roughness_id = current_projected_decal_roughness_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let mask_id = current_projected_decal_mask_atlas_id
                        .filter(|atlas_id| self.texture_manager.get(*atlas_id).is_some())
                        .unwrap_or(0);
                    let binding = ProjectedDecalAtlasBinding {
                        albedo_id,
                        normal_id,
                        roughness_id,
                        mask_id,
                    };
                    let atlas_slot =
                        if albedo_id != 0 || normal_id != 0 || roughness_id != 0 || mask_id != 0 {
                            if let Some(slot) = projected_decal_atlas_bindings
                                .iter()
                                .position(|existing| *existing == binding)
                            {
                                Some(slot as u32)
                            } else if projected_decal_atlas_bindings.len() < MAX_POST_DECAL_ATLASES
                            {
                                projected_decal_atlas_bindings.push(binding);
                                Some((projected_decal_atlas_bindings.len() - 1) as u32)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                    let normal_atlas_enabled = atlas_slot.is_some() && normal_id != 0;
                    let roughness_atlas_enabled = atlas_slot.is_some() && roughness_id != 0;
                    let mask_atlas_enabled = atlas_slot.is_some() && mask_id != 0;
                    projected_decal_count += 1;
                    projected_decals.push(
                        PostDecalGpu::new_with_uv(
                            position.to_array(),
                            yaw,
                            width,
                            length,
                            depth,
                            color,
                            uv_rect,
                            atlas_slot,
                        )
                        .with_distance_fade(
                            current_projected_decal_fade[0],
                            current_projected_decal_fade[1],
                        )
                        .with_angle_fade(
                            current_projected_decal_angle_fade[0],
                            current_projected_decal_angle_fade[1],
                        )
                        .with_receiver_mask(current_projected_decal_receivers)
                        .with_surface_response(
                            current_projected_decal_surface[0],
                            current_projected_decal_surface[1],
                            current_projected_decal_surface[2],
                        )
                        .with_material_maps(
                            normal_atlas_enabled,
                            roughness_atlas_enabled,
                            mask_atlas_enabled,
                        ),
                    );
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
                            material_response: [1.0, 0.0, 1.0, 1.0],
                            texture_flags2: [0.0, 0.0, 0.0, 0.0],
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
                    scene_removal_count += 1;
                    self.render_world.destroy_object(scene_id, object_id);
                }
                DrawCommand::Scene3DClear { scene_id } => {
                    scene_removal_count += 1;
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
                    flags,
                    lod_near,
                    lod_far,
                    wind_strength,
                    atlas_uv,
                } => {
                    scene_upsert_count += 1;
                    resident_chunk_rebuild_count += 1;
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
                        flags,
                        lod_near,
                        lod_far,
                        wind_strength,
                        atlas_uv,
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
                    scene_removal_count += 1;
                    resident_chunk_rebuild_count += 1;
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
                    scene_removal_count += 1;
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
                    scene_removal_count += 1;
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
                    resident_chunk_rebuild_count += 1;
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
                            flags: instance.flags,
                            lod_near: instance.lod_near,
                            lod_far: instance.lod_far,
                            wind_strength: instance.wind_strength,
                            atlas_uv: instance.atlas_uv,
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
                    resident_chunk_rebuild_count += 1;
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
                                flags: instance.flags,
                                lod_near: instance.lod_near,
                                lod_far: instance.lod_far,
                                wind_strength: instance.wind_strength,
                                atlas_uv: instance.atlas_uv,
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
                    resident_chunk_rebuild_count += 1;
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
                                flags: instance.flags,
                                lod_near: instance.lod_near,
                                lod_far: instance.lod_far,
                                wind_strength: instance.wind_strength,
                                atlas_uv: instance.atlas_uv,
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
                    resident_chunk_rebuild_count += 1;
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
                            // NDC -> logical screen coordinates.
                            let screen_x = (ndc_x + 1.0) * 0.5 * screen_w - bw * 0.5;
                            let screen_y = (1.0 - ndc_y) * 0.5 * screen_h - bh * 0.5;

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
        perf.decode_ms = elapsed_ms_opt(decode_start);
        if perf_overrides.has(RENDERER_DIAG_DISABLE_SHADOWS) {
            shadow_enabled = false;
            shadow_strength = 0.0;
            shadow_quality = 0;
        }
        if perf_overrides.has(RENDERER_DIAG_DISABLE_POST_EFFECTS) {
            post_bloom_strength = 0.0;
            post_sharpen_strength = 0.0;
            post_fxaa_strength = 0.0;
            post_contact_ao_strength = 0.0;
            post_contact_ao_quality = 0;
            projected_decals.clear();
            projected_decal_atlas_bindings.clear();
        } else {
            if perf_overrides.has(RENDERER_DIAG_DISABLE_BLOOM) {
                post_bloom_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_SHARPEN) {
                post_sharpen_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_FXAA) {
                post_fxaa_strength = 0.0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_CONTACT_AO) {
                post_contact_ao_strength = 0.0;
                post_contact_ao_quality = 0;
            }
            if perf_overrides.has(RENDERER_DIAG_DISABLE_DECALS) {
                projected_decals.clear();
                projected_decal_atlas_bindings.clear();
            }
        }
        let contact_ao_active = post_contact_ao_strength > 0.001 && post_contact_ao_quality > 0;
        let projected_decals_active = !projected_decals.is_empty();
        let post_depth_active = contact_ao_active || projected_decals_active;
        let depth_prepass_active = MAIN_SAMPLE_COUNT > 1 && post_depth_active;
        let primitives_enabled = !perf_overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVES);
        let primitive_shadows_enabled = primitives_enabled
            && shadow_enabled
            && !perf_overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS);

        let scene_update_start = if perf_enabled { Some(perf_now()) } else { None };
        for scene_id in &retained_scene_draws {
            self.render_world
                .collect_scene_draws(*scene_id, &mut model_draws);
            if !primitives_enabled {
                continue;
            }
            self.render_world.collect_scene_primitive_draws(
                *scene_id,
                camera3d_uniform.as_ref(),
                &mut primitive_draws,
                &mut primitive_chunks,
            );
            if depth_prepass_active {
                self.render_world.collect_scene_primitive_depth_draws(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_depth_draws,
                    &mut primitive_depth_chunks,
                );
            }
            if primitive_shadows_enabled {
                self.render_world.collect_scene_primitive_shadow_objects(
                    *scene_id,
                    camera3d_uniform.as_ref(),
                    &mut primitive_shadow_draws,
                );
                self.render_world
                    .collect_scene_primitive_shadow_chunks_from_candidates(
                        *scene_id,
                        camera3d_uniform.as_ref(),
                        &primitive_chunks,
                        &mut primitive_shadow_chunks,
                    );
            }
        }
        perf.scene_update_ms = elapsed_ms_opt(scene_update_start);

        // Flush font atlas (re-upload if new glyphs were rasterized)
        self.font_manager
            .ensure_atlas(&mut self.texture_manager, &self.device, &self.queue);
        self.font_manager.reset_current();

        // Resolve draw list: sort by (layer, order), produce draw calls
        let frame = self.draw_list.resolve();
        Self::debug_submit_status(
            debug_frame_count,
            &format!(
                "voplay submit #{} bytes={} cmds={} cam3d={} modelCmds={} sceneUpserts={} sceneDraws={} models={} primitives={} primitiveChunks={} skybox={} projectedDecals={} diagFlags=0x{:x} 2d(rect/circ/line/text/sprite)={}/{}/{}/{}/{} resolved(shapes/sprites/calls/cams)={}/{}/{}/{} clear={:.2},{:.2},{:.2}",
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
                projected_decal_count,
                perf_overrides.flags(),
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

        let mut frame_graph = FrameGraph::single_view(
            debug_frame_count.min(u32::MAX as u64) as u32,
            perf_overrides.flags(),
        );
        frame_graph.declare_target(RES_SURFACE_COLOR, true);
        frame_graph.declare_target(
            RES_MAIN_COLOR,
            self.post_color_view.is_some() || self.msaa_color_view.is_some(),
        );
        frame_graph.declare_target(RES_DEPTH, self.depth_view.is_some());
        frame_graph.declare_target(RES_SHADOW_MAP, true);
        frame_graph.declare_target(RES_POST_COLOR, self.post_color_view.is_some());
        frame_graph.declare_target(RES_WATER_COLOR, false);
        frame_graph.declare_target(RES_OVERLAY, true);
        if post_depth_active {
            frame_graph.declare_target(RES_RECEIVER_MASK, self.receiver_mask_view.is_some());
            frame_graph.declare_target(RES_SURFACE_PROPS, self.surface_props_view.is_some());
        }
        frame_graph.plan_pass(
            RenderPassKind::DepthPrepass,
            &[],
            &[RES_DEPTH],
            depth_prepass_active,
        );
        frame_graph.plan_pass(
            RenderPassKind::Shadow,
            &[RES_DEPTH],
            &[RES_SHADOW_MAP],
            shadow_enabled
                && camera3d_uniform.is_some()
                && (!model_draws.is_empty()
                    || !primitive_shadow_draws.is_empty()
                    || !primitive_shadow_chunks.is_empty()),
        );
        frame_graph.plan_pass(
            RenderPassKind::MainOpaque,
            &[RES_SHADOW_MAP],
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::MainTransparent,
            &[RES_MAIN_COLOR, RES_DEPTH],
            &[RES_MAIN_COLOR],
            false,
        );
        frame_graph.plan_pass(
            RenderPassKind::Water,
            &[RES_DEPTH, RES_MAIN_COLOR],
            &[RES_WATER_COLOR, RES_MAIN_COLOR],
            false,
        );
        frame_graph.plan_pass(
            RenderPassKind::Post,
            &[
                RES_MAIN_COLOR,
                RES_DEPTH,
                RES_RECEIVER_MASK,
                RES_SURFACE_PROPS,
            ],
            &[RES_POST_COLOR, RES_SURFACE_COLOR],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::Overlay,
            &[RES_SURFACE_COLOR],
            &[RES_OVERLAY],
            true,
        );
        frame_graph.plan_pass(
            RenderPassKind::BackendSubmit,
            &[RES_OVERLAY],
            &[RES_SURFACE_COLOR],
            true,
        );

        let depth_start = if perf_enabled { Some(perf_now()) } else { None };
        if frame_graph.has_pass(RenderPassKind::DepthPrepass) {
            let empty_model_draws: &[ModelDraw] = &[];
            let empty_primitive_draws: &[PrimitiveDraw] = &[];
            let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
            if !primitive_depth_chunks.is_empty() {
                self.primitive_pipeline.append_resident_depth_draws(
                    &primitive_depth_chunks,
                    &mut primitive_depth_draws,
                );
            }
            let (depth_model_draws, depth_primitive_draws, depth_view_proj) =
                if let Some(ref cam3d) = camera3d_uniform {
                    (
                        &model_draws[..],
                        &primitive_depth_draws[..],
                        cam3d.view_proj,
                    )
                } else {
                    (
                        empty_model_draws,
                        empty_primitive_draws,
                        math3d::MAT4_IDENTITY,
                    )
                };
            self.pipeline_depth.render_depth_pass(
                &self.device,
                &mut encoder,
                &self.queue,
                &depth_view_proj,
                depth_model_draws,
                depth_primitive_draws,
                empty_primitive_chunks,
                &self.primitive_pipeline,
                &self.model_manager,
            );
            primitive_depth_draw_calls = self.pipeline_depth.last_primitive_batch_count();
        }
        perf.depth_pass_ms = elapsed_ms_opt(depth_start);
        frame_graph
            .executor()
            .execute_recorded(RenderPassKind::DepthPrepass, perf.depth_pass_ms);

        let mut shadow_active = false;
        let shadow_start = if perf_enabled { Some(perf_now()) } else { None };
        if frame_graph.has_pass(RenderPassKind::Shadow) {
            if let Some(ref cam3d) = camera3d_uniform {
                if light_uniform.count[0] > 0 && light_uniform.lights[0].position_or_dir[3] == 0.0 {
                    let shadow_to_light = Vec3::new(
                        light_uniform.lights[0].position_or_dir[0],
                        light_uniform.lights[0].position_or_dir[1],
                        light_uniform.lights[0].position_or_dir[2],
                    );
                    let shadow_dir = (-shadow_to_light).normalize();
                    if shadow_dir.length() > 0.0 {
                        let mut cascade_count = shadow_cascade_count_for_quality(shadow_quality);
                        if camera3d_state.is_none() {
                            cascade_count = 1;
                        }
                        let shadow_atlas_size =
                            shadow_atlas_resolution(shadow_resolution, cascade_count);
                        let tile_resolution = if cascade_count > 1 {
                            (shadow_atlas_size / 2).max(1)
                        } else {
                            shadow_atlas_size
                        };
                        if self.pipeline_shadow.size() != shadow_atlas_size {
                            self.clear_texture_bind_group_caches();
                            self.pipeline_shadow.resize(&self.device, shadow_atlas_size);
                        }
                        let mut shadow_cascade_vps = [math3d::MAT4_IDENTITY; 4];
                        let mut shadow_cascade_splits = [0.0; 4];
                        let shadow_vp = if let Some((eye, target, up, fov, near, camera_far)) =
                            camera3d_state
                        {
                            let shadow_far = if shadow_distance > 0.0 {
                                shadow_distance.min(camera_far).max(near + 0.1)
                            } else {
                                camera_far
                            };
                            if cascade_count > 1 {
                                shadow_cascade_splits =
                                    compute_shadow_cascade_splits(near, shadow_far, cascade_count);
                                let mut cascade_near = near;
                                for cascade_index in 0..cascade_count {
                                    let cascade_far = shadow_cascade_splits[cascade_index];
                                    shadow_cascade_vps[cascade_index] =
                                        math3d::compute_shadow_vp_for_camera_stabilized(
                                            eye,
                                            target,
                                            up,
                                            fov.to_radians(),
                                            aspect,
                                            cascade_near,
                                            cascade_far,
                                            shadow_dir,
                                            tile_resolution,
                                        );
                                    cascade_near = cascade_far;
                                }
                                shadow_cascade_vps[0]
                            } else {
                                let shadow_vp = math3d::compute_shadow_vp_for_camera_stabilized(
                                    eye,
                                    target,
                                    up,
                                    fov.to_radians(),
                                    aspect,
                                    near,
                                    shadow_far,
                                    shadow_dir,
                                    tile_resolution,
                                );
                                shadow_cascade_vps[0] = shadow_vp;
                                shadow_cascade_splits[0] = shadow_far;
                                shadow_vp
                            }
                        } else {
                            let inv_view_proj =
                                    math3d::mat4_inverse(&cam3d.view_proj).ok_or_else(|| {
                                        "voplay: failed to invert camera view projection for shadow mapping"
                                            .to_string()
                                    })?;
                            let shadow_vp = math3d::compute_shadow_vp_stabilized(
                                &inv_view_proj,
                                shadow_dir,
                                tile_resolution,
                            );
                            shadow_cascade_vps[0] = shadow_vp;
                            shadow_vp
                        };
                        if cascade_count > 1 {
                            let mut cascade_primitive_shadow_draws: Vec<Vec<PrimitiveDraw>> =
                                Vec::new();
                            let mut cascade_primitive_shadow_chunks: Vec<Vec<PrimitiveChunkRef>> =
                                Vec::new();
                            if !primitive_shadow_draws.is_empty()
                                || !primitive_shadow_chunks.is_empty()
                            {
                                cascade_primitive_shadow_draws.reserve(cascade_count);
                                cascade_primitive_shadow_chunks.reserve(cascade_count);
                                for cascade_index in 0..cascade_count {
                                    let light_camera = Camera3DUniform {
                                        view_proj: shadow_cascade_vps[cascade_index],
                                        camera_pos: cam3d.camera_pos,
                                        _pad: 0.0,
                                    };
                                    let mut cascade_shadow_draws = Vec::new();
                                    let mut cascade_shadow_chunks = Vec::new();
                                    for scene_id in &retained_scene_draws {
                                        self.render_world
                                            .collect_scene_primitive_shadow_objects_for_light_view(
                                                *scene_id,
                                                camera3d_uniform.as_ref(),
                                                &light_camera,
                                                &mut cascade_shadow_draws,
                                            );
                                        self.render_world
                                            .collect_scene_primitive_shadow_chunks_for_light_view(
                                                *scene_id,
                                                camera3d_uniform.as_ref(),
                                                &light_camera,
                                                &primitive_shadow_chunks,
                                                &mut cascade_shadow_chunks,
                                            );
                                    }
                                    if !cascade_shadow_chunks.is_empty() {
                                        self.primitive_pipeline.append_resident_shadow_draws(
                                            &cascade_shadow_chunks,
                                            &mut cascade_shadow_draws,
                                        );
                                    }
                                    cascade_primitive_shadow_draws.push(cascade_shadow_draws);
                                    cascade_primitive_shadow_chunks.push(Vec::new());
                                }
                            }
                            let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                            self.pipeline_shadow.render_shadow_cascade_pass(
                                &self.device,
                                &mut encoder,
                                &self.queue,
                                &shadow_cascade_vps[..cascade_count],
                                &model_draws,
                                &primitive_shadow_draws,
                                &cascade_primitive_shadow_draws,
                                empty_primitive_chunks,
                                &cascade_primitive_shadow_chunks,
                                &self.primitive_pipeline,
                                &self.model_manager,
                            );
                        } else {
                            let empty_primitive_chunks: &[PrimitiveChunkRef] = &[];
                            if !primitive_shadow_chunks.is_empty() {
                                self.primitive_pipeline.append_resident_shadow_draws(
                                    &primitive_shadow_chunks,
                                    &mut primitive_shadow_draws,
                                );
                            }
                            self.pipeline_shadow.render_shadow_pass(
                                &self.device,
                                &mut encoder,
                                &self.queue,
                                &shadow_vp,
                                &model_draws,
                                &primitive_shadow_draws,
                                empty_primitive_chunks,
                                &self.primitive_pipeline,
                                &self.model_manager,
                            );
                        }
                        primitive_shadow_draw_calls =
                            self.pipeline_shadow.last_primitive_batch_count();
                        light_uniform.shadow_vp = shadow_vp;
                        light_uniform.shadow_cascade_vp = shadow_cascade_vps;
                        light_uniform.shadow_cascade_splits = shadow_cascade_splits;
                        light_uniform.shadow_params =
                            [1.0, 0.002, shadow_softness, shadow_strength];
                        light_uniform.shadow_params2 = [
                            shadow_distance,
                            shadow_fade,
                            shadow_quality as f32,
                            cascade_count as f32,
                        ];
                        light_uniform.count[2] = 0;
                        shadow_active = true;
                    }
                }
            }
        }
        if !shadow_active {
            light_uniform.shadow_vp = math3d::MAT4_IDENTITY;
            light_uniform.shadow_cascade_vp = [math3d::MAT4_IDENTITY; 4];
            light_uniform.shadow_cascade_splits = [0.0; 4];
            light_uniform.shadow_params = [0.0, 0.002, shadow_softness, shadow_strength];
            light_uniform.shadow_params2 =
                [shadow_distance, shadow_fade, shadow_quality as f32, 0.0];
        }
        perf.shadow_pass_ms = elapsed_ms_opt(shadow_start);
        frame_graph
            .executor()
            .execute_recorded(RenderPassKind::Shadow, perf.shadow_pass_ms);

        // Render pass
        let main_aux_targets_enabled = post_depth_active;

        let mut post_uniform = PostUniform::from_settings(
            self.surface_config.width,
            self.surface_config.height,
            post_bloom_threshold,
            post_bloom_strength,
            post_sharpen_strength,
            post_fxaa_strength,
            post_contact_ao_strength,
            post_contact_ao_radius,
            post_contact_ao_depth_scale,
            post_contact_ao_detail_strength,
            post_contact_ao_detail_radius,
            post_contact_ao_normal_bias,
            post_contact_ao_quality,
        );
        let mut post_decal_light_vectors = [[0.0f32; 4]; 3];
        let mut post_decal_light_colors = [[0.0f32; 4]; 3];
        let mut post_decal_light_count = 0usize;
        for light in light_uniform
            .lights
            .iter()
            .take(light_uniform.count[0].min(light_uniform.lights.len() as u32) as usize)
        {
            if light.color_intensity[3] > 0.0 {
                post_decal_light_vectors[post_decal_light_count] = [
                    light.position_or_dir[0],
                    light.position_or_dir[1],
                    light.position_or_dir[2],
                    light.color_intensity[3],
                ];
                post_decal_light_colors[post_decal_light_count] = [
                    light.color_intensity[0],
                    light.color_intensity[1],
                    light.color_intensity[2],
                    light.position_or_dir[3],
                ];
                post_decal_light_count += 1;
                if post_decal_light_count >= post_decal_light_vectors.len() {
                    break;
                }
            }
        }
        if post_decal_light_count > 0 {
            post_uniform = post_uniform.with_decal_lights(
                &post_decal_light_vectors[..post_decal_light_count],
                &post_decal_light_colors[..post_decal_light_count],
            );
        }
        self.queue.write_buffer(
            &self.post_uniform_buffer,
            0,
            bytemuck::bytes_of(&post_uniform),
        );
        let post_inv_view_proj = camera3d_uniform
            .as_ref()
            .and_then(|camera| math3d::mat4_inverse(&camera.view_proj))
            .unwrap_or(math3d::MAT4_IDENTITY);
        let post_camera_pos = camera3d_state
            .map(|(eye, _, _, _, _, _)| eye.to_array())
            .unwrap_or([0.0, 0.0, 0.0]);
        self.queue.write_buffer(
            &self.post_decal_uniform_buffer,
            0,
            bytemuck::bytes_of(&PostDecalUniform::from_decals(
                post_inv_view_proj,
                post_camera_pos,
                &projected_decals,
                projected_decal_atlas_bindings.len() as u32,
            )),
        );
        let main_start = if perf_enabled { Some(perf_now()) } else { None };
        {
            let main_setup_start = if perf_enabled { Some(perf_now()) } else { None };
            let post_color_view = self
                .post_color_view
                .as_ref()
                .ok_or_else(|| "voplay: missing post color target".to_string())?;
            let main_color_view = if MAIN_SAMPLE_COUNT > 1 {
                self.msaa_color_view
                    .as_ref()
                    .ok_or_else(|| "voplay: missing MSAA color target".to_string())?
            } else {
                post_color_view
            };
            let receiver_mask_view = if main_aux_targets_enabled {
                Some(
                    self.receiver_mask_view
                        .as_ref()
                        .ok_or_else(|| "voplay: missing receiver mask target".to_string())?,
                )
            } else {
                None
            };
            let surface_props_view = if main_aux_targets_enabled {
                Some(
                    self.surface_props_view
                        .as_ref()
                        .ok_or_else(|| "voplay: missing surface props target".to_string())?,
                )
            } else {
                None
            };
            let main_receiver_mask_view = if main_aux_targets_enabled {
                Some(if MAIN_SAMPLE_COUNT > 1 {
                    self.msaa_receiver_mask_view
                        .as_ref()
                        .ok_or_else(|| "voplay: missing MSAA receiver mask target".to_string())?
                } else {
                    receiver_mask_view.expect("receiver mask view present")
                })
            } else {
                None
            };
            let main_surface_props_view = if main_aux_targets_enabled {
                Some(if MAIN_SAMPLE_COUNT > 1 {
                    self.msaa_surface_props_view
                        .as_ref()
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
            let receiver_mask_resolve_target = if main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1
            {
                receiver_mask_view
            } else {
                None
            };
            let surface_props_resolve_target = if main_aux_targets_enabled && MAIN_SAMPLE_COUNT > 1
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
            let receiver_mask_store = if MAIN_SAMPLE_COUNT > 1 {
                wgpu::StoreOp::Discard
            } else {
                wgpu::StoreOp::Store
            };
            let surface_props_store = if MAIN_SAMPLE_COUNT > 1 {
                wgpu::StoreOp::Discard
            } else {
                wgpu::StoreOp::Store
            };
            let color_attachments = [
                Some(wgpu::RenderPassColorAttachment {
                    view: main_color_view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: color_store,
                    },
                }),
                main_receiver_mask_view.map(|view| wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: receiver_mask_resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: receiver_mask_store,
                    },
                }),
                main_surface_props_view.map(|view| wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: surface_props_resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: surface_props_store,
                    },
                }),
            ];
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay_main"),
                color_attachments: &color_attachments,
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
            perf.main_pass_setup_ms = elapsed_ms_opt(main_setup_start);

            if let (Some(cubemap_id), Some((eye, target, up, fov, near, far))) =
                (skybox_cubemap_id, camera3d_state)
            {
                if let Some(cubemap) = self.texture_manager.get_cubemap(cubemap_id) {
                    let skybox_start = if perf_enabled { Some(perf_now()) } else { None };
                    let view_rot = math3d::view_rotation_only(eye, target, up);
                    let proj = math3d::perspective_rh_zo(fov.to_radians(), aspect, near, far);
                    let vp = math3d::mat4_mul(&proj, &view_rot);
                    let inv_vp = math3d::mat4_inverse(&vp).unwrap_or(math3d::MAT4_IDENTITY);
                    self.pipeline_skybox.set_camera(&self.queue, &inv_vp);
                    self.pipeline_skybox
                        .draw(&mut render_pass, cubemap, main_aux_targets_enabled);
                    perf.main_skybox_ms += elapsed_ms_opt(skybox_start);
                }
            }

            // Draw 3D models first (depth tested)
            if !model_draws.is_empty() {
                if let Some(ref cam3d) = camera3d_uniform {
                    let model_start = if perf_enabled { Some(perf_now()) } else { None };
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
                        main_aux_targets_enabled,
                    );
                    perf.main_model_ms += elapsed_ms_opt(model_start);
                }
            }

            if !primitive_draws.is_empty() || !primitive_chunks.is_empty() {
                if let Some(ref cam3d) = camera3d_uniform {
                    let primitive_start = if perf_enabled { Some(perf_now()) } else { None };
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
                        main_aux_targets_enabled,
                    );
                    primitive_main_submitted = true;
                    perf.main_primitive_ms += elapsed_ms_opt(primitive_start);
                }
            }
            let main_close_start = if perf_enabled { Some(perf_now()) } else { None };
            drop(render_pass);
            perf.main_pass_close_ms = elapsed_ms_opt(main_close_start);
        }
        if primitive_main_submitted {
            primitive_main_draw_calls = self.primitive_pipeline.last_main_batch_count();
        }
        perf.main_pass_ms = elapsed_ms_opt(main_start);
        frame_graph
            .executor()
            .execute_recorded(RenderPassKind::MainOpaque, perf.main_pass_ms);

        let post_start = if perf_enabled { Some(perf_now()) } else { None };
        {
            let post_color_view = self
                .post_color_view
                .as_ref()
                .ok_or_else(|| "voplay: missing post color target".to_string())?;
            let receiver_mask_view = self
                .receiver_mask_view
                .as_ref()
                .ok_or_else(|| "voplay: missing receiver mask target".to_string())?;
            let surface_props_view = self
                .surface_props_view
                .as_ref()
                .ok_or_else(|| "voplay: missing surface props target".to_string())?;
            let dynamic_post_bind_group;
            let post_bind_group = if projected_decal_atlas_bindings.is_empty() {
                self.post_bind_group
                    .as_ref()
                    .ok_or_else(|| "voplay: missing post bind group".to_string())?
            } else {
                let fallback_decal_atlas = self.pipeline_post.decal_fallback_view();
                let fallback_decal_normal_atlas = self.pipeline_post.decal_normal_fallback_view();
                let fallback_decal_roughness_atlas =
                    self.pipeline_post.decal_roughness_fallback_view();
                let fallback_decal_mask_atlas = self.pipeline_post.decal_mask_fallback_view();
                let mut decal_atlas_views = [fallback_decal_atlas; MAX_POST_DECAL_ATLASES];
                let mut decal_normal_atlas_views =
                    [fallback_decal_normal_atlas; MAX_POST_DECAL_ATLASES];
                let mut decal_roughness_atlas_views =
                    [fallback_decal_roughness_atlas; MAX_POST_DECAL_ATLASES];
                let mut decal_mask_atlas_views =
                    [fallback_decal_mask_atlas; MAX_POST_DECAL_ATLASES];
                for (slot, binding) in projected_decal_atlas_bindings.iter().enumerate() {
                    if let Some(texture) = self.texture_manager.get(binding.albedo_id) {
                        decal_atlas_views[slot] = &texture.view;
                    }
                    if let Some(texture) = self.texture_manager.get(binding.normal_id) {
                        decal_normal_atlas_views[slot] = &texture.view;
                    }
                    if let Some(texture) = self.texture_manager.get(binding.roughness_id) {
                        decal_roughness_atlas_views[slot] = &texture.view;
                    }
                    if let Some(texture) = self.texture_manager.get(binding.mask_id) {
                        decal_mask_atlas_views[slot] = &texture.view;
                    }
                }
                let post_depth_view = if MAIN_SAMPLE_COUNT > 1 {
                    self.pipeline_depth.depth_texture_view()
                } else {
                    self.depth_view
                        .as_ref()
                        .ok_or_else(|| "voplay: missing depth target".to_string())?
                };
                dynamic_post_bind_group = self.pipeline_post.create_bind_group(
                    &self.device,
                    post_color_view,
                    post_depth_view,
                    &self.post_uniform_buffer,
                    &self.post_decal_uniform_buffer,
                    decal_atlas_views,
                    decal_normal_atlas_views,
                    decal_roughness_atlas_views,
                    decal_mask_atlas_views,
                    receiver_mask_view,
                    surface_props_view,
                );
                &dynamic_post_bind_group
            };
            let mut post_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay_post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.pipeline_post.draw(&mut post_pass, &post_bind_group);
        }
        perf.post_pass_ms = elapsed_ms_opt(post_start);
        frame_graph
            .executor()
            .execute_recorded(RenderPassKind::Post, perf.post_pass_ms);

        let overlay_start = if perf_enabled { Some(perf_now()) } else { None };
        {
            let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay_overlay_2d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            for dc in &frame.draw_calls {
                let cam_offset = dc.camera_idx as u32 * align;
                match &dc.kind {
                    DrawCallKind::Shapes { start, count } => {
                        self.pipeline2d.draw_range(
                            &mut overlay_pass,
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
                                &mut overlay_pass,
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
        perf.overlay_pass_ms = elapsed_ms_opt(overlay_start);
        frame_graph
            .executor()
            .execute_recorded(RenderPassKind::Overlay, perf.overlay_pass_ms);

        let queue_submit_start = if perf_enabled { Some(perf_now()) } else { None };
        self.queue.submit(std::iter::once(encoder.finish()));
        perf.queue_submit_cpu_ms = elapsed_ms_opt(queue_submit_start);
        let present_start = if perf_enabled { Some(perf_now()) } else { None };
        output.present();
        perf.present_cpu_ms = elapsed_ms_opt(present_start);
        frame_graph.executor().execute_recorded(
            RenderPassKind::BackendSubmit,
            perf.queue_submit_cpu_ms + perf.present_cpu_ms,
        );
        self.last_frame_graph_report = frame_graph.report();
        if perf_enabled {
            perf.submit_frame_ms = elapsed_ms_opt(frame_start);
            perf.graph_pass_count = self.last_frame_graph_report.pass_count;
            perf.graph_resource_count = self.last_frame_graph_report.resource_count;
            perf.graph_target_count = self.last_frame_graph_report.target_count;
            perf.graph_ready_target_count = self.last_frame_graph_report.ready_target_count;
            perf.text_draws = text_count;
            perf.sprite_draws = sprite_count;
            perf.primitive_draws = primitive_main_draw_calls;
            perf.primitive_chunks = saturating_u32(primitive_chunks.len());
            perf.retained_scene_upserts = scene_upsert_count;
            perf.retained_scene_removals = scene_removal_count;
            perf.resident_chunk_rebuilds = resident_chunk_rebuild_count;
            perf.shadow_cascades = if shadow_active {
                light_uniform.shadow_params2[3].max(1.0) as u32
            } else {
                0
            };
            let primitive_shadow_draw_count = primitive_shadow_draw_calls;
            let primitive_depth_draw_count = primitive_depth_draw_calls;
            perf.post_effects = 1
                + (post_bloom_strength > 0.0) as u32
                + (post_sharpen_strength > 0.0) as u32
                + (post_fxaa_strength > 0.0) as u32
                + contact_ao_active as u32
                + projected_decals_active as u32;
            perf.visible_objects = saturating_u32(model_draws.len() + primitive_draws.len());
            let mut model_mesh_draws = 0u32;
            let mut skinned_mesh_draws = 0u32;
            let mut instance_count = 0u32;
            let mut triangle_count = 0u32;
            for draw in &model_draws {
                let Some(gpu_model) = self.model_manager.get(draw.model_id) else {
                    continue;
                };
                for mesh in &gpu_model.meshes {
                    model_mesh_draws = model_mesh_draws.saturating_add(1);
                    if mesh.skinned {
                        skinned_mesh_draws = skinned_mesh_draws.saturating_add(1);
                    }
                    instance_count = instance_count.saturating_add(1);
                    triangle_count = triangle_count.saturating_add(mesh.index_count / 3);
                }
            }
            instance_count =
                instance_count.saturating_add(self.primitive_pipeline.last_main_instance_count());
            triangle_count =
                triangle_count.saturating_add(self.primitive_pipeline.last_main_triangle_count());
            perf.model_draws = model_mesh_draws;
            perf.skinned_draws = skinned_mesh_draws;
            perf.instances = instance_count;
            perf.triangles = triangle_count;
            perf.draw_calls = saturating_u32(frame.draw_calls.len())
                .saturating_add(model_mesh_draws)
                .saturating_add(perf.primitive_draws)
                .saturating_add(
                    perf.shadow_cascades
                        .saturating_mul(model_mesh_draws)
                        .saturating_add(primitive_shadow_draw_count),
                )
                .saturating_add(if depth_prepass_active {
                    model_mesh_draws + primitive_depth_draw_count
                } else {
                    0
                });
            let camera_upload = frame.cameras.len() * std::mem::size_of::<CameraUniform>();
            let shape_upload =
                frame.shapes.len() * std::mem::size_of::<crate::pipeline2d::ShapeInstance>();
            let sprite_upload = frame.sprites.len() * std::mem::size_of::<SpriteInstance>();
            let post_upload = std::mem::size_of::<PostUniform>()
                + std::mem::size_of::<PostDecalUniform>()
                + projected_decals.len() * std::mem::size_of::<PostDecalGpu>();
            perf.upload_bytes =
                saturating_u32(camera_upload + shape_upload + sprite_upload + post_upload);
            if perf.submit_frame_ms >= 16.0 {
                eprintln!(
                    "voplay renderer slow submit frame={} total={:.2}ms acquire={:.2}ms decode={:.2}ms scene={:.2}ms depth={:.2}ms shadow={:.2}ms main={:.2}ms(setup={:.2} sky={:.2} model={:.2} primitive={:.2} close={:.2}) post={:.2}ms overlay={:.2}ms queue={:.2}ms present={:.2}ms graphPasses={} graphResources={} graphTargets={}/{} slowestPass={} slowestPassMs={:.2} draws={} primitives={} chunks={} cascades={} postEffects={} upload={} flags=0x{:x}",
                    perf.frame_id,
                    perf.submit_frame_ms,
                    perf.surface_acquire_ms,
                    perf.decode_ms,
                    perf.scene_update_ms,
                    perf.depth_pass_ms,
                    perf.shadow_pass_ms,
                    perf.main_pass_ms,
                    perf.main_pass_setup_ms,
                    perf.main_skybox_ms,
                    perf.main_model_ms,
                    perf.main_primitive_ms,
                    perf.main_pass_close_ms,
                    perf.post_pass_ms,
                    perf.overlay_pass_ms,
                    perf.queue_submit_cpu_ms,
                    perf.present_cpu_ms,
                    self.last_frame_graph_report.pass_count,
                    self.last_frame_graph_report.resource_count,
                    self.last_frame_graph_report.ready_target_count,
                    self.last_frame_graph_report.target_count,
                    self.last_frame_graph_report.slowest_pass,
                    self.last_frame_graph_report.slowest_pass_ms,
                    perf.draw_calls,
                    perf.primitive_draws,
                    perf.primitive_chunks,
                    perf.shadow_cascades,
                    perf.post_effects,
                    perf.upload_bytes,
                    perf.diagnostic_flags,
                );
            }
            self.last_perf_packet = encode_renderer_perf_packet(&perf);
        } else {
            self.last_perf_packet.clear();
        }
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

    pub fn last_perf_packet(&self) -> &[u8] {
        &self.last_perf_packet
    }

    pub(crate) fn last_frame_graph_report(&self) -> &FrameGraphReport {
        &self.last_frame_graph_report
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
