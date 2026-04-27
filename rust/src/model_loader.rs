//! glTF/GLB model loader.
//!
//! Loads 3D models via the `gltf` crate, extracts mesh geometry
//! (position + normal + UV + tangent + vertex color), and uploads to GPU buffers.
//! Materials preserve the core glTF PBR texture slots used by the renderer.

use crate::animation::{
    self, AnimationChannel, AnimationClip, AnimationClipInfo, AnimationInterpolation,
    AnimationProperty, Joint, ModelAnimationInfo, Skeleton, Transform, MAX_JOINTS,
};
use crate::file_io;
use crate::material::{
    MaterialSamplerKey, MATERIAL_FILTER_LINEAR, MATERIAL_FILTER_NEAREST, MATERIAL_WRAP_CLAMP,
    MATERIAL_WRAP_MIRROR, MATERIAL_WRAP_REPEAT,
};
use crate::math3d::{self, Mat4, Quat, Vec3, MAT4_IDENTITY};
use crate::primitives;
use crate::texture::{TextureId, TextureManager};
use base64::{engine::general_purpose, Engine as _};
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Opaque model handle matching Vo's ModelID.
pub type ModelId = u32;

/// A level node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelNodeKind {
    Entity,
    Terrain,
}

pub struct LevelNodeTerrain {
    pub rows: u32,
    pub cols: u32,
    pub scale: [f32; 3],
    pub heights: Vec<f32>,
    pub layer: u16,
    pub mask: u16,
    pub friction: f32,
    pub restitution: f32,
}

pub struct LevelNode {
    pub kind: LevelNodeKind,
    pub name: String,
    pub model_id: ModelId,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub terrain: Option<LevelNodeTerrain>,
}

struct ParsedSkinning {
    joint_indices: Vec<[u16; 4]>,
    joint_weights: Vec<[f32; 4]>,
}

struct ParsedPrimitive {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    tangents: Vec<[f32; 4]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
    material: MeshMaterial,
    skinning: Option<ParsedSkinning>,
}

struct ParsedNodeModel {
    primitives: Vec<ParsedPrimitive>,
    cpu_positions: Vec<[f32; 3]>,
    cpu_indices: Vec<u32>,
    skinned: bool,
}

#[derive(Debug)]
struct FlattenedLevelNodeInfo {
    name: String,
    position: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

#[derive(Default, Deserialize)]
struct LevelNodeExtras {
    #[serde(rename = "voplayTerrain")]
    voplay_terrain: Option<LevelTerrainExtras>,
}

#[derive(Default, Deserialize)]
struct LevelTerrainExtras {
    heightmap: String,
    size: [f32; 3],
    #[serde(default)]
    material: LevelTerrainMaterialExtras,
    #[serde(default)]
    physics: LevelTerrainPhysicsExtras,
}

#[derive(Default, Deserialize)]
struct LevelTerrainMaterialExtras {
    albedo: Option<String>,
    control: Option<String>,
    layers: Option<Vec<LevelTerrainLayerExtras>>,
    #[serde(default = "default_terrain_uv_scale", rename = "uvScale")]
    uv_scale: f32,
}

#[derive(Deserialize)]
struct LevelTerrainLayerExtras {
    texture: String,
    normal: Option<String>,
    #[serde(rename = "metallicRoughness")]
    metallic_roughness: Option<String>,
    #[serde(default = "default_terrain_uv_scale", rename = "uvScale")]
    uv_scale: f32,
    #[serde(default, rename = "normalScale")]
    normal_scale: f32,
}

#[derive(Default, Deserialize)]
struct LevelTerrainPhysicsExtras {
    layer: Option<u16>,
    mask: Option<u16>,
    friction: Option<f32>,
    restitution: Option<f32>,
}

struct UploadedGltfTextures {
    srgb: HashMap<usize, TextureId>,
    linear: HashMap<usize, TextureId>,
}

fn default_terrain_uv_scale() -> f32 {
    1.0
}

/// A single sub-mesh within a model (one draw call).
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub material: MeshMaterial,
    pub skinned: bool,
}

/// Material payload consumed by the forward renderer.
#[derive(Clone, Copy)]
pub struct MeshMaterial {
    pub base_color: [f32; 4],
    pub texture_id: Option<TextureId>,
    pub normal_texture_id: Option<TextureId>,
    pub metallic_roughness_texture_id: Option<TextureId>,
    pub emissive_texture_id: Option<TextureId>,
    pub toon_ramp_texture_id: Option<TextureId>,
    pub metallic: f32,
    pub roughness: f32,
    pub normal_scale: f32,
    pub emissive_factor: [f32; 3],
    pub uv_scales: [f32; 4],
    pub sampler: MaterialSamplerKey,
    pub control_texture_id: Option<TextureId>,
    pub layer_texture_ids: [Option<TextureId>; 4],
    pub layer_normal_texture_ids: [Option<TextureId>; 4],
    pub layer_metallic_roughness_texture_ids: [Option<TextureId>; 4],
    pub layer_normal_scales: [f32; 4],
}

impl MeshMaterial {
    pub fn standard(base_color: [f32; 4], texture_id: Option<TextureId>, uv_scale: f32) -> Self {
        assert!(uv_scale > 0.0, "voplay: material uv scale must be > 0");
        Self {
            base_color,
            texture_id,
            normal_texture_id: None,
            metallic_roughness_texture_id: None,
            emissive_texture_id: None,
            toon_ramp_texture_id: None,
            metallic: 0.0,
            roughness: 0.55,
            normal_scale: 1.0,
            emissive_factor: [0.0, 0.0, 0.0],
            uv_scales: [uv_scale, 1.0, 1.0, 1.0],
            sampler: MaterialSamplerKey::REPEAT_LINEAR,
            control_texture_id: None,
            layer_texture_ids: [None, None, None, None],
            layer_normal_texture_ids: [None, None, None, None],
            layer_metallic_roughness_texture_ids: [None, None, None, None],
            layer_normal_scales: [0.0, 0.0, 0.0, 0.0],
        }
    }

    pub fn terrain_splat(
        base_color: [f32; 4],
        control_texture_id: TextureId,
        layer_texture_ids: [TextureId; 4],
        layer_normal_texture_ids: [TextureId; 4],
        layer_metallic_roughness_texture_ids: [TextureId; 4],
        uv_scales: [f32; 4],
        layer_normal_scales: [f32; 4],
    ) -> Self {
        let uv_scales: [f32; 4] = std::array::from_fn(|index| {
            let value = uv_scales[index];
            if value.is_finite() && value > 0.0 {
                value
            } else {
                1.0
            }
        });
        let layer_normal_scales: [f32; 4] = std::array::from_fn(|index| {
            let value = layer_normal_scales[index];
            if value.is_finite() && value >= 0.0 {
                value
            } else {
                0.0
            }
        });
        let layer_normal_scales = std::array::from_fn(|index| {
            if layer_normal_texture_ids[index] == 0 {
                0.0
            } else if layer_normal_scales[index] > 0.0 {
                layer_normal_scales[index]
            } else {
                1.0
            }
        });
        Self {
            base_color,
            texture_id: None,
            normal_texture_id: None,
            metallic_roughness_texture_id: None,
            emissive_texture_id: None,
            toon_ramp_texture_id: None,
            metallic: 0.0,
            roughness: 0.55,
            normal_scale: 1.0,
            emissive_factor: [0.0, 0.0, 0.0],
            uv_scales,
            sampler: MaterialSamplerKey::REPEAT_LINEAR,
            control_texture_id: Some(control_texture_id),
            layer_texture_ids: layer_texture_ids.map(Some),
            layer_normal_texture_ids: layer_normal_texture_ids.map(|id| {
                if id == 0 {
                    None
                } else {
                    Some(id)
                }
            }),
            layer_metallic_roughness_texture_ids: layer_metallic_roughness_texture_ids.map(|id| {
                if id == 0 {
                    None
                } else {
                    Some(id)
                }
            }),
            layer_normal_scales,
        }
    }
}

/// A loaded model: one or more sub-meshes.
pub struct GpuModel {
    pub meshes: Vec<GpuMesh>,
    pub cpu_positions: Vec<[f32; 3]>,
    pub cpu_indices: Vec<u32>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub skeleton: Option<Skeleton>,
    pub clips: Vec<AnimationClip>,
    pub rest_joint_palette: Vec<Mat4>,
}

/// Interleaved vertex format: position (3) + normal (3) + UV (2) + tangent (4) + color (4).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkinnedMeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
    pub color: [f32; 4],
    pub joint_indices: [u16; 4],
    pub joint_weights: [f32; 4],
}

impl MeshVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3, // position
        1 => Float32x3, // normal
        2 => Float32x2, // uv
        3 => Float32x4, // tangent.xyz + handedness
        4 => Float32x4, // vertex color
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

impl SkinnedMeshVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
        4 => Float32x4,
        5 => Uint16x4,
        6 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PrimitiveKey {
    Plane {
        width_bits: u32,
        depth_bits: u32,
        sub_x: u32,
        sub_z: u32,
    },
    Cube,
    RoundedBox {
        bevel_radius_bits: u32,
        segments: u32,
    },
    Sphere {
        segments: u32,
    },
    Cylinder {
        segments: u32,
    },
    Cone {
        segments: u32,
    },
    Wedge,
    Capsule {
        segments: u32,
        half_height_bits: u32,
        radius_bits: u32,
    },
}

/// Manages all loaded models, keyed by ModelId.
pub struct ModelManager {
    models: HashMap<ModelId, GpuModel>,
    primitive_cache: HashMap<PrimitiveKey, ModelId>,
    next_id: AtomicU32,
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            models: HashMap::new(),
            primitive_cache: HashMap::new(),
            next_id: AtomicU32::new(1), // 0 = no model
        }
    }

    fn create_gpu_mesh(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[MeshVertex],
        indices: &[u32],
        material: MeshMaterial,
    ) -> GpuMesh {
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_vb"),
            size: (vertices.len() * std::mem::size_of::<MeshVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(vertices));

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_mesh_ib"),
            size: (indices.len() * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(indices));

        GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            material,
            skinned: false,
        }
    }

    fn create_skinned_gpu_mesh(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[SkinnedMeshVertex],
        indices: &[u32],
        material: MeshMaterial,
    ) -> GpuMesh {
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_skinned_mesh_vb"),
            size: (vertices.len() * std::mem::size_of::<SkinnedMeshVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(vertices));

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay_skinned_mesh_ib"),
            size: (indices.len() * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(indices));

        GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            material,
            skinned: true,
        }
    }

    fn compute_aabb(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
        if positions.is_empty() {
            return ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for position in positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
        (min, max)
    }

    fn insert_model(
        &mut self,
        meshes: Vec<GpuMesh>,
        cpu_positions: Vec<[f32; 3]>,
        cpu_indices: Vec<u32>,
        skeleton: Option<Skeleton>,
        clips: Vec<AnimationClip>,
    ) -> ModelId {
        let (aabb_min, aabb_max) = Self::compute_aabb(&cpu_positions);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let rest_joint_palette = skeleton
            .as_ref()
            .map(animation::compute_rest_joint_palette)
            .unwrap_or_default();
        self.models.insert(
            id,
            GpuModel {
                meshes,
                cpu_positions,
                cpu_indices,
                aabb_min,
                aabb_max,
                skeleton,
                clips,
                rest_joint_palette,
            },
        );
        id
    }

    fn get_or_create_primitive<F>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: PrimitiveKey,
        build: F,
    ) -> ModelId
    where
        F: FnOnce() -> (Vec<MeshVertex>, Vec<u32>),
    {
        if let Some(id) = self.primitive_cache.get(&key).copied() {
            if self.models.contains_key(&id) {
                return id;
            }
        }
        let (vertices, indices) = build();
        let id = self.create_raw_with_material(
            device,
            queue,
            &vertices,
            &indices,
            MeshMaterial::standard([1.0, 1.0, 1.0, 1.0], None, 1.0),
        );
        self.primitive_cache.insert(key, id);
        id
    }

    pub fn create_raw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[MeshVertex],
        indices: &[u32],
        base_color: [f32; 4],
    ) -> ModelId {
        self.create_raw_with_material(
            device,
            queue,
            vertices,
            indices,
            MeshMaterial::standard(base_color, None, 1.0),
        )
    }

    pub fn create_raw_with_material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[MeshVertex],
        indices: &[u32],
        material: MeshMaterial,
    ) -> ModelId {
        let cpu_positions: Vec<[f32; 3]> = vertices.iter().map(|vertex| vertex.position).collect();
        let mesh = Self::create_gpu_mesh(device, queue, vertices, indices, material);
        self.insert_model(
            vec![mesh],
            cpu_positions,
            indices.to_vec(),
            None,
            Vec::new(),
        )
    }

    pub fn create_plane(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: f32,
        depth: f32,
        sub_x: u32,
        sub_z: u32,
    ) -> ModelId {
        self.get_or_create_primitive(
            device,
            queue,
            PrimitiveKey::Plane {
                width_bits: width.to_bits(),
                depth_bits: depth.to_bits(),
                sub_x,
                sub_z,
            },
            || primitives::generate_plane(width, depth, sub_x, sub_z),
        )
    }

    pub fn create_cube(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> ModelId {
        self.get_or_create_primitive(device, queue, PrimitiveKey::Cube, primitives::generate_cube)
    }

    pub fn create_rounded_box(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bevel_radius: f32,
        segments: u32,
    ) -> ModelId {
        self.get_or_create_primitive(
            device,
            queue,
            PrimitiveKey::RoundedBox {
                bevel_radius_bits: bevel_radius.to_bits(),
                segments,
            },
            || primitives::generate_rounded_box(bevel_radius, segments),
        )
    }

    pub fn create_sphere(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        segments: u32,
    ) -> ModelId {
        self.get_or_create_primitive(device, queue, PrimitiveKey::Sphere { segments }, || {
            primitives::generate_sphere(segments)
        })
    }

    pub fn create_cylinder(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        segments: u32,
    ) -> ModelId {
        self.get_or_create_primitive(device, queue, PrimitiveKey::Cylinder { segments }, || {
            primitives::generate_cylinder(segments)
        })
    }

    pub fn create_cone(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        segments: u32,
    ) -> ModelId {
        self.get_or_create_primitive(device, queue, PrimitiveKey::Cone { segments }, || {
            primitives::generate_cone(segments)
        })
    }

    pub fn create_wedge(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> ModelId {
        self.get_or_create_primitive(
            device,
            queue,
            PrimitiveKey::Wedge,
            primitives::generate_wedge,
        )
    }

    pub fn create_capsule(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        segments: u32,
        half_height: f32,
        radius: f32,
    ) -> ModelId {
        self.get_or_create_primitive(
            device,
            queue,
            PrimitiveKey::Capsule {
                segments,
                half_height_bits: half_height.to_bits(),
                radius_bits: radius.to_bits(),
            },
            || primitives::generate_capsule(segments, half_height, radius),
        )
    }

    fn collect_gltf_texture_usage(document: &gltf::Document) -> (HashSet<usize>, HashSet<usize>) {
        let mut srgb = HashSet::new();
        let mut linear = HashSet::new();
        for material in document.materials() {
            let pbr = material.pbr_metallic_roughness();
            if let Some(info) = pbr.base_color_texture() {
                srgb.insert(info.texture().source().index());
            }
            if let Some(info) = material.emissive_texture() {
                srgb.insert(info.texture().source().index());
            }
            if let Some(info) = material.normal_texture() {
                linear.insert(info.texture().source().index());
            }
            if let Some(info) = pbr.metallic_roughness_texture() {
                linear.insert(info.texture().source().index());
            }
        }
        (srgb, linear)
    }

    fn gltf_texture_sampler(texture: gltf::Texture<'_>) -> MaterialSamplerKey {
        let sampler = texture.sampler();
        let wrap_mode = if sampler.wrap_s() == gltf::texture::WrappingMode::ClampToEdge
            || sampler.wrap_t() == gltf::texture::WrappingMode::ClampToEdge
        {
            MATERIAL_WRAP_CLAMP
        } else if sampler.wrap_s() == gltf::texture::WrappingMode::MirroredRepeat
            || sampler.wrap_t() == gltf::texture::WrappingMode::MirroredRepeat
        {
            MATERIAL_WRAP_MIRROR
        } else {
            MATERIAL_WRAP_REPEAT
        };

        let filter_mode = match (sampler.mag_filter(), sampler.min_filter()) {
            (Some(gltf::texture::MagFilter::Nearest), _)
            | (_, Some(gltf::texture::MinFilter::Nearest))
            | (_, Some(gltf::texture::MinFilter::NearestMipmapNearest))
            | (_, Some(gltf::texture::MinFilter::NearestMipmapLinear)) => MATERIAL_FILTER_NEAREST,
            _ => MATERIAL_FILTER_LINEAR,
        };
        MaterialSamplerKey {
            wrap_mode,
            filter_mode,
        }
    }

    fn gltf_image_to_rgba(image: &gltf::image::Data) -> Option<Vec<u8>> {
        match image.format {
            gltf::image::Format::R8 => {
                let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
                for value in &image.pixels {
                    rgba.extend_from_slice(&[*value, *value, *value, 255]);
                }
                Some(rgba)
            }
            gltf::image::Format::R8G8 => {
                let mut rgba = Vec::with_capacity(image.pixels.len() / 2 * 4);
                for chunk in image.pixels.chunks_exact(2) {
                    rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
                }
                Some(rgba)
            }
            gltf::image::Format::R8G8B8 => {
                let mut rgba = Vec::with_capacity(image.pixels.len() / 3 * 4);
                for chunk in image.pixels.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                Some(rgba)
            }
            gltf::image::Format::R8G8B8A8 => Some(image.pixels.clone()),
            _ => None,
        }
    }

    fn upload_resolved_textures(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        document: &gltf::Document,
        images: &[gltf::image::Data],
    ) -> UploadedGltfTextures {
        let (srgb_usage, linear_usage) = Self::collect_gltf_texture_usage(document);
        let mut textures = UploadedGltfTextures {
            srgb: HashMap::new(),
            linear: HashMap::new(),
        };
        for (idx, image) in images.iter().enumerate() {
            let Some(rgba) = Self::gltf_image_to_rgba(image) else {
                continue;
            };
            if srgb_usage.contains(&idx) || !linear_usage.contains(&idx) {
                let tex_id = texture_manager.load_rgba_with_srgb(
                    device,
                    queue,
                    image.width,
                    image.height,
                    &rgba,
                    true,
                );
                textures.srgb.insert(idx, tex_id);
            }
            if linear_usage.contains(&idx) {
                let tex_id = texture_manager.load_rgba_with_srgb(
                    device,
                    queue,
                    image.width,
                    image.height,
                    &rgba,
                    false,
                );
                textures.linear.insert(idx, tex_id);
            }
        }
        textures
    }

    fn build_node_parent_map(document: &gltf::Document) -> HashMap<usize, usize> {
        let mut node_parent = HashMap::new();
        for node in document.nodes() {
            for child in node.children() {
                node_parent.insert(child.index(), node.index());
            }
        }
        node_parent
    }

    fn build_skin_and_clips(
        document: &gltf::Document,
        buffers: &[gltf::buffer::Data],
        node_parent: &HashMap<usize, usize>,
        label: &str,
    ) -> Result<(Option<usize>, Option<Skeleton>, Vec<AnimationClip>), String> {
        let mut used_skin_index: Option<usize> = None;
        for node in document.nodes() {
            if let Some(skin) = node.skin() {
                match used_skin_index {
                    Some(existing) if existing != skin.index() => {
                        return Err(format!("model '{}' uses multiple skins; Step 06 supports one skeleton per model", label));
                    }
                    None => used_skin_index = Some(skin.index()),
                    _ => {}
                }
            }
        }
        let Some(skin_index) = used_skin_index else {
            return Ok((None, None, Vec::new()));
        };
        let skin = document
            .skins()
            .find(|candidate| candidate.index() == skin_index)
            .unwrap_or_else(|| panic!("voplay: skin {} missing in '{}'", skin_index, label));
        let joint_nodes: Vec<_> = skin.joints().collect();
        if joint_nodes.len() > MAX_JOINTS {
            return Err(format!(
                "model '{}' has {} joints, exceeding max {}",
                label,
                joint_nodes.len(),
                MAX_JOINTS
            ));
        }
        let mut joint_node_to_index = HashMap::new();
        for (joint_index, joint_node) in joint_nodes.iter().enumerate() {
            joint_node_to_index.insert(joint_node.index(), joint_index);
        }
        let inverse_bind_matrices: Vec<Mat4> = skin
            .reader(|buffer| Some(&buffers[buffer.index()]))
            .read_inverse_bind_matrices()
            .map(|iter| iter.collect())
            .unwrap_or_else(|| vec![MAT4_IDENTITY; joint_nodes.len()]);
        if inverse_bind_matrices.len() != joint_nodes.len() {
            return Err(format!(
                "model '{}' skin inverse bind count mismatch ({} joints, {} matrices)",
                label,
                joint_nodes.len(),
                inverse_bind_matrices.len()
            ));
        }
        let joints = joint_nodes
            .iter()
            .map(|joint_node| {
                let mut parent = node_parent.get(&joint_node.index()).copied();
                let mut joint_parent = None;
                while let Some(parent_index) = parent {
                    if let Some(&mapped) = joint_node_to_index.get(&parent_index) {
                        joint_parent = Some(mapped);
                        break;
                    }
                    parent = node_parent.get(&parent_index).copied();
                }
                let (translation, rotation, scale) = joint_node.transform().decomposed();
                Joint {
                    name: joint_node.name().unwrap_or("").to_string(),
                    parent: joint_parent,
                    local_transform: Transform {
                        translation: Vec3::from_array(translation),
                        rotation: Quat::new(rotation[0], rotation[1], rotation[2], rotation[3]),
                        scale: Vec3::from_array(scale),
                    },
                }
            })
            .collect();
        let skeleton = Skeleton {
            joints,
            inverse_bind_matrices,
        };
        let mut clips = Vec::new();
        for (clip_index, animation) in document.animations().enumerate() {
            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            for channel in animation.channels() {
                let target_node_index = channel.target().node().index();
                let Some(&joint_index) = joint_node_to_index.get(&target_node_index) else {
                    continue;
                };
                let property = match channel.target().property() {
                    gltf::animation::Property::Translation => AnimationProperty::Translation,
                    gltf::animation::Property::Rotation => AnimationProperty::Rotation,
                    gltf::animation::Property::Scale => AnimationProperty::Scale,
                    gltf::animation::Property::MorphTargetWeights => {
                        return Err(format!(
                            "model '{}' uses unsupported morph target animation",
                            label
                        ));
                    }
                };
                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => AnimationInterpolation::Step,
                    gltf::animation::Interpolation::Linear => AnimationInterpolation::Linear,
                    gltf::animation::Interpolation::CubicSpline => {
                        AnimationInterpolation::CubicSpline
                    }
                };
                let reader = channel.reader(|buffer| Some(&buffers[buffer.index()]));
                let times: Vec<f32> = reader
                    .read_inputs()
                    .unwrap_or_else(|| {
                        panic!("voplay: animation channel missing inputs in '{}'", label)
                    })
                    .collect();
                if times.is_empty() {
                    continue;
                }
                duration = duration.max(*times.last().unwrap());
                let values: Vec<f32> = match reader.read_outputs().unwrap_or_else(|| {
                    panic!("voplay: animation channel missing outputs in '{}'", label)
                }) {
                    gltf::animation::util::ReadOutputs::Translations(iter) => {
                        assert_eq!(
                            property,
                            AnimationProperty::Translation,
                            "voplay: translation output/property mismatch"
                        );
                        iter.flat_map(|value| value.into_iter()).collect()
                    }
                    gltf::animation::util::ReadOutputs::Rotations(iter) => {
                        assert_eq!(
                            property,
                            AnimationProperty::Rotation,
                            "voplay: rotation output/property mismatch"
                        );
                        iter.into_f32()
                            .flat_map(|value| value.into_iter())
                            .collect()
                    }
                    gltf::animation::util::ReadOutputs::Scales(iter) => {
                        assert_eq!(
                            property,
                            AnimationProperty::Scale,
                            "voplay: scale output/property mismatch"
                        );
                        iter.flat_map(|value| value.into_iter()).collect()
                    }
                    gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => {
                        return Err(format!(
                            "model '{}' uses unsupported morph target animation",
                            label
                        ));
                    }
                };
                let width = match property {
                    AnimationProperty::Translation | AnimationProperty::Scale => 3,
                    AnimationProperty::Rotation => 4,
                };
                let expected = match interpolation {
                    AnimationInterpolation::Step | AnimationInterpolation::Linear => {
                        times.len() * width
                    }
                    AnimationInterpolation::CubicSpline => times.len() * width * 3,
                };
                if values.len() != expected {
                    return Err(format!(
                        "model '{}' animation channel has {} values, expected {}",
                        label,
                        values.len(),
                        expected
                    ));
                }
                channels.push(AnimationChannel {
                    joint_index,
                    property,
                    interpolation,
                    times,
                    values,
                });
            }
            clips.push(AnimationClip {
                name: animation
                    .name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("clip_{}", clip_index)),
                duration,
                channels,
            });
        }
        Ok((Some(skin_index), Some(skeleton), clips))
    }

    fn upload_parsed_node_primitives(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        primitives: Vec<ParsedPrimitive>,
    ) -> Vec<GpuMesh> {
        let mut gpu_meshes = Vec::with_capacity(primitives.len());
        for primitive in primitives {
            if let Some(skinning) = primitive.skinning {
                let vertices: Vec<SkinnedMeshVertex> = primitive
                    .positions
                    .iter()
                    .enumerate()
                    .map(|(i, pos)| SkinnedMeshVertex {
                        position: *pos,
                        normal: primitive.normals[i],
                        uv: primitive.uvs[i],
                        tangent: primitive.tangents[i],
                        color: primitive.colors[i],
                        joint_indices: skinning.joint_indices[i],
                        joint_weights: skinning.joint_weights[i],
                    })
                    .collect();
                gpu_meshes.push(Self::create_skinned_gpu_mesh(
                    device,
                    queue,
                    &vertices,
                    &primitive.indices,
                    primitive.material,
                ));
            } else {
                let vertices: Vec<MeshVertex> = primitive
                    .positions
                    .iter()
                    .enumerate()
                    .map(|(i, pos)| MeshVertex {
                        position: *pos,
                        normal: primitive.normals[i],
                        uv: primitive.uvs[i],
                        tangent: primitive.tangents[i],
                        color: primitive.colors[i],
                    })
                    .collect();
                gpu_meshes.push(Self::create_gpu_mesh(
                    device,
                    queue,
                    &vertices,
                    &primitive.indices,
                    primitive.material,
                ));
            }
        }
        gpu_meshes
    }

    fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn normalize3(value: [f32; 3]) -> Option<[f32; 3]> {
        let len_sq = Self::dot3(value, value);
        if len_sq <= 0.00000001 {
            return None;
        }
        let inv_len = len_sq.sqrt().recip();
        Some([value[0] * inv_len, value[1] * inv_len, value[2] * inv_len])
    }

    fn fallback_tangent(normal: [f32; 3]) -> [f32; 3] {
        let reference = if normal[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let projected = [
            reference[0] - normal[0] * Self::dot3(reference, normal),
            reference[1] - normal[1] * Self::dot3(reference, normal),
            reference[2] - normal[2] * Self::dot3(reference, normal),
        ];
        Self::normalize3(projected).unwrap_or([1.0, 0.0, 0.0])
    }

    fn generate_tangents(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
    ) -> Vec<[f32; 4]> {
        let mut tangents = vec![[0.0f32; 3]; positions.len()];
        let mut bitangents = vec![[0.0f32; 3]; positions.len()];
        for tri in indices.chunks_exact(3) {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;
            if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
                continue;
            }
            let p0 = positions[i0];
            let p1 = positions[i1];
            let p2 = positions[i2];
            let uv0 = uvs.get(i0).copied().unwrap_or([0.0, 0.0]);
            let uv1 = uvs.get(i1).copied().unwrap_or([0.0, 0.0]);
            let uv2 = uvs.get(i2).copied().unwrap_or([0.0, 0.0]);
            let edge1 = Self::sub3(p1, p0);
            let edge2 = Self::sub3(p2, p0);
            let duv1 = [uv1[0] - uv0[0], uv1[1] - uv0[1]];
            let duv2 = [uv2[0] - uv0[0], uv2[1] - uv0[1]];
            let det = duv1[0] * duv2[1] - duv2[0] * duv1[1];
            if det.abs() <= 0.00000001 {
                continue;
            }
            let inv_det = det.recip();
            let tangent = [
                (edge1[0] * duv2[1] - edge2[0] * duv1[1]) * inv_det,
                (edge1[1] * duv2[1] - edge2[1] * duv1[1]) * inv_det,
                (edge1[2] * duv2[1] - edge2[2] * duv1[1]) * inv_det,
            ];
            let bitangent = [
                (edge2[0] * duv1[0] - edge1[0] * duv2[0]) * inv_det,
                (edge2[1] * duv1[0] - edge1[1] * duv2[0]) * inv_det,
                (edge2[2] * duv1[0] - edge1[2] * duv2[0]) * inv_det,
            ];
            for index in [i0, i1, i2] {
                tangents[index][0] += tangent[0];
                tangents[index][1] += tangent[1];
                tangents[index][2] += tangent[2];
                bitangents[index][0] += bitangent[0];
                bitangents[index][1] += bitangent[1];
                bitangents[index][2] += bitangent[2];
            }
        }

        positions
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let normal = Self::normalize3(normals[index]).unwrap_or([0.0, 1.0, 0.0]);
                let tangent_accum = tangents[index];
                let projected = [
                    tangent_accum[0] - normal[0] * Self::dot3(normal, tangent_accum),
                    tangent_accum[1] - normal[1] * Self::dot3(normal, tangent_accum),
                    tangent_accum[2] - normal[2] * Self::dot3(normal, tangent_accum),
                ];
                let tangent =
                    Self::normalize3(projected).unwrap_or_else(|| Self::fallback_tangent(normal));
                let handedness =
                    if Self::dot3(Self::cross3(normal, tangent), bitangents[index]) < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                [tangent[0], tangent[1], tangent[2], handedness]
            })
            .collect()
    }

    fn parse_node_model(
        node: &gltf::Node,
        textures: &UploadedGltfTextures,
        buffers: &[gltf::buffer::Data],
        used_skin_index: Option<usize>,
        label: &str,
    ) -> Result<Option<ParsedNodeModel>, String> {
        let Some(mesh) = node.mesh() else {
            return Ok(None);
        };
        let node_name = node
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("node_{}", node.index()));
        let node_skin = node.skin().map(|skin| skin.index());
        if node_skin.is_some() && node_skin != used_skin_index {
            return Err(format!(
                "model '{}' node '{}' skinned mesh references a different skin",
                label, node_name
            ));
        }
        let mut primitives = Vec::new();
        let mut cpu_positions = Vec::new();
        let mut cpu_indices = Vec::new();
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(iter) => iter.collect(),
                None => continue,
            };
            let normals: Vec<[f32; 3]> = match reader.read_normals() {
                Some(iter) => iter.collect(),
                None => vec![[0.0, 1.0, 0.0]; positions.len()],
            };
            let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                Some(iter) => iter.into_f32().collect(),
                None => vec![[0.0, 0.0]; positions.len()],
            };
            let indices: Vec<u32> = match reader.read_indices() {
                Some(iter) => iter.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            let tangents: Vec<[f32; 4]> = match reader.read_tangents() {
                Some(iter) => iter.collect(),
                None => Self::generate_tangents(&positions, &normals, &uvs, &indices),
            };
            let colors: Vec<[f32; 4]> = match reader.read_colors(0) {
                Some(iter) => iter.into_rgba_f32().collect(),
                None => vec![[1.0, 1.0, 1.0, 1.0]; positions.len()],
            };
            if normals.len() != positions.len()
                || uvs.len() != positions.len()
                || tangents.len() != positions.len()
                || colors.len() != positions.len()
            {
                return Err(format!(
                    "model '{}' node '{}' mesh attribute count mismatch (positions {}, normals {}, uvs {}, tangents {}, colors {})",
                    label,
                    node_name,
                    positions.len(),
                    normals.len(),
                    uvs.len(),
                    tangents.len(),
                    colors.len()
                ));
            }
            let material_src = primitive.material();
            let pbr = material_src.pbr_metallic_roughness();
            let base_color_texture = pbr.base_color_texture();
            let mut material = MeshMaterial::standard(
                pbr.base_color_factor(),
                base_color_texture
                    .as_ref()
                    .and_then(|info| textures.srgb.get(&info.texture().source().index()).copied()),
                1.0,
            );
            if let Some(info) = base_color_texture {
                material.sampler = Self::gltf_texture_sampler(info.texture());
            }
            material.roughness = pbr.roughness_factor();
            material.metallic = pbr.metallic_factor();
            if let Some(info) = pbr.metallic_roughness_texture() {
                material.metallic_roughness_texture_id = textures
                    .linear
                    .get(&info.texture().source().index())
                    .copied();
                if material.texture_id.is_none() && material.normal_texture_id.is_none() {
                    material.sampler = Self::gltf_texture_sampler(info.texture());
                }
            }
            if let Some(info) = material_src.normal_texture() {
                material.normal_texture_id = textures
                    .linear
                    .get(&info.texture().source().index())
                    .copied();
                material.normal_scale = info.scale();
                if material.texture_id.is_none() {
                    material.sampler = Self::gltf_texture_sampler(info.texture());
                }
            }
            if let Some(info) = material_src.emissive_texture() {
                material.emissive_texture_id =
                    textures.srgb.get(&info.texture().source().index()).copied();
                if material.texture_id.is_none()
                    && material.normal_texture_id.is_none()
                    && material.metallic_roughness_texture_id.is_none()
                {
                    material.sampler = Self::gltf_texture_sampler(info.texture());
                }
            }
            material.emissive_factor = material_src.emissive_factor();
            let index_base = cpu_positions.len() as u32;
            cpu_positions.extend(positions.iter().copied());
            cpu_indices.extend(indices.iter().map(|index| index_base + *index));
            let skinning = if node_skin.is_some() {
                let joint_indices: Vec<[u16; 4]> = reader
                    .read_joints(0)
                    .unwrap_or_else(|| {
                        panic!("voplay: skinned mesh missing JOINTS_0 in '{}'", label)
                    })
                    .into_u16()
                    .collect();
                let joint_weights: Vec<[f32; 4]> = reader
                    .read_weights(0)
                    .unwrap_or_else(|| {
                        panic!("voplay: skinned mesh missing WEIGHTS_0 in '{}'", label)
                    })
                    .into_f32()
                    .collect();
                if joint_indices.len() != positions.len() || joint_weights.len() != positions.len()
                {
                    return Err(format!(
                        "model '{}' node '{}' skinned mesh attribute count mismatch (positions {}, joints {}, weights {})",
                        label,
                        node_name,
                        positions.len(),
                        joint_indices.len(),
                        joint_weights.len()
                    ));
                }
                Some(ParsedSkinning {
                    joint_indices,
                    joint_weights,
                })
            } else {
                None
            };
            primitives.push(ParsedPrimitive {
                positions,
                normals,
                uvs,
                tangents,
                colors,
                indices,
                material,
                skinning,
            });
        }
        if primitives.is_empty() {
            return Err(format!(
                "model '{}' node '{}' contains no renderable meshes",
                label, node_name
            ));
        }
        Ok(Some(ParsedNodeModel {
            primitives,
            cpu_positions,
            cpu_indices,
            skinned: node_skin.is_some(),
        }))
    }

    fn flatten_level_node(
        node: &gltf::Node,
        parent_world: &Mat4,
        label: &str,
    ) -> Result<(Mat4, FlattenedLevelNodeInfo), String> {
        let (translation, rotation, scale) = node.transform().decomposed();
        let local = math3d::model_matrix(
            Vec3::from_array(translation),
            Quat::new(rotation[0], rotation[1], rotation[2], rotation[3]),
            Vec3::from_array(scale),
        );
        let world = math3d::mat4_mul(parent_world, &local);
        let name = node
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("node_{}", node.index()));
        let (world_pos, world_rot, world_scale) =
            math3d::decompose_matrix(&world).ok_or_else(|| {
                format!(
                    "level '{}' node '{}' has a non-decomposable transform",
                    label, name
                )
            })?;
        Ok((
            world,
            FlattenedLevelNodeInfo {
                name,
                position: world_pos.to_array(),
                rotation: [world_rot.x, world_rot.y, world_rot.z, world_rot.w],
                scale: world_scale.to_array(),
            },
        ))
    }

    #[cfg(test)]
    fn collect_flattened_level_nodes(
        node: gltf::Node,
        parent_world: &Mat4,
        label: &str,
        out: &mut Vec<FlattenedLevelNodeInfo>,
    ) -> Result<(), String> {
        let (world, flattened) = Self::flatten_level_node(&node, parent_world, label)?;
        out.push(flattened);
        for child in node.children() {
            Self::collect_flattened_level_nodes(child, &world, label, out)?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn flatten_level_scene_nodes(
        scene: gltf::Scene,
        label: &str,
    ) -> Result<Vec<FlattenedLevelNodeInfo>, String> {
        let mut nodes = Vec::new();
        for root in scene.nodes() {
            Self::collect_flattened_level_nodes(root, &MAT4_IDENTITY, label, &mut nodes)?;
        }
        Ok(nodes)
    }

    fn import_path(
        path: &Path,
    ) -> Result<
        (
            gltf::Document,
            Vec<gltf::buffer::Data>,
            Vec<gltf::image::Data>,
        ),
        String,
    > {
        let path_label = path.display().to_string();
        let data = file_io::read_bytes(path)
            .map_err(|e| format!("gltf import '{}': {}", path_label, e))?;
        let gltf = gltf::Gltf::from_slice(&data)
            .map_err(|e| format!("gltf import '{}': {}", path_label, e))?;
        let base_dir = path.parent();
        let buffers = Self::import_buffer_data(&gltf.document, base_dir, gltf.blob)?;
        let images = Self::import_image_data(&gltf.document, base_dir, &buffers)?;
        Ok((gltf.document, buffers, images))
    }

    fn read_external_uri(
        base_dir: Option<&Path>,
        uri: &str,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        if let Some(rest) = uri.strip_prefix("data:") {
            let (mime, encoded) = rest
                .split_once(";base64,")
                .ok_or_else(|| format!("unsupported data URI: {}", uri))?;
            let data = general_purpose::STANDARD
                .decode(encoded)
                .map_err(|e| format!("base64 decode '{}': {}", uri, e))?;
            let mime = if mime.is_empty() {
                None
            } else {
                Some(mime.to_string())
            };
            return Ok((data, mime));
        }
        if let Some(path) = uri.strip_prefix("file://") {
            return file_io::read_bytes(path).map(|data| (data, None));
        }
        if let Some(path) = uri.strip_prefix("file:") {
            return file_io::read_bytes(path).map(|data| (data, None));
        }
        if uri.contains(':') {
            return Err(format!("unsupported URI scheme: {}", uri));
        }
        let base_dir =
            base_dir.ok_or_else(|| "external reference in slice-only import".to_string())?;
        file_io::read_bytes(base_dir.join(uri)).map(|data| (data, None))
    }

    fn image_format_from_mime(mime_type: &str) -> Result<ImageFormat, String> {
        match mime_type {
            "image/png" => Ok(ImageFormat::Png),
            "image/jpeg" => Ok(ImageFormat::Jpeg),
            _ => Err(format!("unsupported image encoding: {}", mime_type)),
        }
    }

    fn image_format_for_uri(
        uri: &str,
        mime_type: Option<&str>,
        inline_mime: Option<&str>,
    ) -> Result<ImageFormat, String> {
        if let Some(mime_type) = mime_type.or(inline_mime) {
            return Self::image_format_from_mime(mime_type);
        }
        let extension = Path::new(uri)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        match extension.as_deref() {
            Some("png") => Ok(ImageFormat::Png),
            Some("jpg") | Some("jpeg") => Ok(ImageFormat::Jpeg),
            _ => Err(format!("unsupported image encoding: {}", uri)),
        }
    }

    fn dynamic_image_to_gltf_data(image: DynamicImage) -> Result<gltf::image::Data, String> {
        let format = match &image {
            DynamicImage::ImageLuma8(_) => gltf::image::Format::R8,
            DynamicImage::ImageLumaA8(_) => gltf::image::Format::R8G8,
            DynamicImage::ImageRgb8(_) => gltf::image::Format::R8G8B8,
            DynamicImage::ImageRgba8(_) => gltf::image::Format::R8G8B8A8,
            DynamicImage::ImageLuma16(_) => gltf::image::Format::R16,
            DynamicImage::ImageLumaA16(_) => gltf::image::Format::R16G16,
            DynamicImage::ImageRgb16(_) => gltf::image::Format::R16G16B16,
            DynamicImage::ImageRgba16(_) => gltf::image::Format::R16G16B16A16,
            DynamicImage::ImageRgb32F(_) => gltf::image::Format::R32G32B32FLOAT,
            DynamicImage::ImageRgba32F(_) => gltf::image::Format::R32G32B32A32FLOAT,
            _ => return Err("unsupported image format".to_string()),
        };
        let (width, height) = image.dimensions();
        Ok(gltf::image::Data {
            pixels: image.into_bytes(),
            format,
            width,
            height,
        })
    }

    fn import_buffer_data(
        document: &gltf::Document,
        base_dir: Option<&Path>,
        blob: Option<Vec<u8>>,
    ) -> Result<Vec<gltf::buffer::Data>, String> {
        let mut blob = blob;
        let mut buffers = Vec::new();
        for buffer in document.buffers() {
            let mut data = match buffer.source() {
                gltf::buffer::Source::Uri(uri) => Self::read_external_uri(base_dir, uri)?.0,
                gltf::buffer::Source::Bin => blob
                    .take()
                    .ok_or_else(|| "missing binary portion of binary glTF".to_string())?,
            };
            if data.len() < buffer.length() {
                return Err(format!(
                    "buffer {}: expected {} bytes but received {} bytes",
                    buffer.index(),
                    buffer.length(),
                    data.len()
                ));
            }
            while data.len() % 4 != 0 {
                data.push(0);
            }
            buffers.push(gltf::buffer::Data(data));
        }
        Ok(buffers)
    }

    fn import_image_data(
        document: &gltf::Document,
        base_dir: Option<&Path>,
        buffer_data: &[gltf::buffer::Data],
    ) -> Result<Vec<gltf::image::Data>, String> {
        let mut images = Vec::new();
        for image in document.images() {
            let image_data = match image.source() {
                gltf::image::Source::Uri { uri, mime_type } => {
                    let (encoded_image, inline_mime) = Self::read_external_uri(base_dir, uri)?;
                    let encoded_format =
                        Self::image_format_for_uri(uri, mime_type, inline_mime.as_deref())?;
                    let decoded_image =
                        image::load_from_memory_with_format(&encoded_image, encoded_format)
                            .map_err(|e| format!("image decode '{}': {}", uri, e))?;
                    Self::dynamic_image_to_gltf_data(decoded_image)?
                }
                gltf::image::Source::View { view, mime_type } => {
                    let parent_buffer_data = &buffer_data[view.buffer().index()].0;
                    let begin = view.offset();
                    let end = begin + view.length();
                    let encoded_image = &parent_buffer_data[begin..end];
                    let encoded_format = Self::image_format_from_mime(mime_type)?;
                    let decoded_image =
                        image::load_from_memory_with_format(encoded_image, encoded_format)
                            .map_err(|e| {
                                format!("image decode buffer view {}: {}", view.index(), e)
                            })?;
                    Self::dynamic_image_to_gltf_data(decoded_image)?
                }
            };
            images.push(image_data);
        }
        Ok(images)
    }

    fn build_level_node_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &UploadedGltfTextures,
        buffers: &[gltf::buffer::Data],
        node: &gltf::Node,
        used_skin_index: Option<usize>,
        skeleton: &Option<Skeleton>,
        clips: &[AnimationClip],
        label: &str,
    ) -> Result<(ModelId, [f32; 3], [f32; 3]), String> {
        let Some(parsed) = Self::parse_node_model(node, textures, buffers, used_skin_index, label)?
        else {
            return Ok((0, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]));
        };
        let (aabb_min, aabb_max) = Self::compute_aabb(&parsed.cpu_positions);
        let ParsedNodeModel {
            primitives,
            cpu_positions,
            cpu_indices,
            skinned,
        } = parsed;
        let gpu_meshes = Self::upload_parsed_node_primitives(device, queue, primitives);
        let model_id = self.insert_model(
            gpu_meshes,
            cpu_positions,
            cpu_indices,
            if skinned { skeleton.clone() } else { None },
            if skinned { clips.to_vec() } else { Vec::new() },
        );
        Ok((model_id, aabb_min, aabb_max))
    }

    fn parse_level_terrain_extras(
        node: &gltf::Node,
        label: &str,
    ) -> Result<Option<LevelTerrainExtras>, String> {
        let Some(raw) = node.extras().as_ref() else {
            return Ok(None);
        };
        let node_name = node
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("node_{}", node.index()));
        let extras: LevelNodeExtras = serde_json::from_str(raw.get())
            .map_err(|e| format!("level '{}' node '{}' extras parse: {}", label, node_name, e))?;
        Ok(extras.voplay_terrain)
    }

    fn resolve_level_asset_path(level_dir: &Path, value: &str) -> PathBuf {
        let path = Path::new(value);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            level_dir.join(path)
        }
    }

    fn load_level_texture_cached(
        texture_manager: &mut TextureManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_cache: &mut HashMap<String, TextureId>,
        path: &Path,
    ) -> Result<TextureId, String> {
        let key = path.to_string_lossy().into_owned();
        if let Some(id) = texture_cache.get(&key).copied() {
            return Ok(id);
        }
        let id = texture_manager.load_file(device, queue, &key)?;
        texture_cache.insert(key, id);
        Ok(id)
    }

    fn build_level_terrain_material(
        texture_manager: &mut TextureManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_cache: &mut HashMap<String, TextureId>,
        level_dir: &Path,
        extras: &LevelTerrainExtras,
        label: &str,
        node_name: &str,
    ) -> Result<MeshMaterial, String> {
        if extras.material.control.is_some() {
            if extras.material.albedo.is_some() {
                return Err(format!(
                    "level '{}' node '{}' terrain material cannot mix albedo and splat",
                    label, node_name
                ));
            }
            let control_path = Self::resolve_level_asset_path(
                level_dir,
                extras.material.control.as_deref().unwrap_or(""),
            );
            let layers = extras.material.layers.as_ref().ok_or_else(|| {
                format!(
                    "level '{}' node '{}' terrain splat requires 4 layers",
                    label, node_name
                )
            })?;
            if layers.len() != 4 {
                return Err(format!(
                    "level '{}' node '{}' terrain splat requires exactly 4 layers, got {}",
                    label,
                    node_name,
                    layers.len()
                ));
            }
            let control_texture_id = Self::load_level_texture_cached(
                texture_manager,
                device,
                queue,
                texture_cache,
                &control_path,
            )?;
            let mut layer_texture_ids = [0u32; 4];
            let mut layer_normal_texture_ids = [0u32; 4];
            let mut layer_metallic_roughness_texture_ids = [0u32; 4];
            let mut uv_scales = [1.0f32; 4];
            let mut normal_scales = [0.0f32; 4];
            for (index, layer) in layers.iter().enumerate() {
                if layer.uv_scale <= 0.0 {
                    return Err(format!(
                        "level '{}' node '{}' terrain layer {} uvScale must be > 0",
                        label, node_name, index
                    ));
                }
                if layer.normal_scale < 0.0 {
                    return Err(format!(
                        "level '{}' node '{}' terrain layer {} normalScale must be >= 0",
                        label, node_name, index
                    ));
                }
                let texture_path = Self::resolve_level_asset_path(level_dir, &layer.texture);
                layer_texture_ids[index] = Self::load_level_texture_cached(
                    texture_manager,
                    device,
                    queue,
                    texture_cache,
                    &texture_path,
                )?;
                if let Some(normal) = layer.normal.as_deref() {
                    let normal_path = Self::resolve_level_asset_path(level_dir, normal);
                    layer_normal_texture_ids[index] = Self::load_level_texture_cached(
                        texture_manager,
                        device,
                        queue,
                        texture_cache,
                        &normal_path,
                    )?;
                }
                if let Some(metallic_roughness) = layer.metallic_roughness.as_deref() {
                    let mr_path = Self::resolve_level_asset_path(level_dir, metallic_roughness);
                    layer_metallic_roughness_texture_ids[index] = Self::load_level_texture_cached(
                        texture_manager,
                        device,
                        queue,
                        texture_cache,
                        &mr_path,
                    )?;
                }
                uv_scales[index] = layer.uv_scale;
                normal_scales[index] = layer.normal_scale;
            }
            return Ok(MeshMaterial::terrain_splat(
                [1.0, 1.0, 1.0, 1.0],
                control_texture_id,
                layer_texture_ids,
                layer_normal_texture_ids,
                layer_metallic_roughness_texture_ids,
                uv_scales,
                normal_scales,
            ));
        }
        if let Some(layers) = extras.material.layers.as_ref() {
            return Err(format!(
                "level '{}' node '{}' terrain layers require a control texture, got {} layers",
                label,
                node_name,
                layers.len()
            ));
        }
        if extras.material.uv_scale <= 0.0 {
            return Err(format!(
                "level '{}' node '{}' terrain uvScale must be > 0",
                label, node_name
            ));
        }
        let texture_id = match extras.material.albedo.as_deref() {
            Some(value) => {
                let path = Self::resolve_level_asset_path(level_dir, value);
                Some(Self::load_level_texture_cached(
                    texture_manager,
                    device,
                    queue,
                    texture_cache,
                    &path,
                )?)
            }
            None => None,
        };
        Ok(MeshMaterial::standard(
            [1.0, 1.0, 1.0, 1.0],
            texture_id,
            extras.material.uv_scale,
        ))
    }

    fn build_level_terrain_node(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        texture_cache: &mut HashMap<String, TextureId>,
        level_dir: &Path,
        node: &gltf::Node,
        flattened: &FlattenedLevelNodeInfo,
        extras: &LevelTerrainExtras,
        label: &str,
    ) -> Result<LevelNode, String> {
        let node_name = &flattened.name;
        if node.mesh().is_some() {
            return Err(format!(
                "level '{}' node '{}' terrain node must not contain a mesh",
                label, node_name
            ));
        }
        let identity = [0.0, 0.0, 0.0, 1.0];
        if flattened
            .rotation
            .iter()
            .zip(identity.iter())
            .any(|(a, b)| (*a - *b).abs() > 0.0001)
        {
            return Err(format!(
                "level '{}' node '{}' terrain node rotation must be identity",
                label, node_name
            ));
        }
        let unit_scale = [1.0, 1.0, 1.0];
        if flattened
            .scale
            .iter()
            .zip(unit_scale.iter())
            .any(|(a, b)| (*a - *b).abs() > 0.0001)
        {
            return Err(format!(
                "level '{}' node '{}' terrain node scale must be 1",
                label, node_name
            ));
        }
        let [scale_x, scale_y, scale_z] = extras.size;
        if scale_x <= 0.0 || scale_y <= 0.0 || scale_z <= 0.0 {
            return Err(format!(
                "level '{}' node '{}' terrain size must be > 0",
                label, node_name
            ));
        }
        if extras.heightmap.is_empty() {
            return Err(format!(
                "level '{}' node '{}' terrain heightmap path is required",
                label, node_name
            ));
        }
        let heightmap_path = Self::resolve_level_asset_path(level_dir, &extras.heightmap);
        let image_data = file_io::read_bytes(&heightmap_path).map_err(|e| {
            format!(
                "level '{}' node '{}' terrain heightmap read '{}': {}",
                label,
                node_name,
                heightmap_path.display(),
                e
            )
        })?;
        let material = Self::build_level_terrain_material(
            texture_manager,
            device,
            queue,
            texture_cache,
            level_dir,
            extras,
            label,
            node_name,
        )?;
        let terrain_data = crate::terrain::generate_terrain(
            device,
            queue,
            self,
            &image_data,
            scale_x,
            scale_y,
            scale_z,
            material,
        )?;
        let (min_height, max_height) = terrain_data.heights.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(min_v, max_v), value| (min_v.min(*value), max_v.max(*value)),
        );
        Ok(LevelNode {
            kind: LevelNodeKind::Terrain,
            name: flattened.name.clone(),
            model_id: terrain_data.model_id,
            position: flattened.position,
            rotation: flattened.rotation,
            scale: flattened.scale,
            aabb_min: [-scale_x * 0.5, min_height * scale_y, -scale_z * 0.5],
            aabb_max: [scale_x * 0.5, max_height * scale_y, scale_z * 0.5],
            terrain: Some(LevelNodeTerrain {
                rows: terrain_data.rows,
                cols: terrain_data.cols,
                scale: [scale_x, scale_y, scale_z],
                heights: terrain_data.heights,
                layer: extras.physics.layer.unwrap_or(1),
                mask: extras.physics.mask.unwrap_or(0xFFFF),
                friction: extras.physics.friction.unwrap_or(0.8),
                restitution: extras.physics.restitution.unwrap_or(0.0),
            }),
        })
    }

    fn collect_level_nodes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        texture_cache: &mut HashMap<String, TextureId>,
        level_dir: &Path,
        textures: &UploadedGltfTextures,
        buffers: &[gltf::buffer::Data],
        node: gltf::Node,
        parent_world: &Mat4,
        used_skin_index: Option<usize>,
        skeleton: &Option<Skeleton>,
        clips: &[AnimationClip],
        label: &str,
        out: &mut Vec<LevelNode>,
    ) -> Result<(), String> {
        let (world, flattened) = Self::flatten_level_node(&node, parent_world, label)?;
        if let Some(terrain_extras) = Self::parse_level_terrain_extras(&node, label)? {
            let terrain_node = self.build_level_terrain_node(
                device,
                queue,
                texture_manager,
                texture_cache,
                level_dir,
                &node,
                &flattened,
                &terrain_extras,
                label,
            )?;
            out.push(terrain_node);
        } else {
            let (model_id, aabb_min, aabb_max) = self.build_level_node_model(
                device,
                queue,
                textures,
                buffers,
                &node,
                used_skin_index,
                skeleton,
                clips,
                label,
            )?;
            out.push(LevelNode {
                kind: LevelNodeKind::Entity,
                name: flattened.name,
                model_id,
                position: flattened.position,
                rotation: flattened.rotation,
                scale: flattened.scale,
                aabb_min,
                aabb_max,
                terrain: None,
            });
        }
        for child in node.children() {
            self.collect_level_nodes(
                device,
                queue,
                texture_manager,
                texture_cache,
                level_dir,
                textures,
                buffers,
                child,
                &world,
                used_skin_index,
                skeleton,
                clips,
                label,
                out,
            )?;
        }
        Ok(())
    }

    pub fn load_level_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        path: &str,
    ) -> Result<Vec<LevelNode>, String> {
        let path_ref = Path::new(path);
        let (document, buffers, images) = Self::import_path(path_ref)?;
        let scene = document
            .default_scene()
            .ok_or_else(|| format!("level '{}' has no default scene", path))?;
        let textures =
            Self::upload_resolved_textures(device, queue, texture_manager, &document, &images);
        let node_parent = Self::build_node_parent_map(&document);
        let (used_skin_index, skeleton, clips) =
            Self::build_skin_and_clips(&document, &buffers, &node_parent, path)?;
        let level_dir = path_ref
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut texture_cache = HashMap::new();
        let mut nodes = Vec::new();
        for root in scene.nodes() {
            self.collect_level_nodes(
                device,
                queue,
                texture_manager,
                &mut texture_cache,
                &level_dir,
                &textures,
                &buffers,
                root,
                &MAT4_IDENTITY,
                used_skin_index,
                &skeleton,
                &clips,
                path,
                &mut nodes,
            )?;
        }
        Ok(nodes)
    }

    /// Load a model from a file path (glTF or GLB).
    /// Uses gltf::import so external .bin buffers and texture files are resolved
    /// relative to the file's directory, supporting both GLB and split .gltf+.bin layouts.
    pub fn load_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        path: &str,
    ) -> Result<ModelId, String> {
        let (document, buffers, images) = Self::import_path(Path::new(path))?;
        self.upload_gltf(
            device,
            queue,
            texture_manager,
            document,
            buffers,
            images,
            path,
        )
    }

    /// Load a model from raw glTF/GLB bytes.
    /// Only supports self-contained formats (GLB or glTF with all buffers/textures embedded).
    /// For .gltf files with external .bin or texture files, use load_file instead.
    pub fn load_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        data: &[u8],
        source_path: Option<&str>,
    ) -> Result<ModelId, String> {
        let (document, buffers, images) =
            gltf::import_slice(data).map_err(|e| format!("gltf import: {}", e))?;
        let label = source_path.unwrap_or("<bytes>");
        self.upload_gltf(
            device,
            queue,
            texture_manager,
            document,
            buffers,
            images,
            label,
        )
    }

    /// Upload a parsed glTF document to GPU. Shared by load_file and load_bytes.
    fn upload_gltf(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        document: gltf::Document,
        buffers: Vec<gltf::buffer::Data>,
        images: Vec<gltf::image::Data>,
        label: &str,
    ) -> Result<ModelId, String> {
        let textures =
            Self::upload_resolved_textures(device, queue, texture_manager, &document, &images);
        let node_parent = Self::build_node_parent_map(&document);
        let (used_skin_index, skeleton, clips) =
            Self::build_skin_and_clips(&document, &buffers, &node_parent, label)?;
        let mut gpu_meshes = Vec::new();
        let mut cpu_positions = Vec::new();
        let mut cpu_indices = Vec::new();
        for node in document.nodes() {
            let Some(parsed) =
                Self::parse_node_model(&node, &textures, &buffers, used_skin_index, label)?
            else {
                continue;
            };
            let ParsedNodeModel {
                primitives,
                cpu_positions: node_positions,
                cpu_indices: node_indices,
                ..
            } = parsed;
            let index_base = cpu_positions.len() as u32;
            cpu_positions.extend(node_positions.into_iter());
            cpu_indices.extend(node_indices.into_iter().map(|index| index_base + index));
            gpu_meshes.extend(Self::upload_parsed_node_primitives(
                device, queue, primitives,
            ));
        }
        if gpu_meshes.is_empty() {
            return Err(format!("model '{}' contains no renderable meshes", label));
        }
        Ok(self.insert_model(gpu_meshes, cpu_positions, cpu_indices, skeleton, clips))
    }

    /// Free a model by ID.
    pub fn free(&mut self, id: ModelId) {
        self.models.remove(&id);
        self.primitive_cache.retain(|_, cached_id| *cached_id != id);
    }

    /// Get a model by ID.
    pub fn get(&self, id: ModelId) -> Option<&GpuModel> {
        self.models.get(&id)
    }

    pub fn animation_info(&self, id: ModelId) -> Option<ModelAnimationInfo> {
        let model = self.models.get(&id)?;
        Some(ModelAnimationInfo {
            has_skeleton: model.skeleton.is_some(),
            joint_count: model.skeleton.as_ref().map(|s| s.joints.len()).unwrap_or(0),
            clips: model
                .clips
                .iter()
                .map(|clip| AnimationClipInfo {
                    name: clip.name.clone(),
                    duration: clip.duration,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

    struct TempFixture {
        dir: PathBuf,
        gltf_path: PathBuf,
    }

    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn write_fixture(name: &str, gltf: &str, bin: Option<&[u8]>) -> TempFixture {
        let unique = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("voplay_model_loader_{}_{}", name, unique));
        fs::create_dir_all(&dir).expect("create fixture dir");
        let gltf_path = dir.join("scene.gltf");
        fs::write(&gltf_path, gltf.as_bytes()).expect("write gltf");
        if let Some(bin) = bin {
            fs::write(dir.join("mesh.bin"), bin).expect("write mesh.bin");
        }
        TempFixture { dir, gltf_path }
    }

    fn load_document(
        path: &Path,
    ) -> (
        gltf::Document,
        Vec<gltf::buffer::Data>,
        Vec<gltf::image::Data>,
    ) {
        gltf::import(path).expect("import gltf fixture")
    }

    #[test]
    fn flatten_level_scene_nodes_accumulates_parent_transform() {
        let fixture = write_fixture(
            "flatten",
            r#"{
                "asset": { "version": "2.0" },
                "scene": 0,
                "scenes": [
                    { "nodes": [0] }
                ],
                "nodes": [
                    { "name": "root", "translation": [1, 2, 3], "scale": [2, 3, 4], "children": [1] },
                    { "name": "child", "translation": [4, 5, 6], "scale": [0.5, 2, 1.5] }
                ]
            }"#,
            None,
        );
        let (document, _, _) = load_document(&fixture.gltf_path);
        let scene = document.default_scene().expect("default scene");
        let nodes =
            ModelManager::flatten_level_scene_nodes(scene, fixture.gltf_path.to_str().unwrap())
                .expect("flatten scene");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "root");
        assert_eq!(nodes[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(nodes[0].scale, [2.0, 3.0, 4.0]);
        assert_eq!(nodes[1].name, "child");
        assert_eq!(nodes[1].position, [9.0, 17.0, 27.0]);
        assert_eq!(nodes[1].scale, [1.0, 6.0, 6.0]);
        assert_eq!(nodes[1].rotation, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn flatten_level_scene_nodes_falls_back_to_generated_names() {
        let fixture = write_fixture(
            "unnamed",
            r#"{
                "asset": { "version": "2.0" },
                "scene": 0,
                "scenes": [
                    { "nodes": [0] }
                ],
                "nodes": [
                    { "children": [1] },
                    { "translation": [2, 0, 0] }
                ]
            }"#,
            None,
        );
        let (document, _, _) = load_document(&fixture.gltf_path);
        let scene = document.default_scene().expect("default scene");
        let nodes =
            ModelManager::flatten_level_scene_nodes(scene, fixture.gltf_path.to_str().unwrap())
                .expect("flatten scene");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "node_0");
        assert_eq!(nodes[1].name, "node_1");
        assert_eq!(nodes[1].position, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn parse_node_model_extracts_triangle_defaults() {
        let mut bin = Vec::new();
        for value in [0.0f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        let fixture = write_fixture(
            "triangle",
            r#"{
                "asset": { "version": "2.0" },
                "scene": 0,
                "scenes": [
                    { "nodes": [0] }
                ],
                "nodes": [
                    { "name": "triangle", "mesh": 0 }
                ],
                "meshes": [
                    {
                        "primitives": [
                            {
                                "attributes": { "POSITION": 0 }
                            }
                        ]
                    }
                ],
                "buffers": [
                    { "byteLength": 36, "uri": "mesh.bin" }
                ],
                "bufferViews": [
                    { "buffer": 0, "byteOffset": 0, "byteLength": 36 }
                ],
                "accessors": [
                    {
                        "bufferView": 0,
                        "componentType": 5126,
                        "count": 3,
                        "type": "VEC3",
                        "min": [0, 0, 0],
                        "max": [2, 1, 0]
                    }
                ]
            }"#,
            Some(&bin),
        );
        let (document, buffers, _) = load_document(&fixture.gltf_path);
        let node = document.nodes().next().expect("triangle node");
        let parsed = ModelManager::parse_node_model(
            &node,
            &UploadedGltfTextures {
                srgb: HashMap::new(),
                linear: HashMap::new(),
            },
            &buffers,
            None,
            fixture.gltf_path.to_str().unwrap(),
        )
        .expect("parse node model")
        .expect("triangle mesh");
        assert!(!parsed.skinned);
        assert_eq!(parsed.primitives.len(), 1);
        assert_eq!(parsed.cpu_positions.len(), 3);
        assert_eq!(parsed.cpu_indices, vec![0, 1, 2]);
        assert_eq!(parsed.cpu_positions[1], [2.0, 0.0, 0.0]);
        assert_eq!(parsed.primitives[0].normals[0], [0.0, 1.0, 0.0]);
        assert_eq!(parsed.primitives[0].uvs[0], [0.0, 0.0]);
        assert_eq!(parsed.primitives[0].tangents[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(parsed.primitives[0].colors[0], [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn generate_tangents_uses_uv_orientation() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = vec![[0.0, 0.0, 1.0]; 3];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let tangents = ModelManager::generate_tangents(&positions, &normals, &uvs, &[0, 1, 2]);
        for tangent in tangents {
            assert!((tangent[0] - 1.0).abs() < 0.000001);
            assert!(tangent[1].abs() < 0.000001);
            assert!(tangent[2].abs() < 0.000001);
            assert_eq!(tangent[3], 1.0);
        }
    }
}
