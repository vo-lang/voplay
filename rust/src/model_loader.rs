//! glTF/GLB model loader.
//!
//! Loads 3D models via the `gltf` crate, extracts mesh geometry
//! (position + normal + UV), and uploads to GPU buffers.
//! Materials are simplified to a base color + texture.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use crate::texture::{TextureId, TextureManager};

/// Opaque model handle matching Vo's ModelID.
pub type ModelId = u32;

/// A single sub-mesh within a model (one draw call).
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub material: MeshMaterial,
}

/// Simplified material for Blinn-Phong rendering.
pub struct MeshMaterial {
    pub base_color: [f32; 4],     // RGBA
    pub texture_id: Option<TextureId>, // albedo texture, if any
}

/// A loaded model: one or more sub-meshes.
pub struct GpuModel {
    pub meshes: Vec<GpuMesh>,
}

/// Interleaved vertex format: position (3) + normal (3) + UV (2) = 8 floats.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl MeshVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x3, // position
        1 => Float32x3, // normal
        2 => Float32x2, // uv
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

/// Manages all loaded models, keyed by ModelId.
pub struct ModelManager {
    models: HashMap<ModelId, GpuModel>,
    next_id: AtomicU32,
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            models: HashMap::new(),
            next_id: AtomicU32::new(1), // 0 = no model
        }
    }

    /// Load a model from a file path (glTF or GLB).
    pub fn load_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        path: &str,
    ) -> Result<ModelId, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("read model '{}': {}", path, e))?;
        self.load_bytes(device, queue, texture_manager, &data, Some(path))
    }

    /// Load a model from raw glTF/GLB bytes.
    pub fn load_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_manager: &mut TextureManager,
        data: &[u8],
        source_path: Option<&str>,
    ) -> Result<ModelId, String> {
        let (document, buffers, images) = gltf::import_slice(data)
            .map_err(|e| format!("gltf import: {}", e))?;

        // Upload embedded textures
        let mut tex_map: HashMap<usize, TextureId> = HashMap::new();
        for (idx, image) in images.iter().enumerate() {
            let rgba = match image.format {
                gltf::image::Format::R8G8B8A8 => image.pixels.clone(),
                gltf::image::Format::R8G8B8 => {
                    // Convert RGB → RGBA
                    let mut rgba = Vec::with_capacity(image.pixels.len() / 3 * 4);
                    for chunk in image.pixels.chunks(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    rgba
                }
                _ => continue, // skip unsupported formats
            };
            let tex_id = texture_manager.load_rgba(
                device, queue, image.width, image.height, &rgba,
            );
            tex_map.insert(idx, tex_id);
        }

        let mut gpu_meshes = Vec::new();

        for mesh in document.meshes() {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                // Positions (required)
                let positions: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(iter) => iter.collect(),
                    None => continue,
                };

                // Normals (default to up if missing)
                let normals: Vec<[f32; 3]> = match reader.read_normals() {
                    Some(iter) => iter.collect(),
                    None => vec![[0.0, 1.0, 0.0]; positions.len()],
                };

                // UVs (default to 0,0 if missing)
                let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                    Some(iter) => iter.into_f32().collect(),
                    None => vec![[0.0, 0.0]; positions.len()],
                };

                // Interleave vertices
                let vertices: Vec<MeshVertex> = positions.iter().enumerate().map(|(i, pos)| {
                    MeshVertex {
                        position: *pos,
                        normal: normals[i],
                        uv: uvs[i],
                    }
                }).collect();

                // Indices
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(iter) => iter.into_u32().collect(),
                    None => (0..vertices.len() as u32).collect(),
                };

                // Material
                let mat = primitive.material();
                let pbr = mat.pbr_metallic_roughness();
                let base_color = pbr.base_color_factor();
                let texture_id = pbr.base_color_texture()
                    .and_then(|info| tex_map.get(&info.texture().source().index()).copied());

                // Create GPU buffers
                let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("voplay_mesh_vb"),
                    size: (vertices.len() * std::mem::size_of::<MeshVertex>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));

                let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("voplay_mesh_ib"),
                    size: (indices.len() * std::mem::size_of::<u32>()) as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&indices));

                gpu_meshes.push(GpuMesh {
                    vertex_buffer,
                    index_buffer,
                    index_count: indices.len() as u32,
                    material: MeshMaterial {
                        base_color,
                        texture_id,
                    },
                });
            }
        }

        if gpu_meshes.is_empty() {
            return Err(format!(
                "model '{}' contains no renderable meshes",
                source_path.unwrap_or("<bytes>")
            ));
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.models.insert(id, GpuModel { meshes: gpu_meshes });
        Ok(id)
    }

    /// Free a model by ID.
    pub fn free(&mut self, id: ModelId) {
        self.models.remove(&id);
    }

    /// Get a model by ID.
    pub fn get(&self, id: ModelId) -> Option<&GpuModel> {
        self.models.get(&id)
    }
}
