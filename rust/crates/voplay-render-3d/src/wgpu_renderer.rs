use std::{collections::BTreeMap, sync::Arc, time::Instant};

use voplay_runtime::{
    render_feature::{AttachmentPoint, RealizedFeature},
    RenderEntity,
};
use wgpu::util::DeviceExt;

use crate::{
    decode_material_3d, AlphaMode, DrawPacket3d, LightKind, LightPacket3d, MaterialDescriptor,
    MeshArtifact3d, MeshDescriptor, PortableFeatureDraw3d, PortableFeaturePhase3d,
    PortableFeaturePlan3d, Render3dPlan, ShadowCascade, Wire3dError, WireAntiAlias3d,
    WireEnvironment3d, WireToneMap3d, BUILTIN_DECAL_MESH_3D, BUILTIN_TERRAIN_MESH_3D,
};

const MAX_GPU_LIGHTS: usize = 16;
const MAX_SKIN_JOINTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuSceneRendererConfig {
    pub max_meshes: usize,
    pub max_materials: usize,
    pub max_vertices_per_mesh: usize,
    pub max_indices_per_mesh: usize,
    pub max_shadow_cascades: usize,
    pub shadow_resolution: u32,
    pub sample_count: u32,
    pub max_textures: usize,
    pub max_texture_pixels: usize,
}

impl Default for WgpuSceneRendererConfig {
    fn default() -> Self {
        Self {
            max_meshes: 65_536,
            max_materials: 65_536,
            max_vertices_per_mesh: 16_777_216,
            max_indices_per_mesh: 50_331_648,
            max_shadow_cascades: 4,
            shadow_resolution: 2_048,
            sample_count: 1,
            max_textures: 65_536,
            max_texture_pixels: 16_777_216,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MeshUpload<'a> {
    pub descriptor: MeshDescriptor,
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub texcoords: &'a [[f32; 2]],
    pub joints: Option<&'a [[u16; 4]]>,
    pub weights: Option<&'a [[f32; 4]]>,
    pub indices: &'a [u32],
}

#[derive(Clone, Copy, Debug)]
pub struct TextureUpload3d<'a> {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba8: &'a [u8],
    pub srgb: bool,
}

#[derive(Clone, Debug)]
pub struct SkinPaletteUpload3d<'a> {
    pub entity: RenderEntity,
    pub joint_matrices: &'a [[[f32; 4]; 4]],
}

#[derive(Clone, Copy, Debug)]
pub struct ShadowCascadeRender<'a> {
    pub descriptor: ShadowCascade,
    pub light_view_projection: [[f32; 4]; 4],
    pub draws: &'a [DrawPacket3d],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WgpuSceneRendererError {
    InvalidConfig,
    InvalidMesh,
    DuplicateMesh,
    DuplicateMaterial,
    Capacity,
    MissingMesh,
    MissingMaterial,
    FenceExhausted,
    InvalidMaterialWire,
    InvalidTexture,
    DuplicateTexture,
    MissingTexture,
    InvalidSkinPalette,
    MissingSkinPalette,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuCapturedNode3d {
    pub node: u32,
    pub label: String,
    pub queue: u8,
    pub started_nanos: u64,
    pub finished_nanos: u64,
    pub pipeline_hash: u64,
    pub draw_calls: u32,
    pub dispatch_calls: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuCapturedAttachment3d {
    pub resource: u32,
    pub version: u32,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuShaderDiagnostic3d {
    pub pipeline_hash: u64,
    pub severity: u8,
    pub source_start: u32,
    pub source_end: u32,
    pub graph_node: u32,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WgpuFrameTrace3d {
    pub fence: u64,
    pub nodes: Vec<WgpuCapturedNode3d>,
    pub attachments: Vec<WgpuCapturedAttachment3d>,
    pub shader_diagnostics: Vec<WgpuShaderDiagnostic3d>,
}

fn elapsed_nanos(clock: Instant) -> u64 {
    u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn captured_node(
    node: u32,
    label: &str,
    started_nanos: u64,
    finished_nanos: u64,
    pipeline_hash: u64,
    draw_calls: usize,
) -> WgpuCapturedNode3d {
    WgpuCapturedNode3d {
        node,
        label: label.to_owned(),
        queue: 1,
        started_nanos,
        finished_nanos,
        pipeline_hash: pipeline_hash.max(1),
        draw_calls: u32::try_from(draw_calls).unwrap_or(u32::MAX),
        dispatch_calls: 0,
    }
}

fn captured_attachment(
    resource: u32,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> WgpuCapturedAttachment3d {
    WgpuCapturedAttachment3d {
        resource,
        version: 1,
        label: label.to_owned(),
        width,
        height,
        format: texture_format_tag(format),
        row_bytes: width.saturating_mul(4),
        bytes: Vec::new(),
    }
}

fn texture_format_tag(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => 1,
        wgpu::TextureFormat::Rgba8UnormSrgb => 2,
        wgpu::TextureFormat::Rgba16Float => 3,
        wgpu::TextureFormat::Depth24Plus => 4,
        wgpu::TextureFormat::Depth32Float => 5,
        _ => 255,
    }
}

struct GpuMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    skinned: bool,
}

struct GpuMaterialTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct GpuSkinPalette {
    buffer: wgpu::Buffer,
}

struct PreparedDraw {
    mesh: u64,
    material: u64,
    bind_group: wgpu::BindGroup,
}

struct PreparedShadowDraw {
    mesh: u64,
    bind_group: wgpu::BindGroup,
}

struct TemporalHistory {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    initialized: bool,
}

pub struct WgpuSceneRenderer {
    config: WgpuSceneRendererConfig,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    color_format: wgpu::TextureFormat,
    uniform_layout: wgpu::BindGroupLayout,
    shadow_uniform_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,
    opaque_back: wgpu::RenderPipeline,
    opaque_double: wgpu::RenderPipeline,
    blend_back: wgpu::RenderPipeline,
    blend_double: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    post_pipeline: wgpu::RenderPipeline,
    shadow_sampler: wgpu::Sampler,
    post_sampler: wgpu::Sampler,
    material_sampler: wgpu::Sampler,
    meshes: BTreeMap<u64, GpuMesh>,
    materials: BTreeMap<u64, MaterialDescriptor>,
    textures: BTreeMap<u64, GpuMaterialTexture>,
    skin_palettes: BTreeMap<RenderEntity, GpuSkinPalette>,
    fallback_white: GpuMaterialTexture,
    fallback_normal: GpuMaterialTexture,
    identity_skin_palette: GpuSkinPalette,
    data_features: crate::data_feature_wgpu::WgpuDataFeatureExecutor,
    temporal_history: Option<TemporalHistory>,
    next_fence: u64,
    last_frame_trace: Option<WgpuFrameTrace3d>,
}

impl WgpuSceneRenderer {
    pub fn shutdown(&mut self) {
        self.meshes.clear();
        self.materials.clear();
        self.textures.clear();
        self.skin_palettes.clear();
        self.data_features.shutdown();
        self.temporal_history = None;
        self.last_frame_trace = None;
    }

    pub fn device(&self) -> Arc<wgpu::Device> {
        Arc::clone(&self.device)
    }

    pub fn queue(&self) -> Arc<wgpu::Queue> {
        Arc::clone(&self.queue)
    }

    pub fn new(
        config: WgpuSceneRendererConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        color_format: wgpu::TextureFormat,
    ) -> Result<Self, WgpuSceneRendererError> {
        if config.max_meshes < 2
            || config.max_materials == 0
            || config.max_vertices_per_mesh == 0
            || config.max_indices_per_mesh == 0
            || config.max_shadow_cascades == 0
            || config.max_shadow_cascades > 4
            || config.shadow_resolution == 0
            || !matches!(config.sample_count, 1 | 2 | 4 | 8)
            || config.max_textures == 0
            || config.max_texture_pixels == 0
        {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay-3d-draw-uniform-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                texture_layout_entry(3),
                texture_layout_entry(4),
                texture_layout_entry(5),
                texture_layout_entry(6),
                texture_layout_entry(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shadow_uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("voplay-3d-shadow-uniform-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    texture_layout_entry(1),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    skin_palette_layout_entry(9),
                ],
            });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay-3d-post-layout"),
            entries: &[
                texture_layout_entry(0),
                texture_layout_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                skin_palette_layout_entry(3),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay-3d-mesh-shader"),
            source: wgpu::ShaderSource::Wgsl(SCENE_SHADER.into()),
        });
        let opaque_back = create_pipeline(
            &device,
            &uniform_layout,
            &shader,
            color_format,
            false,
            Some(wgpu::Face::Back),
            config.sample_count,
        );
        let opaque_double = create_pipeline(
            &device,
            &uniform_layout,
            &shader,
            color_format,
            false,
            None,
            config.sample_count,
        );
        let blend_back = create_pipeline(
            &device,
            &uniform_layout,
            &shader,
            color_format,
            true,
            Some(wgpu::Face::Back),
            config.sample_count,
        );
        let blend_double = create_pipeline(
            &device,
            &uniform_layout,
            &shader,
            color_format,
            true,
            None,
            config.sample_count,
        );
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay-3d-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline =
            create_shadow_pipeline(&device, &shadow_uniform_layout, &shadow_shader);
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voplay-3d-post-shader"),
            source: wgpu::ShaderSource::Wgsl(POST_SHADER.into()),
        });
        let post_pipeline = create_post_pipeline(&device, &post_layout, &post_shader, color_format);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay-3d-shadow-comparison-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay-3d-post-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay-3d-material-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let fallback_white = create_solid_material_texture(&device, &queue, [255; 4]);
        let fallback_normal = create_solid_material_texture(&device, &queue, [128, 128, 255, 255]);
        let identity_skin_palette = create_skin_palette(&device, &[identity_matrix()]);
        let data_features =
            crate::data_feature_wgpu::WgpuDataFeatureExecutor::new(config.max_materials)?;
        let mut renderer = Self {
            config,
            device,
            queue,
            color_format,
            uniform_layout,
            shadow_uniform_layout,
            post_layout,
            opaque_back,
            opaque_double,
            blend_back,
            blend_double,
            shadow_pipeline,
            post_pipeline,
            shadow_sampler,
            post_sampler,
            material_sampler,
            meshes: BTreeMap::new(),
            materials: BTreeMap::new(),
            textures: BTreeMap::new(),
            skin_palettes: BTreeMap::new(),
            fallback_white,
            fallback_normal,
            identity_skin_palette,
            data_features,
            temporal_history: None,
            next_fence: 1,
            last_frame_trace: None,
        };
        renderer.install_builtin_feature_meshes()?;
        Ok(renderer)
    }

    fn install_builtin_feature_meshes(&mut self) -> Result<(), WgpuSceneRendererError> {
        const QUAD_POSITIONS: [[f32; 3]; 4] = [
            [-0.5, 0.0, -0.5],
            [0.5, 0.0, -0.5],
            [0.5, 0.0, 0.5],
            [-0.5, 0.0, 0.5],
        ];
        const QUAD_NORMALS: [[f32; 3]; 4] = [[0.0, 1.0, 0.0]; 4];
        const QUAD_UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];
        self.upload_mesh(MeshUpload {
            descriptor: MeshDescriptor {
                id: BUILTIN_TERRAIN_MESH_3D,
                vertex_count: 4,
                index_count: 6,
                skinned: false,
            },
            positions: &QUAD_POSITIONS,
            normals: &QUAD_NORMALS,
            texcoords: &QUAD_UVS,
            joints: None,
            weights: None,
            indices: &QUAD_INDICES,
        })?;
        self.upload_mesh(MeshUpload {
            descriptor: MeshDescriptor {
                id: BUILTIN_DECAL_MESH_3D,
                vertex_count: 4,
                index_count: 6,
                skinned: false,
            },
            positions: &QUAD_POSITIONS,
            normals: &QUAD_NORMALS,
            texcoords: &QUAD_UVS,
            joints: None,
            weights: None,
            indices: &QUAD_INDICES,
        })
    }

    pub fn upload_mesh(&mut self, upload: MeshUpload<'_>) -> Result<(), WgpuSceneRendererError> {
        self.validate_mesh_upload(&upload)?;
        if self.meshes.contains_key(&upload.descriptor.id) {
            return Err(WgpuSceneRendererError::DuplicateMesh);
        }
        if self.meshes.len() == self.config.max_meshes {
            return Err(WgpuSceneRendererError::Capacity);
        }
        let mut vertices = Vec::with_capacity(upload.positions.len() * 56);
        for index in 0..upload.positions.len() {
            let position = upload.positions[index];
            let normal = upload.normals[index];
            let texcoord = upload.texcoords[index];
            for value in position.iter().chain(normal.iter()).chain(texcoord.iter()) {
                vertices.extend_from_slice(&value.to_ne_bytes());
            }
            let joints = upload.joints.map_or([0; 4], |joints| joints[index]);
            for joint in joints {
                vertices.extend_from_slice(&joint.to_le_bytes());
            }
            let weights = upload
                .weights
                .map_or([1.0, 0.0, 0.0, 0.0], |weights| weights[index]);
            let weight_sum = weights.iter().sum::<f32>();
            if !weights
                .iter()
                .all(|weight| weight.is_finite() && *weight >= 0.0)
                || weight_sum <= f32::EPSILON
            {
                return Err(WgpuSceneRendererError::InvalidMesh);
            }
            for weight in weights {
                vertices.extend_from_slice(&(weight / weight_sum).to_ne_bytes());
            }
        }
        let mut indices = Vec::with_capacity(upload.indices.len() * 4);
        for index in upload.indices {
            indices.extend_from_slice(&index.to_le_bytes());
        }
        let vertex = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-mesh-vertices"),
                contents: &vertices,
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-mesh-indices"),
                contents: &indices,
                usage: wgpu::BufferUsages::INDEX,
            });
        self.meshes.insert(
            upload.descriptor.id,
            GpuMesh {
                vertex,
                index,
                index_count: upload.descriptor.index_count,
                skinned: upload.descriptor.skinned,
            },
        );
        Ok(())
    }

    pub fn upsert_mesh_artifact(
        &mut self,
        artifact: &MeshArtifact3d,
    ) -> Result<(), WgpuSceneRendererError> {
        let upload = MeshUpload {
            descriptor: artifact.descriptor,
            positions: &artifact.positions,
            normals: &artifact.normals,
            texcoords: &artifact.texcoords,
            joints: artifact.joints.as_deref(),
            weights: artifact.weights.as_deref(),
            indices: &artifact.indices,
        };
        self.validate_mesh_upload(&upload)?;
        self.meshes.remove(&artifact.descriptor.id);
        self.upload_mesh(upload)
    }

    pub fn upload_skin_palette(
        &mut self,
        upload: SkinPaletteUpload3d<'_>,
    ) -> Result<(), WgpuSceneRendererError> {
        if upload.joint_matrices.is_empty()
            || upload.joint_matrices.len() > MAX_SKIN_JOINTS
            || upload
                .joint_matrices
                .iter()
                .flatten()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(WgpuSceneRendererError::InvalidSkinPalette);
        }
        self.skin_palettes.insert(
            upload.entity,
            create_skin_palette(&self.device, upload.joint_matrices),
        );
        Ok(())
    }

    pub fn register_material(
        &mut self,
        material: MaterialDescriptor,
    ) -> Result<(), WgpuSceneRendererError> {
        if material.id == 0 || material.pipeline_key == 0 {
            return Err(WgpuSceneRendererError::MissingMaterial);
        }
        if self.materials.contains_key(&material.id) {
            return Err(WgpuSceneRendererError::DuplicateMaterial);
        }
        if self.materials.len() == self.config.max_materials {
            return Err(WgpuSceneRendererError::Capacity);
        }
        self.materials.insert(material.id, material);
        Ok(())
    }

    pub fn upsert_material(
        &mut self,
        material: MaterialDescriptor,
    ) -> Result<(), WgpuSceneRendererError> {
        if material.id == 0 || material.pipeline_key == 0 {
            return Err(WgpuSceneRendererError::MissingMaterial);
        }
        if !self.materials.contains_key(&material.id)
            && self.materials.len() == self.config.max_materials
        {
            return Err(WgpuSceneRendererError::Capacity);
        }
        self.materials.insert(material.id, material);
        Ok(())
    }

    pub fn upload_texture(
        &mut self,
        upload: TextureUpload3d<'_>,
    ) -> Result<(), WgpuSceneRendererError> {
        let pixels = (upload.width as usize)
            .checked_mul(upload.height as usize)
            .ok_or(WgpuSceneRendererError::InvalidTexture)?;
        let bytes = pixels
            .checked_mul(4)
            .ok_or(WgpuSceneRendererError::InvalidTexture)?;
        if upload.id == 0
            || upload.width == 0
            || upload.height == 0
            || pixels > self.config.max_texture_pixels
            || upload.rgba8.len() != bytes
        {
            return Err(WgpuSceneRendererError::InvalidTexture);
        }
        if self.textures.contains_key(&upload.id) {
            return Err(WgpuSceneRendererError::DuplicateTexture);
        }
        if self.textures.len() == self.config.max_textures {
            return Err(WgpuSceneRendererError::Capacity);
        }
        let texture = create_material_texture(
            &self.device,
            &self.queue,
            upload.width,
            upload.height,
            upload.rgba8,
            upload.srgb,
        );
        self.textures.insert(upload.id, texture);
        Ok(())
    }

    pub fn upsert_texture(
        &mut self,
        upload: TextureUpload3d<'_>,
    ) -> Result<(), WgpuSceneRendererError> {
        let previous = self.textures.remove(&upload.id);
        match self.upload_texture(upload) {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Some(previous) = previous {
                    self.textures.insert(upload.id, previous);
                }
                Err(error)
            }
        }
    }

    pub fn register_material_wire(
        &mut self,
        bytes: &[u8],
        pipeline_key: u64,
    ) -> Result<(), WgpuSceneRendererError> {
        let material = decode_material_3d(bytes)
            .and_then(|material| material.material_descriptor(pipeline_key))
            .map_err(|_: Wire3dError| WgpuSceneRendererError::InvalidMaterialWire)?;
        self.register_material(material)
    }

    pub fn remove_mesh(&mut self, mesh: u64) -> bool {
        self.meshes.remove(&mesh).is_some()
    }

    pub fn remove_material(&mut self, material: u64) -> bool {
        self.materials.remove(&material).is_some()
    }

    pub fn remove_texture(&mut self, texture: u64) -> bool {
        self.textures.remove(&texture).is_some()
    }

    pub fn remove_skin_palette(&mut self, entity: RenderEntity) -> bool {
        self.skin_palettes.remove(&entity).is_some()
    }

    fn validate_mesh_upload(&self, upload: &MeshUpload<'_>) -> Result<(), WgpuSceneRendererError> {
        if upload.descriptor.id == 0
            || upload.positions.is_empty()
            || upload.positions.len() != upload.normals.len()
            || upload.positions.len() != upload.texcoords.len()
            || upload.joints.is_some() != upload.weights.is_some()
            || upload
                .joints
                .is_some_and(|joints| joints.len() != upload.positions.len())
            || upload
                .weights
                .is_some_and(|weights| weights.len() != upload.positions.len())
            || upload.descriptor.skinned != upload.joints.is_some()
            || upload.positions.len() > self.config.max_vertices_per_mesh
            || upload.indices.is_empty()
            || upload.indices.len() > self.config.max_indices_per_mesh
            || upload.descriptor.vertex_count as usize != upload.positions.len()
            || upload.descriptor.index_count as usize != upload.indices.len()
            || upload
                .indices
                .iter()
                .any(|index| *index as usize >= upload.positions.len())
            || upload
                .positions
                .iter()
                .flatten()
                .chain(upload.normals.iter().flatten())
                .chain(upload.texcoords.iter().flatten())
                .any(|value| !value.is_finite())
        {
            return Err(WgpuSceneRendererError::InvalidMesh);
        }
        Ok(())
    }

    fn ensure_temporal_history(&mut self, width: u32, height: u32) {
        if self
            .temporal_history
            .as_ref()
            .is_some_and(|history| history.width == width && history.height == height)
        {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-3d-temporal-history"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.temporal_history = Some(TemporalHistory {
            texture,
            view,
            width,
            height,
            initialized: false,
        });
    }

    pub fn render(
        &mut self,
        plan: &Render3dPlan,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        view_projection: [[f32; 4]; 4],
        clear_color: [f32; 4],
    ) -> Result<u64, WgpuSceneRendererError> {
        self.render_with_shadows(
            plan,
            target,
            width,
            height,
            view_projection,
            clear_color,
            [0; 3],
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_with_shadows(
        &mut self,
        plan: &Render3dPlan,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        view_projection: [[f32; 4]; 4],
        clear_color: [f32; 4],
        camera_position_milli: [i64; 3],
        cascades: &[ShadowCascadeRender<'_>],
    ) -> Result<u64, WgpuSceneRendererError> {
        self.render_composed_with_shadows(
            plan,
            &PortableFeaturePlan3d {
                scene_revision: 0,
                draws: Vec::new(),
                environment: None,
            },
            target,
            width,
            height,
            view_projection,
            clear_color,
            camera_position_milli,
            cascades,
            &[],
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_composed_with_shadows(
        &mut self,
        plan: &Render3dPlan,
        features: &PortableFeaturePlan3d,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        view_projection: [[f32; 4]; 4],
        clear_color: [f32; 4],
        camera_position_milli: [i64; 3],
        cascades: &[ShadowCascadeRender<'_>],
        render_features: &[RealizedFeature],
        depth_copy: Option<(&wgpu::Texture, wgpu::Origin3d)>,
    ) -> Result<u64, WgpuSceneRendererError> {
        self.last_frame_trace = None;
        let capture_clock = Instant::now();
        if width == 0
            || height == 0
            || cascades.len() > self.config.max_shadow_cascades
            || !valid_shadow_render_cascades(cascades)
            || !environment_matches_samples(features.environment, self.config.sample_count)
            || depth_copy.is_some() && self.config.sample_count != 1
        {
            return Err(WgpuSceneRendererError::InvalidConfig);
        }
        for packet in plan
            .opaque
            .iter()
            .chain(&plan.transparent)
            .chain(cascades.iter().flat_map(|cascade| cascade.draws))
            .map(|packet| (packet.mesh, packet.material))
            .chain(features.draws.iter().map(|draw| (draw.mesh, draw.material)))
        {
            if !self.meshes.contains_key(&packet.0) {
                return Err(WgpuSceneRendererError::MissingMesh);
            }
            if !self.materials.contains_key(&packet.1) {
                return Err(WgpuSceneRendererError::MissingMaterial);
            }
        }
        let shadow_layers =
            u32::try_from(cascades.len().max(1)).map_err(|_| WgpuSceneRendererError::Capacity)?;
        let shadow_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-3d-shadow-cascades"),
            size: wgpu::Extent3d {
                width: self.config.shadow_resolution,
                height: self.config.shadow_resolution,
                depth_or_array_layers: shadow_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_array_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("voplay-3d-shadow-array-view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(shadow_layers),
            ..Default::default()
        });
        let shadow_layer_views = (0..shadow_layers)
            .map(|layer| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("voplay-3d-shadow-layer-view"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        let shadow_frame =
            ShadowFrameUniform::new(camera_position_milli, cascades, features.environment);
        let mut prepared_opaque = plan
            .opaque
            .iter()
            .map(|packet| {
                self.prepare_draw(
                    packet,
                    view_projection,
                    plan,
                    &shadow_frame,
                    &shadow_array_view,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        for phase in [
            PortableFeaturePhase3d::Terrain,
            PortableFeaturePhase3d::Scatter,
            PortableFeaturePhase3d::DecalBeforeLighting,
        ] {
            prepared_opaque.extend(
                features
                    .draws
                    .iter()
                    .filter(|draw| draw.phase == phase)
                    .map(|draw| {
                        self.prepare_feature_draw(
                            draw,
                            view_projection,
                            plan,
                            &shadow_frame,
                            &shadow_array_view,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let mut prepared_transparent = plan
            .transparent
            .iter()
            .map(|packet| {
                self.prepare_draw(
                    packet,
                    view_projection,
                    plan,
                    &shadow_frame,
                    &shadow_array_view,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        for phase in [
            PortableFeaturePhase3d::Particle,
            PortableFeaturePhase3d::DecalAfterLighting,
        ] {
            prepared_transparent.extend(
                features
                    .draws
                    .iter()
                    .filter(|draw| draw.phase == phase)
                    .map(|draw| {
                        self.prepare_feature_draw(
                            draw,
                            view_projection,
                            plan,
                            &shadow_frame,
                            &shadow_array_view,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let prepared_shadows = cascades
            .iter()
            .map(|cascade| {
                cascade
                    .draws
                    .iter()
                    .map(|packet| self.prepare_shadow_draw(packet, cascade.light_view_projection))
                    .chain(
                        features
                            .draws
                            .iter()
                            .filter(|draw| draw.casts_shadow)
                            .map(|draw| {
                                self.prepare_feature_shadow_draw(
                                    draw,
                                    cascade.light_view_projection,
                                )
                            }),
                    )
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-3d-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.config.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if depth_copy.is_some() {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let multisampled_color = (self.config.sample_count > 1).then(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("voplay-3d-msaa-color"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: self.config.sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: self.color_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        });
        let multisampled_color_view = multisampled_color
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let post_mode = post_mode(features.environment);
        let bloom = features
            .environment
            .filter(|environment| environment.bloom_strength > 0);
        let scene_color = (post_mode.is_some() || bloom.is_some()).then(|| {
            create_post_texture(
                &self.device,
                self.color_format,
                width,
                height,
                "voplay-3d-post-source",
                true,
            )
        });
        let scene_color_view = scene_color
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        if post_mode == Some(PostMode::Taa) {
            self.ensure_temporal_history(width, height);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay-3d-frame"),
            });
        let data_feature_context = crate::data_feature_wgpu::WgpuDataFeatureContext {
            world_revision: plan.world_revision,
            view_revision: plan.view_revision,
            width,
            height,
            sample_count: self.config.sample_count,
            camera_position_milli,
            view_projection,
        };
        for (index, draws) in prepared_shadows.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay-3d-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_layer_views[index],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.shadow_pipeline);
            for draw in draws {
                let mesh = self
                    .meshes
                    .get(&draw.mesh)
                    .ok_or(WgpuSceneRendererError::MissingMesh)?;
                pass.set_bind_group(0, &draw.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
        if cascades.is_empty() {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay-3d-empty-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_layer_views[0],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        let shadow_finished_nanos = elapsed_nanos(capture_clock);
        let main_color_view = multisampled_color_view
            .as_ref()
            .or(scene_color_view.as_ref())
            .unwrap_or(target);
        let resolved_scene_view = scene_color_view.as_ref().unwrap_or(target);
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay-3d-main-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: main_color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color[0] as f64,
                            g: clear_color[1] as f64,
                            b: clear_color[2] as f64,
                            a: clear_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.data_features.encode_attachment(
            &self.device,
            &self.queue,
            &mut encoder,
            main_color_view,
            self.color_format,
            self.config.sample_count,
            render_features,
            AttachmentPoint::BeforeDepth,
            &data_feature_context,
        )?;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay-3d-opaque-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: main_color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for draw in &prepared_opaque {
                let mesh = self
                    .meshes
                    .get(&draw.mesh)
                    .ok_or(WgpuSceneRendererError::MissingMesh)?;
                let material = self
                    .materials
                    .get(&draw.material)
                    .ok_or(WgpuSceneRendererError::MissingMaterial)?;
                let blended = matches!(material.alpha, AlphaMode::Blend);
                let pipeline = match (blended, material.double_sided) {
                    (false, false) => &self.opaque_back,
                    (false, true) => &self.opaque_double,
                    (true, false) => &self.blend_back,
                    (true, true) => &self.blend_double,
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &draw.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
        for attachment in [
            AttachmentPoint::AfterOpaque,
            AttachmentPoint::BeforeTransparent,
        ] {
            self.data_features.encode_attachment(
                &self.device,
                &self.queue,
                &mut encoder,
                main_color_view,
                self.color_format,
                self.config.sample_count,
                render_features,
                attachment,
                &data_feature_context,
            )?;
        }
        let opaque_finished_nanos = elapsed_nanos(capture_clock);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voplay-3d-transparent-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: main_color_view,
                    resolve_target: multisampled_color_view
                        .as_ref()
                        .map(|_| resolved_scene_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for draw in &prepared_transparent {
                let mesh = self
                    .meshes
                    .get(&draw.mesh)
                    .ok_or(WgpuSceneRendererError::MissingMesh)?;
                let material = self
                    .materials
                    .get(&draw.material)
                    .ok_or(WgpuSceneRendererError::MissingMaterial)?;
                let blended = matches!(material.alpha, AlphaMode::Blend);
                let pipeline = match (blended, material.double_sided) {
                    (false, false) => &self.opaque_back,
                    (false, true) => &self.opaque_double,
                    (true, false) => &self.blend_back,
                    (true, true) => &self.blend_double,
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &draw.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
        self.data_features.encode_attachment(
            &self.device,
            &self.queue,
            &mut encoder,
            resolved_scene_view,
            self.color_format,
            1,
            render_features,
            AttachmentPoint::BeforePost,
            &data_feature_context,
        )?;
        let transparent_finished_nanos = elapsed_nanos(capture_clock);
        if let (Some(source), Some(source_view)) = (scene_color.as_ref(), scene_color_view.as_ref())
        {
            let bloom_textures = bloom.map(|_| {
                let first = create_post_texture(
                    &self.device,
                    self.color_format,
                    width,
                    height,
                    "voplay-3d-bloom-a",
                    false,
                );
                let second = create_post_texture(
                    &self.device,
                    self.color_format,
                    width,
                    height,
                    "voplay-3d-bloom-b",
                    false,
                );
                let first_view = first.create_view(&wgpu::TextureViewDescriptor::default());
                let second_view = second.create_view(&wgpu::TextureViewDescriptor::default());
                (first, first_view, second, second_view)
            });
            let aa_texture = (post_mode.is_some() && bloom.is_some()).then(|| {
                create_post_texture(
                    &self.device,
                    self.color_format,
                    width,
                    height,
                    "voplay-3d-aa-output",
                    false,
                )
            });
            let aa_view = aa_texture
                .as_ref()
                .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
            if post_mode == Some(PostMode::Taa)
                && self
                    .temporal_history
                    .as_ref()
                    .is_some_and(|history| !history.initialized)
            {
                copy_color_texture(
                    &mut encoder,
                    source,
                    &self.temporal_history.as_ref().unwrap().texture,
                    width,
                    height,
                );
                self.temporal_history.as_mut().unwrap().initialized = true;
            }
            let mut composite_source = source_view;
            if let Some(mode) = post_mode {
                let history_view = self
                    .temporal_history
                    .as_ref()
                    .map_or(source_view, |history| &history.view);
                let output = aa_view.as_ref().unwrap_or(target);
                encode_post_pass(
                    &self.device,
                    &mut encoder,
                    &self.post_pipeline,
                    &self.post_layout,
                    &self.post_sampler,
                    source_view,
                    history_view,
                    output,
                    width,
                    height,
                    match mode {
                        PostMode::Fxaa => 1.0,
                        PostMode::Taa => 2.0,
                    },
                    0.1,
                );
                composite_source = output;
                if mode == PostMode::Taa {
                    copy_color_texture(
                        &mut encoder,
                        source,
                        &self.temporal_history.as_ref().unwrap().texture,
                        width,
                        height,
                    );
                }
            }
            if let (Some(environment), Some((_, bloom_a, _, bloom_b))) =
                (bloom, bloom_textures.as_ref())
            {
                encode_post_pass(
                    &self.device,
                    &mut encoder,
                    &self.post_pipeline,
                    &self.post_layout,
                    &self.post_sampler,
                    source_view,
                    source_view,
                    bloom_a,
                    width,
                    height,
                    3.0,
                    environment.bloom_threshold as f32 / 65_535.0,
                );
                encode_post_pass(
                    &self.device,
                    &mut encoder,
                    &self.post_pipeline,
                    &self.post_layout,
                    &self.post_sampler,
                    bloom_a,
                    bloom_a,
                    bloom_b,
                    width,
                    height,
                    4.0,
                    0.0,
                );
                encode_post_pass(
                    &self.device,
                    &mut encoder,
                    &self.post_pipeline,
                    &self.post_layout,
                    &self.post_sampler,
                    bloom_b,
                    bloom_b,
                    bloom_a,
                    width,
                    height,
                    5.0,
                    0.0,
                );
                encode_post_pass(
                    &self.device,
                    &mut encoder,
                    &self.post_pipeline,
                    &self.post_layout,
                    &self.post_sampler,
                    composite_source,
                    bloom_a,
                    target,
                    width,
                    height,
                    6.0,
                    environment.bloom_strength as f32 / 65_535.0,
                );
            }
        }
        let post_finished_nanos = elapsed_nanos(capture_clock);
        for attachment in [AttachmentPoint::AfterPost, AttachmentPoint::Overlay] {
            self.data_features.encode_attachment(
                &self.device,
                &self.queue,
                &mut encoder,
                target,
                self.color_format,
                1,
                render_features,
                attachment,
                &data_feature_context,
            )?;
        }
        if let Some((destination, origin)) = depth_copy {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: destination,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let overlay_finished_nanos = elapsed_nanos(capture_clock);
        self.queue.submit([encoder.finish()]);
        let submit_finished_nanos = elapsed_nanos(capture_clock);
        let fence = self.next_fence;
        self.next_fence = self
            .next_fence
            .checked_add(1)
            .ok_or(WgpuSceneRendererError::FenceExhausted)?;
        let shadow_draws = prepared_shadows.iter().map(Vec::len).sum::<usize>();
        let post_draws = usize::from(post_mode.is_some()) + bloom.map_or(0, |_| 4);
        self.last_frame_trace = Some(WgpuFrameTrace3d {
            fence,
            nodes: vec![
                captured_node(
                    101,
                    "shadow",
                    0,
                    shadow_finished_nanos,
                    0x7368_6164_6f77,
                    shadow_draws,
                ),
                captured_node(
                    102,
                    "opaque",
                    shadow_finished_nanos,
                    opaque_finished_nanos,
                    0x6f70_6171_7565,
                    prepared_opaque.len(),
                ),
                captured_node(
                    103,
                    "transparent",
                    opaque_finished_nanos,
                    transparent_finished_nanos,
                    0x7472_616e_7370,
                    prepared_transparent.len(),
                ),
                captured_node(
                    104,
                    "post",
                    transparent_finished_nanos,
                    post_finished_nanos,
                    0x706f_7374,
                    post_draws,
                ),
                captured_node(
                    105,
                    "overlay",
                    post_finished_nanos,
                    overlay_finished_nanos,
                    0x6f76_6572_6c61_79,
                    render_features
                        .iter()
                        .filter(|feature| feature.attachment == AttachmentPoint::Overlay)
                        .count(),
                ),
                captured_node(
                    106,
                    "queue-submit",
                    overlay_finished_nanos,
                    submit_finished_nanos,
                    0x7175_6575_652d_7375,
                    0,
                ),
            ],
            attachments: vec![captured_attachment(
                1,
                "final-color",
                width,
                height,
                self.color_format,
            )],
            shader_diagnostics: render_features
                .iter()
                .map(|feature| WgpuShaderDiagnostic3d {
                    pipeline_hash: feature.pipeline_key.max(1),
                    severity: 1,
                    source_start: 0,
                    source_end: 0,
                    graph_node: feature_graph_node(feature.attachment),
                    message: format!(
                        "render feature {}:{} shader ABI validated and pipeline materialized",
                        feature.feature.0, feature.revision
                    ),
                })
                .collect(),
        });
        Ok(fence)
    }

    pub fn take_last_frame_trace(&mut self) -> Option<WgpuFrameTrace3d> {
        self.last_frame_trace.take()
    }

    fn prepare_draw(
        &self,
        packet: &DrawPacket3d,
        view_projection: [[f32; 4]; 4],
        plan: &Render3dPlan,
        shadow_frame: &ShadowFrameUniform,
        shadow_view: &wgpu::TextureView,
    ) -> Result<PreparedDraw, WgpuSceneRendererError> {
        let material = self
            .materials
            .get(&packet.material)
            .ok_or(WgpuSceneRendererError::MissingMaterial)?;
        let texture_views = self.material_texture_views(material)?;
        let skin_palette = self.skin_palette_buffer(packet.mesh, Some(packet.entity))?;
        let uniform_bytes =
            draw_uniform_bytes(view_projection, packet, material, plan, shadow_frame);
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-draw-uniform"),
                contents: &uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay-3d-draw-bind-group"),
            layout: &self.uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                texture_bind_group_entry(3, texture_views[0]),
                texture_bind_group_entry(4, texture_views[1]),
                texture_bind_group_entry(5, texture_views[2]),
                texture_bind_group_entry(6, texture_views[3]),
                texture_bind_group_entry(7, texture_views[4]),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: skin_palette.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedDraw {
            mesh: packet.mesh,
            material: packet.material,
            bind_group,
        })
    }

    fn prepare_feature_draw(
        &self,
        draw: &PortableFeatureDraw3d,
        view_projection: [[f32; 4]; 4],
        plan: &Render3dPlan,
        shadow_frame: &ShadowFrameUniform,
        shadow_view: &wgpu::TextureView,
    ) -> Result<PreparedDraw, WgpuSceneRendererError> {
        let material = self
            .materials
            .get(&draw.material)
            .ok_or(WgpuSceneRendererError::MissingMaterial)?;
        let texture_views = self.material_texture_views(material)?;
        let skin_palette = self.skin_palette_buffer(draw.mesh, None)?;
        let uniform_bytes = draw_uniform_bytes_model(
            view_projection,
            draw.model_milli,
            material,
            plan,
            shadow_frame,
        );
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-feature-draw-uniform"),
                contents: &uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay-3d-feature-draw-bind-group"),
            layout: &self.uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                texture_bind_group_entry(3, texture_views[0]),
                texture_bind_group_entry(4, texture_views[1]),
                texture_bind_group_entry(5, texture_views[2]),
                texture_bind_group_entry(6, texture_views[3]),
                texture_bind_group_entry(7, texture_views[4]),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: skin_palette.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedDraw {
            mesh: draw.mesh,
            material: draw.material,
            bind_group,
        })
    }

    fn prepare_shadow_draw(
        &self,
        packet: &DrawPacket3d,
        light_view_projection: [[f32; 4]; 4],
    ) -> Result<PreparedShadowDraw, WgpuSceneRendererError> {
        let material = self
            .materials
            .get(&packet.material)
            .ok_or(WgpuSceneRendererError::MissingMaterial)?;
        let base_color_view =
            self.material_texture_view(material.textures[0], &self.fallback_white)?;
        let skin_palette = self.skin_palette_buffer(packet.mesh, Some(packet.entity))?;
        let uniform_bytes = shadow_uniform_bytes(light_view_projection, packet, material);
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-shadow-draw-uniform"),
                contents: &uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay-3d-shadow-draw-bind-group"),
            layout: &self.shadow_uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                texture_bind_group_entry(1, base_color_view),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: skin_palette.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedShadowDraw {
            mesh: packet.mesh,
            bind_group,
        })
    }

    fn prepare_feature_shadow_draw(
        &self,
        draw: &PortableFeatureDraw3d,
        light_view_projection: [[f32; 4]; 4],
    ) -> Result<PreparedShadowDraw, WgpuSceneRendererError> {
        let material = self
            .materials
            .get(&draw.material)
            .ok_or(WgpuSceneRendererError::MissingMaterial)?;
        let base_color_view =
            self.material_texture_view(material.textures[0], &self.fallback_white)?;
        let skin_palette = self.skin_palette_buffer(draw.mesh, None)?;
        let uniform_bytes =
            shadow_uniform_bytes_model(light_view_projection, draw.model_milli, material);
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("voplay-3d-feature-shadow-uniform"),
                contents: &uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay-3d-feature-shadow-bind-group"),
            layout: &self.shadow_uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                texture_bind_group_entry(1, base_color_view),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.material_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: skin_palette.as_entire_binding(),
                },
            ],
        });
        Ok(PreparedShadowDraw {
            mesh: draw.mesh,
            bind_group,
        })
    }

    fn material_texture_views(
        &self,
        material: &MaterialDescriptor,
    ) -> Result<[&wgpu::TextureView; 5], WgpuSceneRendererError> {
        Ok([
            self.material_texture_view(material.textures[0], &self.fallback_white)?,
            self.material_texture_view(material.textures[1], &self.fallback_white)?,
            self.material_texture_view(material.textures[2], &self.fallback_normal)?,
            self.material_texture_view(material.textures[3], &self.fallback_white)?,
            self.material_texture_view(material.textures[4], &self.fallback_white)?,
        ])
    }

    fn material_texture_view<'a>(
        &'a self,
        id: u64,
        fallback: &'a GpuMaterialTexture,
    ) -> Result<&'a wgpu::TextureView, WgpuSceneRendererError> {
        if id == 0 {
            return Ok(&fallback.view);
        }
        self.textures
            .get(&id)
            .map(|texture| &texture.view)
            .ok_or(WgpuSceneRendererError::MissingTexture)
    }

    fn skin_palette_buffer(
        &self,
        mesh: u64,
        entity: Option<RenderEntity>,
    ) -> Result<&wgpu::Buffer, WgpuSceneRendererError> {
        let mesh = self
            .meshes
            .get(&mesh)
            .ok_or(WgpuSceneRendererError::MissingMesh)?;
        if !mesh.skinned {
            return Ok(&self.identity_skin_palette.buffer);
        }
        entity
            .and_then(|entity| self.skin_palettes.get(&entity))
            .map(|palette| &palette.buffer)
            .ok_or(WgpuSceneRendererError::MissingSkinPalette)
    }
}

const fn feature_graph_node(attachment: AttachmentPoint) -> u32 {
    match attachment {
        AttachmentPoint::BeforeDepth | AttachmentPoint::AfterOpaque => 102,
        AttachmentPoint::BeforeTransparent => 103,
        AttachmentPoint::BeforePost => 104,
        AttachmentPoint::AfterPost | AttachmentPoint::Overlay => 105,
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    uniform_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    blended: bool,
    cull_mode: Option<wgpu::Face>,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Uint16x4,
        4 => Float32x4
    ];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voplay-3d-pipeline-layout"),
        bind_group_layouts: &[uniform_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voplay-3d-mesh-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 56,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: !blended,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: blended.then_some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn create_shadow_pipeline(
    device: &wgpu::Device,
    uniform_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Uint16x4,
        4 => Float32x4
    ];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voplay-3d-shadow-pipeline-layout"),
        bind_group_layouts: &[uniform_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voplay-3d-shadow-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 56,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Front),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[],
        }),
        multiview: None,
        cache: None,
    })
}

fn texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn skin_palette_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_bind_group_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn create_solid_material_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
) -> GpuMaterialTexture {
    create_material_texture(device, queue, 1, 1, &rgba, false)
}

fn create_material_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba8: &[u8],
    srgb: bool,
) -> GpuMaterialTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay-3d-material-texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba8,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuMaterialTexture {
        _texture: texture,
        view,
    }
}

fn create_skin_palette(device: &wgpu::Device, joint_matrices: &[[[f32; 4]; 4]]) -> GpuSkinPalette {
    let mut bytes = Vec::with_capacity(joint_matrices.len() * 64);
    for matrix in joint_matrices {
        for column in matrix {
            for value in column {
                bytes.extend_from_slice(&value.to_ne_bytes());
            }
        }
    }
    GpuSkinPalette {
        buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voplay-3d-skin-palette"),
            contents: &bytes,
            usage: wgpu::BufferUsages::STORAGE,
        }),
    }
}

fn create_post_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voplay-3d-post-pipeline-layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voplay-3d-post-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

struct ShadowFrameUniform {
    matrices: [[[f32; 4]; 4]; 4],
    splits: [f32; 4],
    camera: [f32; 3],
    count: u32,
    fog: [f32; 4],
    environment: [f32; 4],
    post: [f32; 4],
}

impl ShadowFrameUniform {
    fn new(
        camera_position_milli: [i64; 3],
        cascades: &[ShadowCascadeRender<'_>],
        environment: Option<WireEnvironment3d>,
    ) -> Self {
        let mut matrices = [[[0.0; 4]; 4]; 4];
        for matrix in &mut matrices {
            *matrix = identity_matrix();
        }
        let mut splits = [0.0; 4];
        for (index, cascade) in cascades.iter().enumerate() {
            matrices[index] = cascade.light_view_projection;
            splits[index] = cascade.descriptor.far_milli as f32 / 1_000.0;
        }
        let (fog, environment, post) =
            environment.map_or(([0.0; 4], [0.0, 1.0, 1.0, 0.0], [1.0; 4]), |environment| {
                (
                    [
                        environment.fog_color[0] as f32 / 65_535.0,
                        environment.fog_color[1] as f32 / 65_535.0,
                        environment.fog_color[2] as f32 / 65_535.0,
                        environment.fog_start_milli as f32 / 1_000.0,
                    ],
                    [
                        environment.fog_end_milli as f32 / 1_000.0,
                        2.0_f32.powf(environment.exposure_milli as f32 / 1_000.0),
                        match environment.tone_map {
                            WireToneMap3d::Linear => 1.0,
                            WireToneMap3d::Reinhard => 2.0,
                            WireToneMap3d::Aces => 3.0,
                        },
                        environment.bloom_strength as f32 / 65_535.0,
                    ],
                    [
                        environment.bloom_threshold as f32 / 65_535.0,
                        match environment.anti_alias {
                            WireAntiAlias3d::None => 1.0,
                            WireAntiAlias3d::Fxaa => 2.0,
                            WireAntiAlias3d::Taa => 3.0,
                            WireAntiAlias3d::Msaa => 4.0,
                        },
                        environment.msaa_samples as f32,
                        0.0,
                    ],
                )
            });
        Self {
            matrices,
            splits,
            camera: [
                camera_position_milli[0] as f32 / 1_000.0,
                camera_position_milli[1] as f32 / 1_000.0,
                camera_position_milli[2] as f32 / 1_000.0,
            ],
            count: cascades.len() as u32,
            fog,
            environment,
            post,
        }
    }
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn valid_shadow_render_cascades(cascades: &[ShadowCascadeRender<'_>]) -> bool {
    cascades.iter().enumerate().all(|(index, cascade)| {
        cascade.descriptor.index == index as u32
            && cascade.descriptor.near_milli < cascade.descriptor.far_milli
            && cascade.descriptor.texel_world_milli != 0
            && cascade
                .light_view_projection
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            && (index == 0
                || cascades[index - 1].descriptor.far_milli <= cascade.descriptor.near_milli)
    })
}

fn environment_matches_samples(environment: Option<WireEnvironment3d>, sample_count: u32) -> bool {
    match environment.map(|environment| environment.anti_alias) {
        Some(WireAntiAlias3d::Msaa) => {
            environment.is_some_and(|environment| environment.msaa_samples == sample_count)
        }
        _ => sample_count == 1,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostMode {
    Fxaa,
    Taa,
}

fn post_mode(environment: Option<WireEnvironment3d>) -> Option<PostMode> {
    match environment.map(|environment| environment.anti_alias) {
        Some(WireAntiAlias3d::Fxaa) => Some(PostMode::Fxaa),
        Some(WireAntiAlias3d::Taa) => Some(PostMode::Taa),
        _ => None,
    }
}

fn post_uniform_bytes(width: u32, height: u32, mode: f32, parameter: f32) -> Vec<u8> {
    [1.0 / width as f32, 1.0 / height as f32, mode, parameter]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect()
}

fn create_post_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    label: &'static str,
    copy_source: bool,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | if copy_source {
                wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    })
}

#[allow(clippy::too_many_arguments)]
fn encode_post_pass(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    current: &wgpu::TextureView,
    auxiliary: &wgpu::TextureView,
    target: &wgpu::TextureView,
    width: u32,
    height: u32,
    mode: f32,
    parameter: f32,
) {
    let uniform_bytes = post_uniform_bytes(width, height, mode, parameter);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("voplay-3d-post-uniform"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("voplay-3d-post-bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(current),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(auxiliary),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("voplay-3d-post-pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}

fn copy_color_texture(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    width: u32,
    height: u32,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

fn draw_uniform_bytes(
    view_projection: [[f32; 4]; 4],
    packet: &DrawPacket3d,
    material: &MaterialDescriptor,
    plan: &Render3dPlan,
    shadow_frame: &ShadowFrameUniform,
) -> Vec<u8> {
    draw_uniform_bytes_model(
        view_projection,
        packet.model_milli,
        material,
        plan,
        shadow_frame,
    )
}

fn draw_uniform_bytes_model(
    view_projection: [[f32; 4]; 4],
    model_milli: [i64; 12],
    material: &MaterialDescriptor,
    plan: &Render3dPlan,
    shadow_frame: &ShadowFrameUniform,
) -> Vec<u8> {
    let model = model_matrix(model_milli);
    let lights = select_gpu_lights(&plan.lights, model_milli);
    let mut floats = Vec::with_capacity(332);
    for column in view_projection {
        floats.extend_from_slice(&column);
    }
    for column in model {
        floats.extend_from_slice(&column);
    }
    floats.extend_from_slice(&[
        material.base_color[0] as f32 / 255.0,
        material.base_color[1] as f32 / 255.0,
        material.base_color[2] as f32 / 255.0,
        material.base_color[3] as f32 / 255.0,
    ]);
    floats.extend_from_slice(&[
        if material.unlit { 1.0 } else { 0.0 },
        match material.alpha {
            AlphaMode::Mask { cutoff_q16 } => cutoff_q16 as f32 / 65_535.0,
            _ => 0.0,
        },
        material.metallic_q16 as f32 / 65_535.0,
        material.roughness_q16 as f32 / 65_535.0,
    ]);
    floats.extend_from_slice(&[
        material.emissive_q16[0] as f32 / 65_535.0,
        material.emissive_q16[1] as f32 / 65_535.0,
        material.emissive_q16[2] as f32 / 65_535.0,
        0.0,
    ]);
    floats.extend_from_slice(&[
        material.normal_scale_q16 as f32 / 65_535.0,
        material.occlusion_strength_q16 as f32 / 65_535.0,
        0.0,
        0.0,
    ]);
    for index in 0..MAX_GPU_LIGHTS {
        if let Some(light) = lights.get(index) {
            let (direction, color, position) = gpu_light_vectors(light);
            floats.extend_from_slice(&direction);
            floats.extend_from_slice(&color);
            floats.extend_from_slice(&position);
        } else {
            floats.extend_from_slice(&[0.0; 12]);
        }
    }
    floats.extend_from_slice(&[lights.len() as f32, 0.0, 0.0, 0.0]);
    for matrix in shadow_frame.matrices {
        for column in matrix {
            floats.extend_from_slice(&column);
        }
    }
    floats.extend_from_slice(&shadow_frame.splits);
    floats.extend_from_slice(&[
        shadow_frame.camera[0],
        shadow_frame.camera[1],
        shadow_frame.camera[2],
        shadow_frame.count as f32,
    ]);
    floats.extend_from_slice(&shadow_frame.fog);
    floats.extend_from_slice(&shadow_frame.environment);
    floats.extend_from_slice(&shadow_frame.post);
    let mut bytes = Vec::with_capacity(floats.len() * 4);
    for value in floats {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn select_gpu_lights(lights: &[LightPacket3d], model_milli: [i64; 12]) -> Vec<&LightPacket3d> {
    let position = [model_milli[3], model_milli[7], model_milli[11]];
    let mut selected = lights.iter().collect::<Vec<_>>();
    selected.sort_by_key(|light| {
        let kind = match light.descriptor.kind {
            LightKind::Directional => 0,
            LightKind::Point => 1,
            LightKind::Spot { .. } => 2,
        };
        let distance = if kind == 0 {
            0
        } else {
            light
                .position_milli
                .iter()
                .zip(position)
                .map(|(light, object)| light.saturating_sub(object).unsigned_abs())
                .fold(0_u128, |sum, axis| {
                    sum.saturating_add(u128::from(axis).saturating_mul(u128::from(axis)))
                })
        };
        (kind, distance, light.entity)
    });
    selected.truncate(MAX_GPU_LIGHTS);
    selected
}

fn gpu_light_vectors(light: &LightPacket3d) -> ([f32; 4], [f32; 4], [f32; 4]) {
    let (kind, cone_cosine) = match light.descriptor.kind {
        LightKind::Directional => (0.0, -1.0),
        LightKind::Point => (1.0, -1.0),
        LightKind::Spot { outer_angle_q16 } => (
            2.0,
            (outer_angle_q16 as f32 / 65_535.0 * std::f32::consts::PI).cos(),
        ),
    };
    let mut direction = normalized_direction(light.direction_milli);
    direction[3] = cone_cosine;
    let intensity = light.descriptor.intensity_milli as f32 / 1_000.0;
    (
        direction,
        [
            light.descriptor.color[0] as f32 / 65_535.0 * intensity,
            light.descriptor.color[1] as f32 / 65_535.0 * intensity,
            light.descriptor.color[2] as f32 / 65_535.0 * intensity,
            light.descriptor.range_milli as f32 / 1_000.0,
        ],
        [
            light.position_milli[0] as f32 / 1_000.0,
            light.position_milli[1] as f32 / 1_000.0,
            light.position_milli[2] as f32 / 1_000.0,
            kind,
        ],
    )
}

fn shadow_uniform_bytes(
    light_view_projection: [[f32; 4]; 4],
    packet: &DrawPacket3d,
    material: &MaterialDescriptor,
) -> Vec<u8> {
    shadow_uniform_bytes_model(light_view_projection, packet.model_milli, material)
}

fn shadow_uniform_bytes_model(
    light_view_projection: [[f32; 4]; 4],
    model_milli: [i64; 12],
    material: &MaterialDescriptor,
) -> Vec<u8> {
    let mut floats = Vec::with_capacity(36);
    for column in light_view_projection {
        floats.extend_from_slice(&column);
    }
    for column in model_matrix(model_milli) {
        floats.extend_from_slice(&column);
    }
    floats.extend_from_slice(&[
        material.base_color[3] as f32 / 255.0,
        match material.alpha {
            AlphaMode::Mask { cutoff_q16 } => cutoff_q16 as f32 / 65_535.0,
            _ => 0.0,
        },
        0.0,
        0.0,
    ]);
    let mut bytes = Vec::with_capacity(floats.len() * 4);
    for value in floats {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn model_matrix(transform: [i64; 12]) -> [[f32; 4]; 4] {
    let scale = |value: i64| value as f32 / 1_000.0;
    [
        [
            scale(transform[0]),
            scale(transform[4]),
            scale(transform[8]),
            0.0,
        ],
        [
            scale(transform[1]),
            scale(transform[5]),
            scale(transform[9]),
            0.0,
        ],
        [
            scale(transform[2]),
            scale(transform[6]),
            scale(transform[10]),
            0.0,
        ],
        [
            scale(transform[3]),
            scale(transform[7]),
            scale(transform[11]),
            1.0,
        ],
    ]
}

fn normalized_direction(position_milli: [i64; 3]) -> [f32; 4] {
    let x = position_milli[0] as f32;
    let y = position_milli[1] as f32;
    let z = position_milli[2] as f32;
    let length = (x * x + y * y + z * z).sqrt();
    if length <= f32::EPSILON {
        [0.3, -0.8, -0.5, 0.0]
    } else {
        [x / length, y / length, z / length, 0.0]
    }
}

const SCENE_SHADER: &str = r#"
struct LightUniform {
    direction: vec4<f32>,
    color: vec4<f32>,
    position: vec4<f32>,
}

struct DrawUniform {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    material: vec4<f32>,
    emissive: vec4<f32>,
    surface: vec4<f32>,
    lights: array<LightUniform, 16>,
    light_meta: vec4<f32>,
    shadow_matrices: array<mat4x4<f32>, 4>,
    shadow_splits: vec4<f32>,
    shadow_info: vec4<f32>,
    fog: vec4<f32>,
    environment: vec4<f32>,
    post: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> draw: DrawUniform;
@group(0) @binding(1)
var shadow_texture: texture_depth_2d_array;
@group(0) @binding(2)
var shadow_sampler: sampler_comparison;
@group(0) @binding(3)
var base_color_texture: texture_2d<f32>;
@group(0) @binding(4)
var metallic_roughness_texture: texture_2d<f32>;
@group(0) @binding(5)
var normal_texture: texture_2d<f32>;
@group(0) @binding(6)
var occlusion_texture: texture_2d<f32>;
@group(0) @binding(7)
var emissive_texture: texture_2d<f32>;
@group(0) @binding(8)
var material_sampler: sampler;
@group(0) @binding(9)
var<storage, read> skin_palette: array<mat4x4<f32>>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) texcoord: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) texcoord: vec2<f32>,
}

fn skin_matrix(input: VertexInput) -> mat4x4<f32> {
    return
        skin_palette[input.joints.x] * input.weights.x +
        skin_palette[input.joints.y] * input.weights.y +
        skin_palette[input.joints.z] * input.weights.z +
        skin_palette[input.joints.w] * input.weights.w;
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let skin = skin_matrix(input);
    let world = draw.model * skin * vec4<f32>(input.position, 1.0);
    output.position = draw.view_projection * world;
    output.world_normal =
        normalize((draw.model * skin * vec4<f32>(input.normal, 0.0)).xyz);
    output.world_position = world.xyz;
    output.texcoord = input.texcoord;
    return output;
}

fn sample_shadow(world_position: vec3<f32>) -> f32 {
    let cascade_count = u32(draw.shadow_info.w);
    if cascade_count == 0u {
        return 1.0;
    }
    let view_distance = distance(world_position, draw.shadow_info.xyz);
    var cascade = cascade_count - 1u;
    for (var index = 0u; index < cascade_count; index++) {
        if view_distance <= draw.shadow_splits[index] {
            cascade = index;
            break;
        }
    }
    let clip = draw.shadow_matrices[cascade] * vec4<f32>(world_position, 1.0);
    if abs(clip.w) <= 0.000001 {
        return 1.0;
    }
    let projected = clip.xyz / clip.w;
    let uv = vec2<f32>(projected.x * 0.5 + 0.5, 0.5 - projected.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 ||
        projected.z < 0.0 || projected.z > 1.0 {
        return 1.0;
    }
    return textureSampleCompare(
        shadow_texture,
        shadow_sampler,
        uv,
        i32(cascade),
        projected.z - 0.001,
    );
}

fn aces_tone_map(color: vec3<f32>) -> vec3<f32> {
    let numerator = color * (2.51 * color + vec3<f32>(0.03));
    let denominator = color * (2.43 * color + vec3<f32>(0.59)) + vec3<f32>(0.14);
    return clamp(numerator / denominator, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn tone_map(color: vec3<f32>) -> vec3<f32> {
    if draw.environment.z < 1.5 {
        return clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if draw.environment.z < 2.5 {
        return color / (vec3<f32>(1.0) + color);
    }
    return aces_tone_map(color);
}

fn mapped_normal(input: VertexOutput) -> vec3<f32> {
    let geometric_normal = normalize(input.world_normal);
    let position_dx = dpdx(input.world_position);
    let position_dy = dpdy(input.world_position);
    let uv_dx = dpdx(input.texcoord);
    let uv_dy = dpdy(input.texcoord);
    let determinant = uv_dx.x * uv_dy.y - uv_dx.y * uv_dy.x;
    if abs(determinant) < 0.000001 {
        return geometric_normal;
    }
    let tangent = normalize((position_dx * uv_dy.y - position_dy * uv_dx.y) / determinant);
    let bitangent = normalize((-position_dx * uv_dy.x + position_dy * uv_dx.x) / determinant);
    var sampled = textureSample(normal_texture, material_sampler, input.texcoord).xyz * 2.0 -
        vec3<f32>(1.0);
    sampled.x *= draw.surface.x;
    sampled.y *= draw.surface.x;
    return normalize(mat3x3<f32>(tangent, bitangent, geometric_normal) * sampled);
}

fn evaluate_light(
    light: LightUniform,
    world_position: vec3<f32>,
    normal: vec3<f32>,
    diffuse_color: vec3<f32>,
    specular_color: vec3<f32>,
    specular_power: f32,
    shadow: f32,
) -> vec3<f32> {
    var light_vector = -normalize(light.direction.xyz);
    var attenuation = 1.0;
    if light.position.w > 0.5 {
        let delta = light.position.xyz - world_position;
        let light_distance = max(length(delta), 0.0001);
        light_vector = delta / light_distance;
        if light.color.a > 0.0 {
            let range_factor = clamp(1.0 - light_distance / light.color.a, 0.0, 1.0);
            attenuation = range_factor * range_factor;
        }
        if light.position.w > 1.5 {
            let cone = dot(-light_vector, normalize(light.direction.xyz));
            attenuation *= smoothstep(
                light.direction.w,
                min(1.0, light.direction.w + 0.05),
                cone,
            );
        }
    }
    let facing = max(dot(normal, light_vector), 0.0);
    let diffuse = diffuse_color * light.color.rgb * facing;
    let specular = specular_color * pow(facing, specular_power) * light.color.rgb;
    return (diffuse + specular) * attenuation * shadow;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let base_color =
        draw.base_color * textureSample(base_color_texture, material_sampler, input.texcoord);
    if draw.material.y > 0.0 && base_color.a < draw.material.y {
        discard;
    }
    let metallic_roughness =
        textureSample(metallic_roughness_texture, material_sampler, input.texcoord);
    let normal = mapped_normal(input);
    let occlusion = mix(
        1.0,
        textureSample(occlusion_texture, material_sampler, input.texcoord).r,
        draw.surface.y,
    );
    let emissive =
        draw.emissive.rgb * textureSample(emissive_texture, material_sampler, input.texcoord).rgb;
    let roughness = max(draw.material.w * metallic_roughness.g, 0.04);
    let metallic = draw.material.z * metallic_roughness.b;
    let diffuse_color = base_color.rgb * (1.0 - metallic);
    let specular_power = mix(128.0, 4.0, roughness);
    let specular_color = mix(vec3<f32>(0.04), base_color.rgb, metallic);
    var lit = diffuse_color * vec3<f32>(0.08) * occlusion + emissive;
    let light_count = min(u32(draw.light_meta.x), 16u);
    for (var index = 0u; index < light_count; index++) {
        let light_shadow = select(
            1.0,
            sample_shadow(input.world_position),
            index == 0u && draw.lights[index].position.w < 0.5,
        );
        lit += evaluate_light(
            draw.lights[index],
            input.world_position,
            normal,
            diffuse_color,
            specular_color,
            specular_power,
            light_shadow,
        );
    }
    var rgb = select(lit, base_color.rgb + emissive, draw.material.x > 0.5);
    rgb *= draw.environment.y;
    rgb = tone_map(rgb);
    if draw.environment.x > draw.fog.w {
        let view_distance = distance(input.world_position, draw.shadow_info.xyz);
        let fog_factor = smoothstep(draw.fog.w, draw.environment.x, view_distance);
        rgb = mix(rgb, draw.fog.rgb, fog_factor);
    }
    return vec4<f32>(rgb, base_color.a);
}
"#;

const SHADOW_SHADER: &str = r#"
struct ShadowUniform {
    light_view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    alpha: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> shadow: ShadowUniform;
@group(0) @binding(1)
var base_color_texture: texture_2d<f32>;
@group(0) @binding(2)
var material_sampler: sampler;
@group(0) @binding(3)
var<storage, read> skin_palette: array<mat4x4<f32>>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) texcoord: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let skin =
        skin_palette[input.joints.x] * input.weights.x +
        skin_palette[input.joints.y] * input.weights.y +
        skin_palette[input.joints.z] * input.weights.z +
        skin_palette[input.joints.w] * input.weights.w;
    output.position =
        shadow.light_view_projection * shadow.model * skin * vec4<f32>(input.position, 1.0);
    output.texcoord = input.texcoord;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) {
    let alpha =
        shadow.alpha.x * textureSample(base_color_texture, material_sampler, input.texcoord).a;
    if shadow.alpha.y > 0.0 && alpha < shadow.alpha.y {
        discard;
    }
}
"#;

const POST_SHADER: &str = r#"
struct PostUniform {
    inverse_size_mode_blend: vec4<f32>,
}

@group(0) @binding(0)
var current_texture: texture_2d<f32>;
@group(0) @binding(1)
var history_texture: texture_2d<f32>;
@group(0) @binding(2)
var post_sampler: sampler;
@group(0) @binding(3)
var<uniform> post: PostUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = positions[index] * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return output;
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.299, 0.587, 0.114));
}

fn fxaa(uv: vec2<f32>) -> vec4<f32> {
    let inverse_size = post.inverse_size_mode_blend.xy;
    let northwest = textureSample(
        current_texture, post_sampler, uv + inverse_size * vec2<f32>(-1.0, -1.0));
    let northeast = textureSample(
        current_texture, post_sampler, uv + inverse_size * vec2<f32>(1.0, -1.0));
    let southwest = textureSample(
        current_texture, post_sampler, uv + inverse_size * vec2<f32>(-1.0, 1.0));
    let southeast = textureSample(
        current_texture, post_sampler, uv + inverse_size * vec2<f32>(1.0, 1.0));
    let center = textureSample(current_texture, post_sampler, uv);
    let direction = vec2<f32>(
        -((luma(northwest.rgb) + luma(northeast.rgb)) -
          (luma(southwest.rgb) + luma(southeast.rgb))),
        (luma(northwest.rgb) + luma(southwest.rgb)) -
        (luma(northeast.rgb) + luma(southeast.rgb)),
    );
    let reduction = max(
        (luma(northwest.rgb) + luma(northeast.rgb) +
         luma(southwest.rgb) + luma(southeast.rgb)) * 0.03125,
        0.0078125,
    );
    let reciprocal = 1.0 / (min(abs(direction.x), abs(direction.y)) + reduction);
    let span = clamp(direction * reciprocal, vec2<f32>(-8.0), vec2<f32>(8.0)) *
        inverse_size;
    let first = 0.5 * (
        textureSample(current_texture, post_sampler, uv + span * (1.0 / 3.0 - 0.5)) +
        textureSample(current_texture, post_sampler, uv + span * (2.0 / 3.0 - 0.5)));
    let second = first * 0.5 + 0.25 * (
        textureSample(current_texture, post_sampler, uv + span * -0.5) +
        textureSample(current_texture, post_sampler, uv + span * 0.5));
    let minimum = min(
        luma(center.rgb),
        min(min(luma(northwest.rgb), luma(northeast.rgb)),
            min(luma(southwest.rgb), luma(southeast.rgb))),
    );
    let maximum = max(
        luma(center.rgb),
        max(max(luma(northwest.rgb), luma(northeast.rgb)),
            max(luma(southwest.rgb), luma(southeast.rgb))),
    );
    let candidate = luma(second.rgb);
    return select(second, first, candidate < minimum || candidate > maximum);
}

fn taa(uv: vec2<f32>) -> vec4<f32> {
    let inverse_size = post.inverse_size_mode_blend.xy;
    let current = textureSample(current_texture, post_sampler, uv);
    let left = textureSample(
        current_texture, post_sampler, uv - vec2<f32>(inverse_size.x, 0.0));
    let right = textureSample(
        current_texture, post_sampler, uv + vec2<f32>(inverse_size.x, 0.0));
    let up = textureSample(
        current_texture, post_sampler, uv - vec2<f32>(0.0, inverse_size.y));
    let down = textureSample(
        current_texture, post_sampler, uv + vec2<f32>(0.0, inverse_size.y));
    let minimum = min(current, min(min(left, right), min(up, down)));
    let maximum = max(current, max(max(left, right), max(up, down)));
    let history = clamp(
        textureSample(history_texture, post_sampler, uv),
        minimum,
        maximum,
    );
    return mix(history, current, post.inverse_size_mode_blend.w);
}

fn bloom_threshold(uv: vec2<f32>) -> vec4<f32> {
    let color = textureSample(current_texture, post_sampler, uv);
    let brightness = max(color.r, max(color.g, color.b));
    let contribution = smoothstep(
        post.inverse_size_mode_blend.w,
        min(1.0, post.inverse_size_mode_blend.w + 0.1),
        brightness,
    );
    return vec4<f32>(color.rgb * contribution, 1.0);
}

fn bloom_blur(uv: vec2<f32>, direction: vec2<f32>) -> vec4<f32> {
    let offset = post.inverse_size_mode_blend.xy * direction;
    var color = textureSample(current_texture, post_sampler, uv) * 0.227027;
    color += textureSample(current_texture, post_sampler, uv + offset * 1.384615) * 0.316216;
    color += textureSample(current_texture, post_sampler, uv - offset * 1.384615) * 0.316216;
    color += textureSample(current_texture, post_sampler, uv + offset * 3.230769) * 0.070270;
    color += textureSample(current_texture, post_sampler, uv - offset * 3.230769) * 0.070270;
    return color;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let mode = post.inverse_size_mode_blend.z;
    if mode < 1.5 {
        return fxaa(input.uv);
    }
    if mode < 2.5 {
        return taa(input.uv);
    }
    if mode < 3.5 {
        return bloom_threshold(input.uv);
    }
    if mode < 4.5 {
        return bloom_blur(input.uv, vec2<f32>(1.0, 0.0));
    }
    if mode < 5.5 {
        return bloom_blur(input.uv, vec2<f32>(0.0, 1.0));
    }
    let current = textureSample(current_texture, post_sampler, input.uv);
    let bloom = textureSample(history_texture, post_sampler, input.uv);
    return vec4<f32>(
        current.rgb + bloom.rgb * post.inverse_size_mode_blend.w,
        current.a,
    );
}
"#;
