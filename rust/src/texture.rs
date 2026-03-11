//! GPU texture manager — load, store, and free textures.
//!
//! Textures are identified by integer IDs (TextureID on the Vo side).
//! Images are decoded via the `image` crate and uploaded as wgpu textures
//! with associated bind groups for sampling in the sprite pipeline.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use crate::file_io;

/// Opaque handle to a GPU texture. Matches Vo's TextureID (int).
pub type TextureId = u32;

pub type CubemapId = u32;

/// A loaded GPU texture with its bind group for sprite rendering.
#[allow(dead_code)]
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

#[allow(dead_code)]
pub struct GpuCubemap {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub face_size: u32,
}

/// Manages all loaded textures, keyed by TextureId.
pub struct TextureManager {
    textures: HashMap<TextureId, GpuTexture>,
    cubemaps: HashMap<CubemapId, GpuCubemap>,
    next_id: AtomicU32,
    next_cubemap_id: AtomicU32,
    sampler: wgpu::Sampler,
    cubemap_sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    cubemap_bind_group_layout: wgpu::BindGroupLayout,
}

impl TextureManager {
    /// Create a new texture manager. The bind group layout is shared with the sprite pipeline.
    pub fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_sprite_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let cubemap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay_cubemap_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_texture_bgl"),
            entries: &[
                // binding 0: texture
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 1: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let cubemap_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voplay_cubemap_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        Self {
            textures: HashMap::new(),
            cubemaps: HashMap::new(),
            next_id: AtomicU32::new(1), // 0 is reserved (no texture)
            next_cubemap_id: AtomicU32::new(1),
            sampler,
            cubemap_sampler,
            bind_group_layout,
            cubemap_bind_group_layout,
        }
    }

    /// Returns the bind group layout for texture sampling (used by sprite pipeline).
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    pub fn cubemap_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.cubemap_bind_group_layout
    }

    /// Load a texture from raw RGBA8 pixel data.
    pub fn load_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba_data: &[u8],
    ) -> TextureId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            rgba_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_texture_bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.textures.insert(id, GpuTexture {
            texture,
            view,
            bind_group,
            width,
            height,
        });

        id
    }

    /// Load a texture from encoded image bytes (PNG, JPEG, etc.).
    pub fn load_image_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
    ) -> Result<TextureId, String> {
        let img = image::load_from_memory(data)
            .map_err(|e| format!("image decode: {}", e))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Ok(self.load_rgba(device, queue, w, h, &rgba))
    }

    /// Load a texture from a file path.
    pub fn load_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
    ) -> Result<TextureId, String> {
        let data = file_io::read_bytes(path)?;
        self.load_image_bytes(device, queue, &data)
    }

    fn load_cubemap_rgba_faces(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        faces: [&[u8]; 6],
        face_size: u32,
    ) -> CubemapId {
        let id = self.next_cubemap_id.fetch_add(1, Ordering::Relaxed);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_cubemap"),
            size: wgpu::Extent3d {
                width: face_size,
                height: face_size,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for (i, face_rgba) in faces.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                face_rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * face_size),
                    rows_per_image: Some(face_size),
                },
                wgpu::Extent3d {
                    width: face_size,
                    height: face_size,
                    depth_or_array_layers: 1,
                },
            );
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay_cubemap_bg"),
            layout: &self.cubemap_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.cubemap_sampler),
                },
            ],
        });

        self.cubemaps.insert(id, GpuCubemap {
            texture,
            view,
            bind_group,
            face_size,
        });

        id
    }

    pub fn load_cubemap_image_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        face_data: [&[u8]; 6],
    ) -> Result<CubemapId, String> {
        let mut decoded: Vec<Vec<u8>> = Vec::with_capacity(6);
        let mut face_size = 0u32;

        for data in face_data {
            let img = image::load_from_memory(data)
                .map_err(|e| format!("cubemap image decode: {}", e))?;
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            if w != h {
                return Err(format!("cubemap face must be square, got {}x{}", w, h));
            }
            if face_size == 0 {
                face_size = w;
            } else if w != face_size {
                return Err(format!("cubemap face size mismatch: expected {}, got {}", face_size, w));
            }
            decoded.push(rgba.into_raw());
        }

        let faces = [
            decoded[0].as_slice(),
            decoded[1].as_slice(),
            decoded[2].as_slice(),
            decoded[3].as_slice(),
            decoded[4].as_slice(),
            decoded[5].as_slice(),
        ];
        Ok(self.load_cubemap_rgba_faces(device, queue, faces, face_size))
    }

    pub fn load_cubemap_files(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paths: [&str; 6],
    ) -> Result<CubemapId, String> {
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(6);
        for path in paths {
            data.push(file_io::read_bytes(path)?);
        }
        let faces = [
            data[0].as_slice(),
            data[1].as_slice(),
            data[2].as_slice(),
            data[3].as_slice(),
            data[4].as_slice(),
            data[5].as_slice(),
        ];
        self.load_cubemap_image_bytes(device, queue, faces)
    }

    /// Re-upload RGBA pixel data to an existing texture. Size must match.
    pub fn update_rgba(&self, queue: &wgpu::Queue, id: TextureId, rgba_data: &[u8]) {
        if let Some(tex) = self.textures.get(&id) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * tex.width),
                    rows_per_image: Some(tex.height),
                },
                wgpu::Extent3d {
                    width: tex.width,
                    height: tex.height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Free a texture by ID, releasing GPU resources.
    pub fn free(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }

    pub fn free_cubemap(&mut self, id: CubemapId) {
        self.cubemaps.remove(&id);
    }

    /// Get a texture by ID.
    pub fn get(&self, id: TextureId) -> Option<&GpuTexture> {
        self.textures.get(&id)
    }

    pub fn get_cubemap(&self, id: CubemapId) -> Option<&GpuCubemap> {
        self.cubemaps.get(&id)
    }
}
