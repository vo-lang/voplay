use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{mpsc, Arc},
};

use voplay_runtime::{
    control::StableControlRef,
    platform_backend::PlatformAdapterError,
    render_commands::{
        decode_frame_commands, decode_target_frame_bundle, is_target_frame_bundle,
        portable_frame_commands, DrawCommand,
    },
    render_graph::{ResourceUsage, TextureFormat},
    render_readback::{
        convert_rgba16f_readback, convert_rgba8_readback, ReadbackFormat, ReadbackRegion,
        ReadbackRequestId,
    },
    surface::{
        GameSurfaceId, PresentOutcome, RenderBackendReadback, RenderFrameSubmission, SurfaceMetrics,
    },
};
use wgpu::util::DeviceExt;

use crate::{NativeRenderDevice, NativeRenderSubmission, NativeSurfaceBinding};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuRenderDeviceConfig {
    pub max_surfaces: usize,
    pub max_pixels_per_surface: usize,
    pub max_textures: usize,
    pub max_texture_bytes: usize,
    pub max_targets: usize,
    pub max_target_pixels: usize,
}

impl Default for WgpuRenderDeviceConfig {
    fn default() -> Self {
        Self {
            max_surfaces: 16,
            max_pixels_per_surface: 16_777_216,
            max_textures: 4096,
            max_texture_bytes: 512 * 1024 * 1024,
            max_targets: 2048,
            max_target_pixels: 268_435_456,
        }
    }
}

struct SurfaceTexture {
    surface: GameSurfaceId,
    metrics: SurfaceMetrics,
    texture: wgpu::Texture,
}

#[derive(Clone, Copy)]
struct RectVertex {
    position: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct SpriteVertex {
    position: [f32; 2],
    uv: [f32; 2],
    tint: [f32; 4],
}

struct GpuTexture {
    width: u32,
    height: u32,
    byte_len: usize,
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

struct GpuRenderTarget {
    width: u32,
    height: u32,
    color_format: TextureFormat,
    sample_count: u8,
    texture: wgpu::Texture,
    multisampled: Option<wgpu::Texture>,
}

struct GpuDepthTarget {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
}

struct PendingGpuReadback {
    target: StableControlRef,
    target_revision: u64,
    source_row_bytes: u32,
    width: u32,
    height: u32,
    format: ReadbackFormat,
    source_format: ReadbackFormat,
    buffer: wgpu::Buffer,
    ready: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

pub struct WgpuRenderDevice {
    config: WgpuRenderDeviceConfig,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    rect_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    sprite_layout: wgpu::BindGroupLayout,
    sprite_sampler: wgpu::Sampler,
    device_generation: u64,
    next_surface_token: u64,
    next_fence_value: u64,
    surfaces: BTreeMap<u64, SurfaceTexture>,
    textures: BTreeMap<u64, GpuTexture>,
    texture_bytes: usize,
    targets: BTreeMap<StableControlRef, GpuRenderTarget>,
    depth_targets: BTreeMap<StableControlRef, GpuDepthTarget>,
    external_targets: BTreeMap<StableControlRef, u64>,
    target_revisions: BTreeMap<StableControlRef, u64>,
    target_usages: BTreeMap<StableControlRef, ResourceUsage>,
    readbacks: BTreeMap<ReadbackRequestId, PendingGpuReadback>,
}

impl WgpuRenderDevice {
    pub fn new(
        config: WgpuRenderDeviceConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        device_generation: u64,
    ) -> Result<Self, PlatformAdapterError> {
        if config.max_surfaces == 0
            || config.max_pixels_per_surface == 0
            || config.max_textures == 0
            || config.max_texture_bytes == 0
            || config.max_targets == 0
            || config.max_target_pixels == 0
            || device_generation == 0
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let rect_pipeline = create_rect_pipeline(&device, wgpu::TextureFormat::Rgba8UnormSrgb, 1);
        let sprite_layout = create_sprite_layout(&device);
        let sprite_pipeline = create_sprite_pipeline(
            &device,
            &sprite_layout,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            1,
        );
        let sprite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voplay-sprite-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Ok(Self {
            config,
            device,
            queue,
            rect_pipeline,
            sprite_pipeline,
            sprite_layout,
            sprite_sampler,
            device_generation,
            next_surface_token: 1,
            next_fence_value: 1,
            surfaces: BTreeMap::new(),
            textures: BTreeMap::new(),
            texture_bytes: 0,
            targets: BTreeMap::new(),
            depth_targets: BTreeMap::new(),
            external_targets: BTreeMap::new(),
            target_revisions: BTreeMap::new(),
            target_usages: BTreeMap::new(),
            readbacks: BTreeMap::new(),
        })
    }

    pub fn texture(&self, texture_token: u64) -> Option<&wgpu::Texture> {
        self.surfaces
            .get(&texture_token)
            .map(|record| &record.texture)
    }

    pub fn prepare_scene_target(
        &mut self,
        binding: NativeSurfaceBinding,
        target: StableControlRef,
        width: u32,
        height: u32,
        external_surface: bool,
        content_revision: u64,
        color_format: TextureFormat,
        depth_format: Option<TextureFormat>,
        sample_count: u8,
        usage: ResourceUsage,
    ) -> Result<wgpu::TextureView, PlatformAdapterError> {
        self.validate_binding(binding)?;
        if !target.engine.is_valid()
            || !target.handle.is_valid()
            || width == 0
            || height == 0
            || content_revision == 0
            || !matches!(
                color_format,
                TextureFormat::Rgba8 | TextureFormat::Rgba16Float
            )
            || !matches!(depth_format, None | Some(TextureFormat::Depth32Float))
            || !matches!(sample_count, 1 | 2 | 4 | 8)
            || external_surface && (color_format != TextureFormat::Rgba8 || sample_count != 1)
            || depth_format.is_some() && sample_count != 1
            || usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 == 0
            || (usage.0 & ResourceUsage::DEPTH_ATTACHMENT.0 != 0) != depth_format.is_some()
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        if external_surface {
            let metrics = self
                .surfaces
                .get(&binding.surface_token)
                .ok_or(PlatformAdapterError::SurfaceLost)?
                .metrics;
            if metrics.width != width || metrics.height != height {
                return Err(PlatformAdapterError::RejectedBeforeSubmit);
            }
            self.external_targets.insert(target, binding.surface_token);
            self.prepare_depth_target(target, width, height, depth_format);
            self.target_revisions.insert(target, content_revision);
            self.target_usages.insert(target, usage);
            return Ok(self
                .surfaces
                .get(&binding.surface_token)
                .ok_or(PlatformAdapterError::SurfaceLost)?
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let metrics = SurfaceMetrics {
            width,
            height,
            scale_numerator: 1,
            scale_denominator: 1,
        };
        validate_metrics(metrics, self.config.max_target_pixels)?;
        if !self.targets.contains_key(&target) && self.targets.len() == self.config.max_targets {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let replace = self.targets.get(&target).is_none_or(|current| {
            current.width != width
                || current.height != height
                || current.color_format != color_format
                || current.sample_count != sample_count
        });
        if replace {
            self.targets.insert(
                target,
                GpuRenderTarget {
                    width,
                    height,
                    color_format,
                    sample_count,
                    texture: create_target_texture(&self.device, width, height, color_format),
                    multisampled: create_multisampled_target_texture(
                        &self.device,
                        width,
                        height,
                        color_format,
                        sample_count,
                    ),
                },
            );
        }
        self.prepare_depth_target(target, width, height, depth_format);
        self.target_revisions.insert(target, content_revision);
        self.target_usages.insert(target, usage);
        Ok(self
            .targets
            .get(&target)
            .expect("scene target inserted before view creation")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default()))
    }

    fn prepare_depth_target(
        &mut self,
        target: StableControlRef,
        width: u32,
        height: u32,
        depth_format: Option<TextureFormat>,
    ) {
        if depth_format.is_none() {
            self.depth_targets.remove(&target);
            return;
        }
        let replace = self
            .depth_targets
            .get(&target)
            .is_none_or(|current| current.width != width || current.height != height);
        if replace {
            self.depth_targets.insert(
                target,
                GpuDepthTarget {
                    width,
                    height,
                    texture: create_depth_target_texture(&self.device, width, height),
                },
            );
        }
    }

    pub fn scene_target_depth_texture(
        &self,
        binding: NativeSurfaceBinding,
        target: StableControlRef,
    ) -> Result<Option<&wgpu::Texture>, PlatformAdapterError> {
        self.validate_binding(binding)?;
        Ok(self.depth_targets.get(&target).map(|depth| &depth.texture))
    }

    pub fn scene_target_texture(
        &self,
        binding: NativeSurfaceBinding,
        target: StableControlRef,
        external_surface: bool,
    ) -> Result<&wgpu::Texture, PlatformAdapterError> {
        self.validate_binding(binding)?;
        if external_surface {
            let token = self
                .external_targets
                .get(&target)
                .copied()
                .ok_or(PlatformAdapterError::SurfaceLost)?;
            if token != binding.surface_token {
                return Err(PlatformAdapterError::SurfaceLost);
            }
            return self
                .surfaces
                .get(&token)
                .map(|record| &record.texture)
                .ok_or(PlatformAdapterError::SurfaceLost);
        }
        self.targets
            .get(&target)
            .map(|record| &record.texture)
            .ok_or(PlatformAdapterError::SurfaceLost)
    }

    pub fn finish_scene_submission(
        &self,
        binding: NativeSurfaceBinding,
        frame: &RenderFrameSubmission,
        fence_value: u64,
    ) -> Result<NativeRenderSubmission, PlatformAdapterError> {
        self.validate_binding(binding)?;
        let record = self
            .surfaces
            .get(&binding.surface_token)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        if record.surface != frame.surface || fence_value == 0 {
            return Err(PlatformAdapterError::SurfaceLost);
        }
        Ok(NativeRenderSubmission {
            fence_value,
            texture_token: binding.surface_token,
            device_generation: binding.device_generation,
            content_revision: frame.required_render_revision,
        })
    }

    pub fn upload_rgba_texture(
        &mut self,
        texture_id: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        let byte_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(4))
            .filter(|length| *length == pixels.len())
            .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
        if texture_id == 0
            || width == 0
            || height == 0
            || (!self.textures.contains_key(&texture_id)
                && self.textures.len() == self.config.max_textures)
            || self
                .texture_bytes
                .checked_sub(
                    self.textures
                        .get(&texture_id)
                        .map_or(0, |texture| texture.byte_len),
                )
                .and_then(|bytes| bytes.checked_add(byte_len))
                .is_none_or(|bytes| bytes > self.config.max_texture_bytes)
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-sprite-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
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
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voplay-sprite-texture-bind-group"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sprite_sampler),
                },
            ],
        });
        let previous = self.textures.insert(
            texture_id,
            GpuTexture {
                width,
                height,
                byte_len,
                _texture: texture,
                bind_group,
            },
        );
        self.texture_bytes = self
            .texture_bytes
            .saturating_sub(previous.map_or(0, |texture| texture.byte_len))
            + byte_len;
        Ok(())
    }

    pub fn remove_texture(&mut self, texture_id: u64) -> bool {
        let Some(texture) = self.textures.remove(&texture_id) else {
            return false;
        };
        self.texture_bytes -= texture.byte_len;
        true
    }

    pub fn request_target_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        if request.0 == 0
            || self.readbacks.contains_key(&request)
            || self.target_revisions.get(&target).copied() != Some(expected_target_revision)
            || self
                .target_usages
                .get(&target)
                .is_none_or(|usage| usage.0 & ResourceUsage::READBACK.0 == 0)
            || region.width == 0
            || region.height == 0
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let depth_readback = format == ReadbackFormat::Depth32Float;
        let (texture, width, height, source_format) = if depth_readback {
            let target = self
                .depth_targets
                .get(&target)
                .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
            (
                &target.texture,
                target.width,
                target.height,
                ReadbackFormat::Depth32Float,
            )
        } else if let Some(surface_token) = self.external_targets.get(&target) {
            let surface = self
                .surfaces
                .get(surface_token)
                .ok_or(PlatformAdapterError::SurfaceLost)?;
            (
                &surface.texture,
                surface.metrics.width,
                surface.metrics.height,
                ReadbackFormat::Rgba8,
            )
        } else {
            let target = self
                .targets
                .get(&target)
                .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
            (
                &target.texture,
                target.width,
                target.height,
                match target.color_format {
                    TextureFormat::Rgba8 => ReadbackFormat::Rgba8,
                    TextureFormat::Rgba16Float => ReadbackFormat::Rgba16Float,
                    TextureFormat::Depth24 | TextureFormat::Depth32Float => {
                        return Err(PlatformAdapterError::RejectedBeforeSubmit);
                    }
                },
            )
        };
        if region
            .x
            .checked_add(region.width)
            .is_none_or(|end| end > width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|end| end > height)
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let row_bytes = region
            .width
            .checked_mul(u32::try_from(source_format.bytes_per_pixel()).unwrap_or(u32::MAX))
            .and_then(|bytes| bytes.checked_add(255))
            .map(|bytes| bytes & !255)
            .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
        let byte_len = u64::from(row_bytes)
            .checked_mul(u64::from(region.height))
            .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voplay-render-target-readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("voplay-render-target-readback-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.x,
                    y: region.y,
                    z: 0,
                },
                aspect: if depth_readback {
                    wgpu::TextureAspect::DepthOnly
                } else {
                    wgpu::TextureAspect::All
                },
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(region.height),
                },
            },
            wgpu::Extent3d {
                width: region.width,
                height: region.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (sender, ready) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.readbacks.insert(
            request,
            PendingGpuReadback {
                target,
                target_revision: expected_target_revision,
                source_row_bytes: row_bytes,
                width: region.width,
                height: region.height,
                format,
                source_format,
                buffer,
                ready,
            },
        );
        Ok(())
    }

    pub fn poll_target_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        self.device.poll(wgpu::Maintain::Poll);
        let Some(pending) = self.readbacks.get(&request) else {
            return Ok(None);
        };
        match pending.ready.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) | Ok(Err(_)) => {
                self.readbacks.remove(&request);
                return Err(PlatformAdapterError::OutcomeUnknown);
            }
            Ok(Ok(())) => {}
        }
        let pending = self
            .readbacks
            .remove(&request)
            .expect("ready readback remains registered");
        let bytes = pending.buffer.slice(..).get_mapped_range().to_vec();
        pending.buffer.unmap();
        let (row_bytes, bytes) = if pending.source_format == ReadbackFormat::Depth32Float {
            (pending.source_row_bytes, bytes)
        } else if pending.source_format == ReadbackFormat::Rgba16Float {
            convert_rgba16f_readback(
                &bytes,
                pending.source_row_bytes,
                pending.width,
                pending.height,
                pending.format,
            )
            .ok_or(PlatformAdapterError::OutcomeUnknown)?
        } else {
            convert_rgba8_readback(
                &bytes,
                pending.source_row_bytes,
                pending.width,
                pending.height,
                pending.format,
            )
            .ok_or(PlatformAdapterError::OutcomeUnknown)?
        };
        Ok(Some(RenderBackendReadback {
            request,
            target: pending.target,
            target_revision: pending.target_revision,
            row_bytes,
            bytes,
        }))
    }

    fn create_texture(
        &self,
        metrics: SurfaceMetrics,
    ) -> Result<wgpu::Texture, PlatformAdapterError> {
        validate_metrics(metrics, self.config.max_pixels_per_surface)?;
        Ok(self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-surface-layer"),
            size: wgpu::Extent3d {
                width: metrics.width,
                height: metrics.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        }))
    }
}

impl NativeRenderDevice for WgpuRenderDevice {
    fn shutdown(&mut self) -> Result<(), PlatformAdapterError> {
        self.readbacks.clear();
        self.surfaces.clear();
        self.textures.clear();
        self.texture_bytes = 0;
        self.targets.clear();
        self.depth_targets.clear();
        self.external_targets.clear();
        self.target_revisions.clear();
        self.target_usages.clear();
        Ok(())
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), PlatformAdapterError> {
        let live = targets.iter().copied().collect::<BTreeSet<_>>();
        self.targets.retain(|target, _| live.contains(target));
        self.depth_targets.retain(|target, _| live.contains(target));
        self.external_targets
            .retain(|target, _| live.contains(target));
        self.target_revisions
            .retain(|target, _| live.contains(target));
        self.target_usages.retain(|target, _| live.contains(target));
        Ok(())
    }

    fn upload_rgba_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        WgpuRenderDevice::upload_rgba_texture(self, texture, width, height, pixels)
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), PlatformAdapterError> {
        WgpuRenderDevice::remove_texture(self, texture);
        Ok(())
    }

    fn request_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        self.request_target_readback(request, target, expected_target_revision, region, format)
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        self.poll_target_readback(request)
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), PlatformAdapterError> {
        self.readbacks.remove(&request);
        Ok(())
    }

    fn attach(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<NativeSurfaceBinding, PlatformAdapterError> {
        if self.surfaces.len() == self.config.max_surfaces {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let texture = self.create_texture(metrics)?;
        let surface_token = self.next_surface_token;
        self.next_surface_token = self
            .next_surface_token
            .checked_add(1)
            .ok_or(PlatformAdapterError::DeviceLost)?;
        self.surfaces.insert(
            surface_token,
            SurfaceTexture {
                surface,
                metrics,
                texture,
            },
        );
        Ok(NativeSurfaceBinding {
            surface_token,
            device_generation: self.device_generation,
        })
    }

    fn resize(
        &mut self,
        binding: NativeSurfaceBinding,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError> {
        self.validate_binding(binding)?;
        let texture = self.create_texture(metrics)?;
        let record = self
            .surfaces
            .get_mut(&binding.surface_token)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        record.metrics = metrics;
        record.texture = texture;
        Ok(())
    }

    fn submit(
        &mut self,
        binding: NativeSurfaceBinding,
        frame: &RenderFrameSubmission,
    ) -> Result<NativeRenderSubmission, PlatformAdapterError> {
        self.validate_binding(binding)?;
        let record = self
            .surfaces
            .get(&binding.surface_token)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        if record.surface != frame.surface {
            return Err(PlatformAdapterError::SurfaceLost);
        }
        let command_bytes = portable_frame_commands(&frame.commands)
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
        if is_target_frame_bundle(command_bytes) {
            let targets = decode_target_frame_bundle(
                command_bytes,
                frame.surface.engine,
                self.config.max_targets,
                1_000_000,
                self.config.max_texture_bytes,
            )
            .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            if targets
                .iter()
                .filter(|target| target.external_surface)
                .count()
                != 1
            {
                return Err(PlatformAdapterError::RejectedBeforeSubmit);
            }
            for target in targets {
                if target.depth_format.is_some()
                    || target.usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 == 0
                    || target.external_surface
                        && (target.color_format != TextureFormat::Rgba8 || target.sample_count != 1)
                {
                    return Err(PlatformAdapterError::RejectedBeforeSubmit);
                }
                self.target_revisions
                    .insert(target.target, frame.required_render_revision);
                self.target_usages.insert(target.target, target.usage);
                let metrics = SurfaceMetrics {
                    width: target.width,
                    height: target.height,
                    scale_numerator: 1,
                    scale_denominator: 1,
                };
                if target.external_surface {
                    if target.width != record.metrics.width
                        || target.height != record.metrics.height
                    {
                        return Err(PlatformAdapterError::RejectedBeforeSubmit);
                    }
                    self.external_targets
                        .insert(target.target, binding.surface_token);
                    encode_draw_commands(
                        &self.device,
                        &self.queue,
                        &self.rect_pipeline,
                        &self.sprite_pipeline,
                        &self.sprite_layout,
                        &self.sprite_sampler,
                        &self.textures,
                        &self.targets,
                        &self.target_usages,
                        Some(target.target),
                        &record.texture,
                        None,
                        TextureFormat::Rgba8,
                        1,
                        record.metrics,
                        self.config.max_pixels_per_surface,
                        &target.commands,
                    )?;
                } else {
                    validate_metrics(metrics, self.config.max_target_pixels)?;
                    if !self.targets.contains_key(&target.target)
                        && self.targets.len() == self.config.max_targets
                    {
                        return Err(PlatformAdapterError::RejectedBeforeSubmit);
                    }
                    let replace = self.targets.get(&target.target).is_none_or(|current| {
                        current.width != target.width
                            || current.height != target.height
                            || current.color_format != target.color_format
                            || current.sample_count != target.sample_count
                    });
                    if replace {
                        self.targets.insert(
                            target.target,
                            GpuRenderTarget {
                                width: target.width,
                                height: target.height,
                                color_format: target.color_format,
                                sample_count: target.sample_count,
                                texture: create_target_texture(
                                    &self.device,
                                    target.width,
                                    target.height,
                                    target.color_format,
                                ),
                                multisampled: create_multisampled_target_texture(
                                    &self.device,
                                    target.width,
                                    target.height,
                                    target.color_format,
                                    target.sample_count,
                                ),
                            },
                        );
                    }
                    let target_texture = &self
                        .targets
                        .get(&target.target)
                        .expect("target inserted before render")
                        .texture;
                    encode_draw_commands(
                        &self.device,
                        &self.queue,
                        &self.rect_pipeline,
                        &self.sprite_pipeline,
                        &self.sprite_layout,
                        &self.sprite_sampler,
                        &self.textures,
                        &self.targets,
                        &self.target_usages,
                        Some(target.target),
                        target_texture,
                        self.targets
                            .get(&target.target)
                            .and_then(|target| target.multisampled.as_ref()),
                        target.color_format,
                        target.sample_count,
                        metrics,
                        self.config.max_target_pixels,
                        &target.commands,
                    )?;
                }
            }
        } else {
            let commands = decode_frame_commands(command_bytes)
                .map_err(|_| PlatformAdapterError::RejectedBeforeSubmit)?;
            encode_draw_commands(
                &self.device,
                &self.queue,
                &self.rect_pipeline,
                &self.sprite_pipeline,
                &self.sprite_layout,
                &self.sprite_sampler,
                &self.textures,
                &self.targets,
                &self.target_usages,
                None,
                &record.texture,
                None,
                TextureFormat::Rgba8,
                1,
                record.metrics,
                self.config.max_pixels_per_surface,
                &commands,
            )?;
        }
        let fence_value = self.next_fence_value;
        self.next_fence_value = self
            .next_fence_value
            .checked_add(1)
            .ok_or(PlatformAdapterError::DeviceLost)?;
        Ok(NativeRenderSubmission {
            fence_value,
            texture_token: binding.surface_token,
            device_generation: binding.device_generation,
            content_revision: frame.required_render_revision,
        })
    }

    fn present(
        &mut self,
        binding: NativeSurfaceBinding,
        _fence_value: u64,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, PlatformAdapterError> {
        self.validate_binding(binding)?;
        Ok(if now_micros > deadline_micros {
            PresentOutcome::DeadlineMissed
        } else {
            PresentOutcome::Presented
        })
    }

    fn detach(&mut self, binding: NativeSurfaceBinding) -> Result<(), PlatformAdapterError> {
        self.validate_binding(binding)?;
        self.surfaces
            .remove(&binding.surface_token)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        let detached_targets = self
            .external_targets
            .iter()
            .filter_map(|(target, token)| (*token == binding.surface_token).then_some(*target))
            .collect::<Vec<_>>();
        self.external_targets
            .retain(|_, token| *token != binding.surface_token);
        for target in detached_targets {
            self.depth_targets.remove(&target);
            self.target_revisions.remove(&target);
            self.target_usages.remove(&target);
        }
        Ok(())
    }
}

fn create_target_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay-offscreen-render-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_color_format(format).expect("validated color target format"),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn create_multisampled_target_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: TextureFormat,
    sample_count: u8,
) -> Option<wgpu::Texture> {
    (sample_count > 1).then(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voplay-multisampled-render-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: u32::from(sample_count),
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_color_format(format).expect("validated color target format"),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    })
}

fn wgpu_color_format(format: TextureFormat) -> Result<wgpu::TextureFormat, PlatformAdapterError> {
    match format {
        TextureFormat::Rgba8 => Ok(wgpu::TextureFormat::Rgba8UnormSrgb),
        TextureFormat::Rgba16Float => Ok(wgpu::TextureFormat::Rgba16Float),
        TextureFormat::Depth24 | TextureFormat::Depth32Float => {
            Err(PlatformAdapterError::RejectedBeforeSubmit)
        }
    }
}

fn create_depth_target_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("voplay-depth-render-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

impl WgpuRenderDevice {
    fn validate_binding(&self, binding: NativeSurfaceBinding) -> Result<(), PlatformAdapterError> {
        if binding.device_generation != self.device_generation {
            return Err(PlatformAdapterError::DeviceLost);
        }
        if !self.surfaces.contains_key(&binding.surface_token) {
            return Err(PlatformAdapterError::SurfaceLost);
        }
        Ok(())
    }
}

const RECT_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

const SPRITE_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}

@group(0) @binding(0)
var sprite_texture: texture_2d<f32>;

@group(0) @binding(1)
var sprite_sampler: sampler;

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.uv = input.uv;
    output.tint = input.tint;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(sprite_texture, sprite_sampler, input.uv) * input.tint;
}
"#;

fn create_rect_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("voplay-portable-rect-shader"),
        source: wgpu::ShaderSource::Wgsl(RECT_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voplay-portable-rect-layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voplay-portable-rect-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn create_sprite_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("voplay-sprite-layout"),
        entries: &[
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
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn create_sprite_pipeline(
    device: &wgpu::Device,
    sprite_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("voplay-sprite-shader"),
        source: wgpu::ShaderSource::Wgsl(SPRITE_SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voplay-sprite-pipeline-layout"),
        bind_group_layouts: &[sprite_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("voplay-sprite-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn validate_metrics(
    metrics: SurfaceMetrics,
    max_pixels: usize,
) -> Result<usize, PlatformAdapterError> {
    if !metrics.is_valid() {
        return Err(PlatformAdapterError::RejectedBeforeSubmit);
    }
    (metrics.width as usize)
        .checked_mul(metrics.height as usize)
        .filter(|pixels| *pixels <= max_pixels)
        .ok_or(PlatformAdapterError::RejectedBeforeSubmit)
}

fn encode_draw_commands(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rect_pipeline: &wgpu::RenderPipeline,
    sprite_pipeline: &wgpu::RenderPipeline,
    sprite_layout: &wgpu::BindGroupLayout,
    sprite_sampler: &wgpu::Sampler,
    textures: &BTreeMap<u64, GpuTexture>,
    sampled_targets: &BTreeMap<StableControlRef, GpuRenderTarget>,
    target_usages: &BTreeMap<StableControlRef, ResourceUsage>,
    consumer: Option<StableControlRef>,
    texture: &wgpu::Texture,
    multisampled: Option<&wgpu::Texture>,
    color_format: TextureFormat,
    sample_count: u8,
    metrics: SurfaceMetrics,
    max_pixels: usize,
    commands: &[DrawCommand],
) -> Result<(), PlatformAdapterError> {
    validate_metrics(metrics, max_pixels)?;
    let gpu_format = wgpu_color_format(color_format)?;
    let owned_rect_pipeline = (color_format != TextureFormat::Rgba8 || sample_count != 1)
        .then(|| create_rect_pipeline(device, gpu_format, u32::from(sample_count)));
    let owned_sprite_pipeline =
        (color_format != TextureFormat::Rgba8 || sample_count != 1).then(|| {
            create_sprite_pipeline(device, sprite_layout, gpu_format, u32::from(sample_count))
        });
    let rect_pipeline = owned_rect_pipeline.as_ref().unwrap_or(rect_pipeline);
    let sprite_pipeline = owned_sprite_pipeline.as_ref().unwrap_or(sprite_pipeline);
    let resolved_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let multisampled_view =
        multisampled.map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
    let view = multisampled_view.as_ref().unwrap_or(&resolved_view);
    let resolve_target = multisampled_view.as_ref().map(|_| &resolved_view);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("voplay-portable-frame"),
    });
    clear_target(&mut encoder, view, resolve_target, [0, 0, 0, 0]);
    let mut vertices = Vec::new();
    for command in commands {
        match *command {
            DrawCommand::Clear { color } => {
                flush_rects(
                    device,
                    rect_pipeline,
                    view,
                    resolve_target,
                    &mut encoder,
                    &mut vertices,
                );
                clear_target(&mut encoder, view, resolve_target, color);
            }
            DrawCommand::SolidRect {
                x,
                y,
                width,
                height,
                color,
            } => {
                let x_end = x.saturating_add(width).min(metrics.width);
                let y_end = y.saturating_add(height).min(metrics.height);
                let x = x.min(metrics.width);
                let y = y.min(metrics.height);
                if x < x_end && y < y_end {
                    append_rect_vertices(&mut vertices, metrics, x, y, x_end, y_end, color);
                }
            }
            DrawCommand::SolidTriangle { points, color } => {
                append_triangle_vertices(&mut vertices, metrics, points, color);
            }
            DrawCommand::TexturedRect {
                texture,
                x,
                y,
                width,
                height,
                source_x,
                source_y,
                source_width,
                source_height,
                tint,
            } => {
                flush_rects(
                    device,
                    rect_pipeline,
                    view,
                    resolve_target,
                    &mut encoder,
                    &mut vertices,
                );
                let texture = textures
                    .get(&texture)
                    .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
                draw_sprite(
                    device,
                    sprite_pipeline,
                    view,
                    resolve_target,
                    &mut encoder,
                    texture.width,
                    texture.height,
                    &texture.bind_group,
                    metrics,
                    x,
                    y,
                    width,
                    height,
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    tint,
                )?;
            }
            DrawCommand::SampledTargetRect {
                target: source,
                x,
                y,
                width,
                height,
                source_x,
                source_y,
                source_width,
                source_height,
                tint,
            } => {
                flush_rects(
                    device,
                    rect_pipeline,
                    view,
                    resolve_target,
                    &mut encoder,
                    &mut vertices,
                );
                let source = StableControlRef {
                    engine: consumer
                        .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?
                        .engine,
                    kind: voplay_runtime::control::ControlKind::RenderTarget,
                    handle: source,
                };
                let sampled = sampled_targets
                    .get(&source)
                    .ok_or(PlatformAdapterError::RejectedBeforeSubmit)?;
                if target_usages
                    .get(&source)
                    .is_none_or(|usage| usage.0 & ResourceUsage::SAMPLED.0 == 0)
                {
                    return Err(PlatformAdapterError::RejectedBeforeSubmit);
                }
                let sampled_view = sampled
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("voplay-sampled-target-bind-group"),
                    layout: sprite_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&sampled_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sprite_sampler),
                        },
                    ],
                });
                draw_sprite(
                    device,
                    sprite_pipeline,
                    view,
                    resolve_target,
                    &mut encoder,
                    sampled.width,
                    sampled.height,
                    &bind_group,
                    metrics,
                    x,
                    y,
                    width,
                    height,
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    tint,
                )?;
            }
        }
    }
    flush_rects(
        device,
        rect_pipeline,
        view,
        resolve_target,
        &mut encoder,
        &mut vertices,
    );
    let command_buffer = encoder.finish();
    queue.submit([command_buffer]);
    Ok(())
}

fn clear_target(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    resolve_target: Option<&wgpu::TextureView>,
    color: [u8; 4],
) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("voplay-portable-clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu_color(color)),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
}

fn append_rect_vertices(
    vertices: &mut Vec<RectVertex>,
    metrics: SurfaceMetrics,
    x: u32,
    y: u32,
    x_end: u32,
    y_end: u32,
    color: [u8; 4],
) {
    let left = x as f32 / metrics.width as f32 * 2.0 - 1.0;
    let right = x_end as f32 / metrics.width as f32 * 2.0 - 1.0;
    let top = 1.0 - y as f32 / metrics.height as f32 * 2.0;
    let bottom = 1.0 - y_end as f32 / metrics.height as f32 * 2.0;
    let color = normalized_color(color);
    vertices.extend_from_slice(&[
        RectVertex {
            position: [left, top],
            color,
        },
        RectVertex {
            position: [left, bottom],
            color,
        },
        RectVertex {
            position: [right, bottom],
            color,
        },
        RectVertex {
            position: [left, top],
            color,
        },
        RectVertex {
            position: [right, bottom],
            color,
        },
        RectVertex {
            position: [right, top],
            color,
        },
    ]);
}

fn append_triangle_vertices(
    vertices: &mut Vec<RectVertex>,
    metrics: SurfaceMetrics,
    points: [[i32; 2]; 3],
    color: [u8; 4],
) {
    let color = normalized_color(color);
    for point in points {
        vertices.push(RectVertex {
            position: [
                point[0] as f32 / metrics.width as f32 * 2.0 - 1.0,
                1.0 - point[1] as f32 / metrics.height as f32 * 2.0,
            ],
            color,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sprite(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    target: &wgpu::TextureView,
    resolve_target: Option<&wgpu::TextureView>,
    encoder: &mut wgpu::CommandEncoder,
    texture_width: u32,
    texture_height: u32,
    bind_group: &wgpu::BindGroup,
    metrics: SurfaceMetrics,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    tint: [u8; 4],
) -> Result<(), PlatformAdapterError> {
    if width == 0
        || height == 0
        || source_width == 0
        || source_height == 0
        || source_x.saturating_add(source_width) > texture_width
        || source_y.saturating_add(source_height) > texture_height
    {
        return Err(PlatformAdapterError::RejectedBeforeSubmit);
    }
    let x_end = x.saturating_add(width).min(metrics.width);
    let y_end = y.saturating_add(height).min(metrics.height);
    let x = x.min(metrics.width);
    let y = y.min(metrics.height);
    if x == x_end || y == y_end {
        return Ok(());
    }
    let left = x as f32 / metrics.width as f32 * 2.0 - 1.0;
    let right = x_end as f32 / metrics.width as f32 * 2.0 - 1.0;
    let top = 1.0 - y as f32 / metrics.height as f32 * 2.0;
    let bottom = 1.0 - y_end as f32 / metrics.height as f32 * 2.0;
    let u0 = source_x as f32 / texture_width as f32;
    let v0 = source_y as f32 / texture_height as f32;
    let u1 = source_x.saturating_add(source_width) as f32 / texture_width as f32;
    let v1 = source_y.saturating_add(source_height) as f32 / texture_height as f32;
    let tint = normalized_color(tint);
    let vertices = [
        SpriteVertex {
            position: [left, top],
            uv: [u0, v0],
            tint,
        },
        SpriteVertex {
            position: [left, bottom],
            uv: [u0, v1],
            tint,
        },
        SpriteVertex {
            position: [right, bottom],
            uv: [u1, v1],
            tint,
        },
        SpriteVertex {
            position: [left, top],
            uv: [u0, v0],
            tint,
        },
        SpriteVertex {
            position: [right, bottom],
            uv: [u1, v1],
            tint,
        },
        SpriteVertex {
            position: [right, top],
            uv: [u1, v0],
            tint,
        },
    ];
    let mut bytes = Vec::with_capacity(vertices.len() * 32);
    for vertex in vertices {
        for value in vertex
            .position
            .into_iter()
            .chain(vertex.uv)
            .chain(vertex.tint)
        {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("voplay-sprite-vertices"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("voplay-sprite-pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.set_vertex_buffer(0, buffer.slice(..));
    pass.draw(0..6, 0..1);
    Ok(())
}

fn flush_rects(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    view: &wgpu::TextureView,
    resolve_target: Option<&wgpu::TextureView>,
    encoder: &mut wgpu::CommandEncoder,
    vertices: &mut Vec<RectVertex>,
) {
    if vertices.is_empty() {
        return;
    }
    let mut bytes = Vec::with_capacity(vertices.len() * 24);
    for vertex in vertices.iter() {
        for value in vertex.position.into_iter().chain(vertex.color) {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("voplay-portable-rect-vertices"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("voplay-portable-rect-pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_vertex_buffer(0, buffer.slice(..));
    pass.draw(0..vertices.len() as u32, 0..1);
    drop(pass);
    vertices.clear();
}

fn normalized_color(color: [u8; 4]) -> [f32; 4] {
    [
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0,
    ]
}

fn wgpu_color(color: [u8; 4]) -> wgpu::Color {
    let color = normalized_color(color);
    wgpu::Color {
        r: color[0] as f64,
        g: color[1] as f64,
        b: color[2] as f64,
        a: color[3] as f64,
    }
}
