use std::collections::{BTreeMap, BTreeSet};

use crate::{
    control::StableControlRef,
    render_commands::{
        decode_frame_commands, decode_target_frame_bundle, is_target_frame_bundle,
        portable_frame_commands, DrawCommand,
    },
    render_graph::{ResourceUsage, TextureFormat},
    render_readback::{convert_rgba8_readback, ReadbackFormat, ReadbackRegion, ReadbackRequestId},
    surface::{
        GameSurfaceId, PresentOutcome, RenderBackend, RenderBackendFailure, RenderBackendReadback,
        RenderFrameAck, RenderFrameSubmission, SurfaceMetrics,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadlessRendererConfig {
    pub max_surfaces: usize,
    pub max_framebuffer_bytes: usize,
    pub max_textures: usize,
    pub max_texture_bytes: usize,
}

impl Default for HeadlessRendererConfig {
    fn default() -> Self {
        Self {
            max_surfaces: 16,
            max_framebuffer_bytes: 64 * 1024 * 1024,
            max_textures: 4096,
            max_texture_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeadlessRendererError {
    InvalidConfig,
    InvalidSurface,
    SurfaceCapacity,
    FramebufferCapacity,
    TextureCapacity,
    InvalidTexture,
    Closed,
}

struct HeadlessSurface {
    metrics: SurfaceMetrics,
    pixels: Vec<u8>,
    pending: Option<RenderFrameAck>,
    presented_frame: u64,
}

#[derive(Clone)]
struct HeadlessTexture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

pub struct HeadlessRenderBackend {
    config: HeadlessRendererConfig,
    surfaces: BTreeMap<GameSurfaceId, HeadlessSurface>,
    targets: BTreeMap<StableControlRef, HeadlessSurface>,
    external_targets: BTreeMap<StableControlRef, GameSurfaceId>,
    target_revisions: BTreeMap<StableControlRef, u64>,
    target_usages: BTreeMap<StableControlRef, ResourceUsage>,
    readbacks: BTreeMap<ReadbackRequestId, RenderBackendReadback>,
    textures: BTreeMap<u64, HeadlessTexture>,
    texture_bytes: usize,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadlessRenderBackendOwnerSnapshot {
    pub closed: bool,
    pub surfaces: usize,
    pub pending_frames: usize,
    pub targets: usize,
    pub readbacks: usize,
    pub textures: usize,
    pub texture_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadlessRenderBackendShutdownReport {
    pub before: HeadlessRenderBackendOwnerSnapshot,
    pub after: HeadlessRenderBackendOwnerSnapshot,
}

impl HeadlessRenderBackend {
    pub fn new(config: HeadlessRendererConfig) -> Result<Self, HeadlessRendererError> {
        if config.max_surfaces == 0
            || config.max_framebuffer_bytes == 0
            || config.max_textures == 0
            || config.max_texture_bytes == 0
        {
            return Err(HeadlessRendererError::InvalidConfig);
        }
        Ok(Self {
            config,
            surfaces: BTreeMap::new(),
            targets: BTreeMap::new(),
            external_targets: BTreeMap::new(),
            target_revisions: BTreeMap::new(),
            target_usages: BTreeMap::new(),
            readbacks: BTreeMap::new(),
            textures: BTreeMap::new(),
            texture_bytes: 0,
            closed: false,
        })
    }

    pub fn upload_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<(), HeadlessRendererError> {
        if self.closed {
            return Err(HeadlessRendererError::Closed);
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(4))
            .filter(|expected| *expected == pixels.len())
            .ok_or(HeadlessRendererError::InvalidTexture)?;
        if texture == 0 || width == 0 || height == 0 {
            return Err(HeadlessRendererError::InvalidTexture);
        }
        let previous_bytes = self
            .textures
            .get(&texture)
            .map_or(0, |texture| texture.pixels.len());
        if (!self.textures.contains_key(&texture)
            && self.textures.len() == self.config.max_textures)
            || self
                .texture_bytes
                .checked_sub(previous_bytes)
                .and_then(|bytes| bytes.checked_add(expected))
                .is_none_or(|bytes| bytes > self.config.max_texture_bytes)
        {
            return Err(HeadlessRendererError::TextureCapacity);
        }
        self.texture_bytes = self.texture_bytes - previous_bytes + expected;
        self.textures.insert(
            texture,
            HeadlessTexture {
                width,
                height,
                pixels,
            },
        );
        Ok(())
    }

    pub fn remove_texture(&mut self, texture: u64) -> bool {
        if self.closed {
            return false;
        }
        let Some(texture) = self.textures.remove(&texture) else {
            return false;
        };
        self.texture_bytes -= texture.pixels.len();
        true
    }

    pub fn pixels(&self, surface: GameSurfaceId) -> Option<&[u8]> {
        self.surfaces
            .get(&surface)
            .map(|surface| surface.pixels.as_slice())
    }

    pub fn presented_frame(&self, surface: GameSurfaceId) -> Option<u64> {
        self.surfaces
            .get(&surface)
            .map(|surface| surface.presented_frame)
    }

    pub fn owner_snapshot(&self) -> HeadlessRenderBackendOwnerSnapshot {
        HeadlessRenderBackendOwnerSnapshot {
            closed: self.closed,
            surfaces: self.surfaces.len(),
            pending_frames: self
                .surfaces
                .values()
                .filter(|surface| surface.pending.is_some())
                .count(),
            targets: self.targets.len(),
            readbacks: self.readbacks.len(),
            textures: self.textures.len(),
            texture_bytes: self.texture_bytes,
        }
    }

    pub fn shutdown(&mut self) -> HeadlessRenderBackendShutdownReport {
        let before = self.owner_snapshot();
        if !self.closed {
            self.surfaces.clear();
            self.targets.clear();
            self.external_targets.clear();
            self.target_revisions.clear();
            self.target_usages.clear();
            self.readbacks.clear();
            self.textures.clear();
            self.texture_bytes = 0;
            self.closed = true;
        }
        HeadlessRenderBackendShutdownReport {
            before,
            after: self.owner_snapshot(),
        }
    }

    fn allocate_pixels(&self, metrics: SurfaceMetrics) -> Result<Vec<u8>, RenderBackendFailure> {
        self.ensure_open()?;
        let length = (metrics.width as usize)
            .checked_mul(metrics.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|bytes| *bytes <= self.config.max_framebuffer_bytes)
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        Ok(vec![0; length])
    }

    fn ensure_open(&self) -> Result<(), RenderBackendFailure> {
        if self.closed {
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        } else {
            Ok(())
        }
    }
}

impl RenderBackend for HeadlessRenderBackend {
    fn shutdown(&mut self) -> Result<(), RenderBackendFailure> {
        HeadlessRenderBackend::shutdown(self);
        Ok(())
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let live = targets.iter().copied().collect::<BTreeSet<_>>();
        self.targets.retain(|target, _| live.contains(target));
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
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.upload_texture(texture, width, height, pixels.to_vec())
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.remove_texture(texture);
        Ok(())
    }

    fn request_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if request.0 == 0
            || self.readbacks.contains_key(&request)
            || format == ReadbackFormat::Depth32Float
            || self.target_revisions.get(&target).copied() != Some(expected_target_revision)
            || self
                .target_usages
                .get(&target)
                .is_none_or(|usage| usage.0 & ResourceUsage::READBACK.0 == 0)
        {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let source = if let Some(surface) = self.external_targets.get(&target) {
            self.surfaces.get(surface)
        } else {
            self.targets.get(&target)
        }
        .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        let end_x = region
            .x
            .checked_add(region.width)
            .filter(|end| *end <= source.metrics.width)
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        let end_y = region
            .y
            .checked_add(region.height)
            .filter(|end| *end <= source.metrics.height)
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        if region.width == 0 || region.height == 0 {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let source_row_bytes = region
            .width
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(255))
            .map(|bytes| bytes & !255)
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        let byte_len = (source_row_bytes as usize)
            .checked_mul(region.height as usize)
            .filter(|bytes| *bytes <= self.config.max_framebuffer_bytes)
            .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        let mut bytes = vec![0; byte_len];
        for (output_row, source_row) in (region.y..end_y).enumerate() {
            let source_start =
                (source_row as usize * source.metrics.width as usize + region.x as usize) * 4;
            let source_end = source_start + (end_x - region.x) as usize * 4;
            let output_start = output_row * source_row_bytes as usize;
            bytes[output_start..output_start + (source_end - source_start)]
                .copy_from_slice(&source.pixels[source_start..source_end]);
        }
        let (row_bytes, bytes) = convert_rgba8_readback(
            &bytes,
            source_row_bytes,
            region.width,
            region.height,
            format,
        )
        .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
        self.readbacks.insert(
            request,
            RenderBackendReadback {
                request,
                target,
                target_revision: expected_target_revision,
                row_bytes,
                bytes,
            },
        );
        Ok(())
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, RenderBackendFailure> {
        self.ensure_open()?;
        Ok(self.readbacks.remove(&request))
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.readbacks.remove(&request);
        Ok(())
    }

    fn attach_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        if self.surfaces.contains_key(&surface) || self.surfaces.len() == self.config.max_surfaces {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        let pixels = self.allocate_pixels(metrics)?;
        self.surfaces.insert(
            surface,
            HeadlessSurface {
                metrics,
                pixels,
                pending: None,
                presented_frame: 0,
            },
        );
        Ok(())
    }

    fn resize_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let pixels = self.allocate_pixels(metrics)?;
        let target = self
            .surfaces
            .get_mut(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if target.pending.is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        target.metrics = metrics;
        target.pixels = pixels;
        Ok(())
    }

    fn rebind_surface(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        let pixels = self.allocate_pixels(metrics)?;
        let target = self
            .surfaces
            .get_mut(&surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        target.metrics = metrics;
        target.pixels = pixels;
        target.pending = None;
        Ok(())
    }

    fn render(
        &mut self,
        submission: &RenderFrameSubmission,
    ) -> Result<RenderFrameAck, RenderBackendFailure> {
        self.ensure_open()?;
        let command_bytes = portable_frame_commands(&submission.commands)
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
        let surface = self
            .surfaces
            .get(&submission.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if surface.pending.is_some() {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        if is_target_frame_bundle(command_bytes) {
            let targets = decode_target_frame_bundle(
                command_bytes,
                submission.surface.engine,
                2048,
                1_000_000,
                self.config.max_framebuffer_bytes,
            )
            .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
            let external_count = targets
                .iter()
                .filter(|target| target.external_surface)
                .count();
            if external_count != 1 {
                return Err(RenderBackendFailure::RejectedBeforeSubmit);
            }
            for target in targets {
                if target.color_format != TextureFormat::Rgba8
                    || target.depth_format.is_some()
                    || target.sample_count != 1
                    || target.usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 == 0
                {
                    return Err(RenderBackendFailure::RejectedBeforeSubmit);
                }
                self.target_revisions
                    .insert(target.target, submission.required_render_revision);
                self.target_usages.insert(target.target, target.usage);
                let sampled_targets = collect_sampled_targets(
                    &target.commands,
                    submission.surface.engine,
                    &self.targets,
                    &self.external_targets,
                    &self.surfaces,
                )?;
                if target.external_surface {
                    let surface = self
                        .surfaces
                        .get_mut(&submission.surface)
                        .ok_or(RenderBackendFailure::SurfaceLost)?;
                    if target.width != surface.metrics.width
                        || target.height != surface.metrics.height
                    {
                        return Err(RenderBackendFailure::RejectedBeforeSubmit);
                    }
                    self.external_targets
                        .insert(target.target, submission.surface);
                    raster_commands(surface, &self.textures, &sampled_targets, target.commands)?;
                } else {
                    let metrics = SurfaceMetrics {
                        width: target.width,
                        height: target.height,
                        scale_numerator: 1,
                        scale_denominator: 1,
                    };
                    let required_bytes = (target.width as usize)
                        .checked_mul(target.height as usize)
                        .and_then(|pixels| pixels.checked_mul(4))
                        .filter(|bytes| *bytes <= self.config.max_framebuffer_bytes)
                        .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
                    let offscreen =
                        self.targets
                            .entry(target.target)
                            .or_insert_with(|| HeadlessSurface {
                                metrics,
                                pixels: vec![0; required_bytes],
                                pending: None,
                                presented_frame: 0,
                            });
                    if offscreen.metrics != metrics {
                        offscreen.metrics = metrics;
                        offscreen.pixels = vec![0; required_bytes];
                    }
                    raster_commands(offscreen, &self.textures, &sampled_targets, target.commands)?;
                }
            }
        } else {
            let commands = decode_frame_commands(command_bytes)
                .map_err(|_| RenderBackendFailure::RejectedBeforeSubmit)?;
            let surface = self
                .surfaces
                .get_mut(&submission.surface)
                .ok_or(RenderBackendFailure::SurfaceLost)?;
            raster_commands(surface, &self.textures, &BTreeMap::new(), commands)?;
        }
        let ack = RenderFrameAck {
            surface: submission.surface,
            pulse_id: submission.pulse_id,
            frame_id: submission.frame_id,
            render_endpoint: submission.render_endpoint,
            device_generation: submission.device_generation,
            rendered_revision: submission.required_render_revision,
            observed_control_revision: submission.required_control_revision,
        };
        self.surfaces
            .get_mut(&submission.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?
            .pending = Some(ack);
        Ok(ack)
    }

    fn present(
        &mut self,
        ack: RenderFrameAck,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, RenderBackendFailure> {
        self.ensure_open()?;
        let target = self
            .surfaces
            .get_mut(&ack.surface)
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        if target.pending != Some(ack) {
            return Err(RenderBackendFailure::RejectedBeforeSubmit);
        }
        target.pending = None;
        if now_micros > deadline_micros {
            return Ok(PresentOutcome::DeadlineMissed);
        }
        target.presented_frame = ack.frame_id;
        Ok(PresentOutcome::Presented)
    }

    fn detach_surface(&mut self, surface: GameSurfaceId) -> Result<(), RenderBackendFailure> {
        self.ensure_open()?;
        self.surfaces
            .remove(&surface)
            .map(|_| ())
            .ok_or(RenderBackendFailure::SurfaceLost)?;
        self.external_targets.retain(|_, owner| *owner != surface);
        Ok(())
    }
}

fn raster_commands(
    target: &mut HeadlessSurface,
    textures: &BTreeMap<u64, HeadlessTexture>,
    sampled_targets: &BTreeMap<voplay_protocol::Handle, HeadlessTexture>,
    commands: Vec<DrawCommand>,
) -> Result<(), RenderBackendFailure> {
    for command in commands {
        match command {
            DrawCommand::Clear { color } => {
                for pixel in target.pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&color);
                }
            }
            DrawCommand::SolidRect {
                x,
                y,
                width,
                height,
                color,
            } => raster_rect(target, x, y, width, height, color),
            DrawCommand::SolidTriangle { points, color } => raster_triangle(target, points, color),
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
                let texture = textures
                    .get(&texture)
                    .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
                raster_texture(
                    target,
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
                let texture = sampled_targets
                    .get(&source)
                    .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
                raster_texture(
                    target,
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
                )?;
            }
        }
    }
    Ok(())
}

fn collect_sampled_targets(
    commands: &[DrawCommand],
    engine: voplay_protocol::EngineId,
    targets: &BTreeMap<StableControlRef, HeadlessSurface>,
    external_targets: &BTreeMap<StableControlRef, GameSurfaceId>,
    surfaces: &BTreeMap<GameSurfaceId, HeadlessSurface>,
) -> Result<BTreeMap<voplay_protocol::Handle, HeadlessTexture>, RenderBackendFailure> {
    let mut sampled = BTreeMap::new();
    for command in commands {
        let DrawCommand::SampledTargetRect { target: handle, .. } = command else {
            continue;
        };
        if sampled.contains_key(handle) {
            continue;
        }
        let target = StableControlRef {
            engine,
            kind: crate::control::ControlKind::RenderTarget,
            handle: *handle,
        };
        let source = if let Some(source) = targets.get(&target) {
            source
        } else {
            let surface = external_targets
                .get(&target)
                .and_then(|surface| surfaces.get(surface))
                .ok_or(RenderBackendFailure::RejectedBeforeSubmit)?;
            surface
        };
        sampled.insert(
            *handle,
            HeadlessTexture {
                width: source.metrics.width,
                height: source.metrics.height,
                pixels: source.pixels.clone(),
            },
        );
    }
    Ok(sampled)
}

fn raster_rect(
    target: &mut HeadlessSurface,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    let end_x = x.saturating_add(width).min(target.metrics.width);
    let end_y = y.saturating_add(height).min(target.metrics.height);
    for row in y.min(target.metrics.height)..end_y {
        for column in x.min(target.metrics.width)..end_x {
            let offset = ((row as usize) * (target.metrics.width as usize) + column as usize) * 4;
            target.pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn raster_triangle(target: &mut HeadlessSurface, points: [[i32; 2]; 3], color: [u8; 4]) {
    let maximum_x = i32::try_from(target.metrics.width).unwrap_or(i32::MAX);
    let maximum_y = i32::try_from(target.metrics.height).unwrap_or(i32::MAX);
    let min_x = points
        .iter()
        .map(|point| point[0])
        .min()
        .unwrap_or(0)
        .max(0);
    let max_x = points
        .iter()
        .map(|point| point[0])
        .max()
        .unwrap_or(0)
        .min(maximum_x.saturating_sub(1));
    let min_y = points
        .iter()
        .map(|point| point[1])
        .min()
        .unwrap_or(0)
        .max(0);
    let max_y = points
        .iter()
        .map(|point| point[1])
        .max()
        .unwrap_or(0)
        .min(maximum_y.saturating_sub(1));
    let area = triangle_edge(points[0], points[1], points[2]);
    if area == 0 || min_x > max_x || min_y > max_y {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let sample = [x, y];
            let edges = [
                triangle_edge(points[0], points[1], sample),
                triangle_edge(points[1], points[2], sample),
                triangle_edge(points[2], points[0], sample),
            ];
            if edges.iter().all(|edge| *edge >= 0) || edges.iter().all(|edge| *edge <= 0) {
                let offset = (y as usize * target.metrics.width as usize + x as usize) * 4;
                target.pixels[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }
}

fn triangle_edge(from: [i32; 2], to: [i32; 2], point: [i32; 2]) -> i128 {
    (i128::from(point[0]) - i128::from(from[0])) * (i128::from(to[1]) - i128::from(from[1]))
        - (i128::from(point[1]) - i128::from(from[1])) * (i128::from(to[0]) - i128::from(from[0]))
}

#[allow(clippy::too_many_arguments)]
fn raster_texture(
    target: &mut HeadlessSurface,
    texture: &HeadlessTexture,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    tint: [u8; 4],
) -> Result<(), RenderBackendFailure> {
    if width == 0
        || height == 0
        || source_width == 0
        || source_height == 0
        || source_x.saturating_add(source_width) > texture.width
        || source_y.saturating_add(source_height) > texture.height
    {
        return Err(RenderBackendFailure::RejectedBeforeSubmit);
    }
    let end_x = x.saturating_add(width).min(target.metrics.width);
    let end_y = y.saturating_add(height).min(target.metrics.height);
    for target_y in y.min(target.metrics.height)..end_y {
        let source_row =
            source_y + ((target_y - y) as u64 * source_height as u64 / height as u64) as u32;
        for target_x in x.min(target.metrics.width)..end_x {
            let source_column =
                source_x + ((target_x - x) as u64 * source_width as u64 / width as u64) as u32;
            let source_offset =
                ((source_row as usize * texture.width as usize) + source_column as usize) * 4;
            let target_offset =
                ((target_y as usize * target.metrics.width as usize) + target_x as usize) * 4;
            blend_tinted_pixel(
                &mut target.pixels[target_offset..target_offset + 4],
                &texture.pixels[source_offset..source_offset + 4],
                tint,
            );
        }
    }
    Ok(())
}

fn blend_tinted_pixel(target: &mut [u8], source: &[u8], tint: [u8; 4]) {
    let source_alpha = u32::from(source[3]) * u32::from(tint[3]) / 255;
    let inverse_alpha = 255 - source_alpha;
    for channel in 0..3 {
        let tinted = u32::from(source[channel]) * u32::from(tint[channel]) / 255;
        target[channel] =
            ((tinted * source_alpha + u32::from(target[channel]) * inverse_alpha) / 255) as u8;
    }
    target[3] = (source_alpha + u32::from(target[3]) * inverse_alpha / 255).min(255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::PresentationDomainId;
    use vo_app_protocol::SurfaceHandle;
    use voplay_protocol::{EngineId, Handle};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn surface() -> GameSurfaceId {
        let engine: EngineId = handle(1);
        GameSurfaceId {
            engine,
            surface: SurfaceHandle {
                index: 2,
                generation: 1,
            },
            domain: PresentationDomainId {
                engine,
                handle: handle(3),
            },
        }
    }

    fn commands() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"VFC1");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&[0, 0, 255, 255]);
        bytes.push(2);
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&[255, 0, 0, 255]);
        bytes
    }

    fn submission(
        surface: GameSurfaceId,
        frame_id: u64,
        commands: Vec<u8>,
    ) -> RenderFrameSubmission {
        RenderFrameSubmission {
            surface,
            pulse_id: frame_id,
            frame_id,
            render_endpoint: handle(4),
            device_generation: 1,
            required_render_revision: frame_id,
            required_control_revision: frame_id,
            graph_signature: 1,
            commands,
        }
    }

    #[test]
    fn rasterizes_portable_commands_and_requires_exact_render_present_pairing() {
        let id = surface();
        let metrics = SurfaceMetrics {
            width: 2,
            height: 1,
            scale_numerator: 1,
            scale_denominator: 1,
        };
        let mut backend = HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap();
        backend.attach_surface(id, metrics).unwrap();
        let ack = backend.render(&submission(id, 1, commands())).unwrap();
        assert_eq!(
            backend.pixels(id).unwrap(),
            &[0, 0, 255, 255, 255, 0, 0, 255]
        );
        assert_eq!(
            backend.render(&submission(id, 2, commands())),
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        );
        assert_eq!(
            backend.present(ack, 11, 10),
            Ok(PresentOutcome::DeadlineMissed)
        );
        assert_eq!(backend.presented_frame(id), Some(0));

        let ack = backend.render(&submission(id, 2, commands())).unwrap();
        assert_eq!(backend.present(ack, 10, 10), Ok(PresentOutcome::Presented));
        assert_eq!(backend.presented_frame(id), Some(2));
    }

    #[test]
    fn malformed_frame_and_invalid_texture_fail_before_surface_publication() {
        let id = surface();
        let metrics = SurfaceMetrics {
            width: 1,
            height: 1,
            scale_numerator: 1,
            scale_denominator: 1,
        };
        let mut backend = HeadlessRenderBackend::new(HeadlessRendererConfig::default()).unwrap();
        backend.attach_surface(id, metrics).unwrap();
        assert_eq!(
            backend.render(&submission(id, 1, b"bad".to_vec())),
            Err(RenderBackendFailure::RejectedBeforeSubmit)
        );
        assert_eq!(backend.pixels(id), Some(&[0, 0, 0, 0][..]));
        assert_eq!(
            backend.upload_texture(1, 2, 2, vec![0; 15]),
            Err(HeadlessRendererError::InvalidTexture)
        );
    }
}
