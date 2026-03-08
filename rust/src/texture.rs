//! GPU texture manager — load, store, and free textures.
//!
//! Textures are identified by integer IDs (TextureID on the Vo side).
//! Images are decoded via the `image` crate and uploaded as wgpu textures
//! with associated bind groups for sampling in the sprite pipeline.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Opaque handle to a GPU texture. Matches Vo's TextureID (int).
pub type TextureId = u32;

/// A loaded GPU texture with its bind group for sprite rendering.
#[allow(dead_code)]
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

/// Manages all loaded textures, keyed by TextureId.
pub struct TextureManager {
    textures: HashMap<TextureId, GpuTexture>,
    next_id: AtomicU32,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
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

        Self {
            textures: HashMap::new(),
            next_id: AtomicU32::new(1), // 0 is reserved (no texture)
            sampler,
            bind_group_layout,
        }
    }

    /// Returns the bind group layout for texture sampling (used by sprite pipeline).
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
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
        let data = std::fs::read(path)
            .map_err(|e| format!("read file '{}': {}", path, e))?;
        self.load_image_bytes(device, queue, &data)
    }

    /// Free a texture by ID, releasing GPU resources.
    pub fn free(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }

    /// Get a texture by ID.
    pub fn get(&self, id: TextureId) -> Option<&GpuTexture> {
        self.textures.get(&id)
    }
}
