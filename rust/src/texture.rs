//! GPU texture manager — load, store, and free textures.
//!
//! Textures are identified by integer IDs (TextureID on the Vo side).
//! Images are decoded via the `image` crate and uploaded as wgpu textures
//! with associated bind groups for sampling in the sprite pipeline.

use crate::file_io;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

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
    pub mip_level_count: u32,
    pub pixels: Vec<u8>,
    pub srgb: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexturePixelsData {
    pub width: u32,
    pub height: u32,
    pub srgb: bool,
    pub pixels: Vec<u8>,
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
            anisotropy_clamp: 4,
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

        let cubemap_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        self.load_rgba_with_srgb(device, queue, width, height, rgba_data, true)
    }

    /// Load a texture from raw RGBA8 pixel data with explicit color space.
    pub fn load_rgba_with_srgb(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba_data: &[u8],
        srgb: bool,
    ) -> TextureId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let mip_chain = build_rgba_mip_chain(width, height, rgba_data);
        let mip_level_count = mip_chain.len().max(1) as u32;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
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

        write_rgba_mip_chain(queue, &texture, &mip_chain, mip_level_count);

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

        self.textures.insert(
            id,
            GpuTexture {
                texture,
                view,
                bind_group,
                width,
                height,
                mip_level_count,
                pixels: rgba_data.to_vec(),
                srgb,
            },
        );

        id
    }

    /// Load a texture from encoded image bytes (PNG, JPEG, etc.).
    pub fn load_image_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
    ) -> Result<TextureId, String> {
        self.load_image_bytes_with_srgb(device, queue, data, true)
    }

    /// Load a texture from encoded image bytes with explicit color space.
    pub fn load_image_bytes_with_srgb(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        srgb: bool,
    ) -> Result<TextureId, String> {
        let img = image::load_from_memory(data).map_err(|e| format!("image decode: {}", e))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Ok(self.load_rgba_with_srgb(device, queue, w, h, &rgba, srgb))
    }

    /// Load a texture from a file path.
    pub fn load_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
    ) -> Result<TextureId, String> {
        self.load_file_with_srgb(device, queue, path, true)
    }

    pub fn load_file_with_srgb(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
        srgb: bool,
    ) -> Result<TextureId, String> {
        let data = file_io::read_bytes(path)?;
        self.load_image_bytes_with_srgb(device, queue, &data, srgb)
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
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
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

        self.cubemaps.insert(
            id,
            GpuCubemap {
                texture,
                view,
                bind_group,
                face_size,
            },
        );

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
                return Err(format!(
                    "cubemap face size mismatch: expected {}, got {}",
                    face_size, w
                ));
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
    pub fn update_rgba(&mut self, queue: &wgpu::Queue, id: TextureId, rgba_data: &[u8]) {
        if let Some(tex) = self.textures.get_mut(&id) {
            let mip_chain = build_rgba_mip_chain(tex.width, tex.height, rgba_data);
            write_rgba_mip_chain(queue, &tex.texture, &mip_chain, tex.mip_level_count);
            tex.pixels.clear();
            tex.pixels.extend_from_slice(rgba_data);
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

    pub fn pixels(&self, id: TextureId) -> Option<TexturePixelsData> {
        self.textures.get(&id).map(|texture| TexturePixelsData {
            width: texture.width,
            height: texture.height,
            srgb: texture.srgb,
            pixels: texture.pixels.clone(),
        })
    }

    pub fn get_cubemap(&self, id: CubemapId) -> Option<&GpuCubemap> {
        self.cubemaps.get(&id)
    }
}

fn build_rgba_mip_chain(width: u32, height: u32, rgba_data: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let expected_len = width as usize * height as usize * 4;
    if width == 0 || height == 0 || rgba_data.len() != expected_len {
        return vec![(width.max(1), height.max(1), rgba_data.to_vec())];
    }

    let mut chain = Vec::new();
    chain.push((width, height, rgba_data.to_vec()));

    let mut src_w = width;
    let mut src_h = height;
    while src_w > 1 || src_h > 1 {
        let (_, _, previous) = chain.last().expect("mip chain has base level");
        let dst_w = (src_w / 2).max(1);
        let dst_h = (src_h / 2).max(1);
        let next = downsample_rgba_2x(previous, src_w, src_h, dst_w, dst_h);
        chain.push((dst_w, dst_h, next));
        src_w = dst_w;
        src_h = dst_h;
    }

    chain
}

fn downsample_rgba_2x(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w as usize * dst_h as usize * 4];
    for y in 0..dst_h {
        for x in 0..dst_w {
            let sx0 = (x * 2).min(src_w - 1);
            let sy0 = (y * 2).min(src_h - 1);
            let sx1 = (sx0 + 1).min(src_w - 1);
            let sy1 = (sy0 + 1).min(src_h - 1);
            let samples = [(sx0, sy0), (sx1, sy0), (sx0, sy1), (sx1, sy1)];
            let mut sum = [0u32; 4];
            for (sx, sy) in samples {
                let src_index = ((sy * src_w + sx) * 4) as usize;
                sum[0] += src[src_index] as u32;
                sum[1] += src[src_index + 1] as u32;
                sum[2] += src[src_index + 2] as u32;
                sum[3] += src[src_index + 3] as u32;
            }
            let dst_index = ((y * dst_w + x) * 4) as usize;
            dst[dst_index] = ((sum[0] + 2) / 4) as u8;
            dst[dst_index + 1] = ((sum[1] + 2) / 4) as u8;
            dst[dst_index + 2] = ((sum[2] + 2) / 4) as u8;
            dst[dst_index + 3] = ((sum[3] + 2) / 4) as u8;
        }
    }
    dst
}

fn write_rgba_mip_chain(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip_chain: &[(u32, u32, Vec<u8>)],
    mip_level_count: u32,
) {
    for (level, (width, height, data)) in
        mip_chain.iter().take(mip_level_count as usize).enumerate()
    {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * *width),
                rows_per_image: Some(*height),
            },
            wgpu::Extent3d {
                width: *width,
                height: *height,
                depth_or_array_layers: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::build_rgba_mip_chain;

    #[test]
    fn rgba_mip_chain_halves_until_single_pixel() {
        let rgba = vec![255u8; 5 * 3 * 4];
        let chain = build_rgba_mip_chain(5, 3, &rgba);
        let dims: Vec<(u32, u32)> = chain.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(dims, vec![(5, 3), (2, 1), (1, 1)]);
    }

    #[test]
    fn rgba_mip_chain_averages_four_texels() {
        let rgba = vec![
            0, 0, 0, 255, 100, 0, 0, 255, 0, 100, 0, 255, 100, 100, 100, 255,
        ];
        let chain = build_rgba_mip_chain(2, 2, &rgba);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[1].2, vec![50, 50, 25, 255]);
    }
}
