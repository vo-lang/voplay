use std::collections::BTreeMap;

pub use voplay_runtime::render_world::{
    decode_render_object, encode_render_object, Aabb3, DirtyUpload, DirtyUploadKind,
    RenderComponent, RenderComponentKind, RenderWorld, RenderWorldConfig, RenderWorldError,
    RetainedRenderObject, TransformSample, ViewVisibility,
};
use voplay_runtime::{
    control::StableControlRef,
    platform_backend::{PlatformAdapterError, PlatformSubmission, PlatformSurfaceAdapter},
    render_readback::{ReadbackFormat, ReadbackRegion, ReadbackRequestId},
    surface::{
        GameSurfaceId, PresentOutcome, RenderBackendReadback, RenderFrameSubmission, SurfaceMetrics,
    },
};

#[cfg(feature = "native-wgpu")]
mod wgpu_device;
#[cfg(feature = "native-wgpu")]
pub use wgpu_device::{WgpuRenderDevice, WgpuRenderDeviceConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSurfaceBinding {
    pub surface_token: u64,
    pub device_generation: u64,
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRenderSubmission {
    pub fence_value: u64,
    pub texture_token: u64,
    pub device_generation: u64,
    pub content_revision: u64,
}

pub trait NativeRenderDevice {
    fn shutdown(&mut self) -> Result<(), PlatformAdapterError> {
        Ok(())
    }

    fn upload_rgba_texture(
        &mut self,
        _texture: u64,
        _width: u32,
        _height: u32,
        _pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn remove_texture(&mut self, _texture: u64) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn upsert_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
        _bytes: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn remove_profile_asset(
        &mut self,
        _kind: u32,
        _asset: u64,
        _revision: u64,
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn synchronize_render_targets(
        &mut self,
        _targets: &[StableControlRef],
    ) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn request_readback(
        &mut self,
        _request: ReadbackRequestId,
        _target: StableControlRef,
        _expected_target_revision: u64,
        _region: ReadbackRegion,
        _format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        Err(PlatformAdapterError::RejectedBeforeSubmit)
    }
    fn poll_readback(
        &mut self,
        _request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        Ok(None)
    }
    fn cancel_readback(&mut self, _request: ReadbackRequestId) -> Result<(), PlatformAdapterError> {
        Ok(())
    }
    fn attach(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<NativeSurfaceBinding, PlatformAdapterError>;
    fn resize(
        &mut self,
        binding: NativeSurfaceBinding,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError>;
    fn submit(
        &mut self,
        binding: NativeSurfaceBinding,
        frame: &RenderFrameSubmission,
    ) -> Result<NativeRenderSubmission, PlatformAdapterError>;
    fn present(
        &mut self,
        binding: NativeSurfaceBinding,
        fence_value: u64,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, PlatformAdapterError>;
    fn detach(&mut self, binding: NativeSurfaceBinding) -> Result<(), PlatformAdapterError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeRenderOwnerConfig {
    pub max_surfaces: usize,
    pub max_command_bytes: usize,
}

impl Default for NativeRenderOwnerConfig {
    fn default() -> Self {
        Self {
            max_surfaces: 16,
            max_command_bytes: 64 * 1024 * 1024,
        }
    }
}

pub struct NativeRenderOwner<D> {
    config: NativeRenderOwnerConfig,
    device: D,
    bindings: BTreeMap<GameSurfaceId, NativeSurfaceBinding>,
    composition_outputs: BTreeMap<GameSurfaceId, NativeRenderSubmission>,
}

impl<D: NativeRenderDevice> NativeRenderOwner<D> {
    pub fn new(config: NativeRenderOwnerConfig, device: D) -> Result<Self, PlatformAdapterError> {
        if config.max_surfaces == 0 || config.max_command_bytes == 0 {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        Ok(Self {
            config,
            device,
            bindings: BTreeMap::new(),
            composition_outputs: BTreeMap::new(),
        })
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    pub fn composition_output(&self, surface: GameSurfaceId) -> Option<NativeRenderSubmission> {
        self.composition_outputs.get(&surface).copied()
    }
}

impl<D: NativeRenderDevice> PlatformSurfaceAdapter for NativeRenderOwner<D> {
    fn shutdown(&mut self) -> Result<(), PlatformAdapterError> {
        let bindings = std::mem::take(&mut self.bindings);
        self.composition_outputs.clear();
        let mut first_error = None;
        for binding in bindings.into_values() {
            if let Err(error) = self.device.detach(binding) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.device.shutdown() {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    fn synchronize_render_targets(
        &mut self,
        targets: &[StableControlRef],
    ) -> Result<(), PlatformAdapterError> {
        self.device.synchronize_render_targets(targets)
    }
    fn upload_rgba_texture(
        &mut self,
        texture: u64,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        self.device
            .upload_rgba_texture(texture, width, height, pixels)
    }

    fn remove_texture(&mut self, texture: u64) -> Result<(), PlatformAdapterError> {
        self.device.remove_texture(texture)
    }

    fn upsert_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), PlatformAdapterError> {
        self.device
            .upsert_profile_asset(kind, asset, revision, bytes)
    }

    fn remove_profile_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), PlatformAdapterError> {
        self.device.remove_profile_asset(kind, asset, revision)
    }

    fn request_readback(
        &mut self,
        request: ReadbackRequestId,
        target: StableControlRef,
        expected_target_revision: u64,
        region: ReadbackRegion,
        format: ReadbackFormat,
    ) -> Result<(), PlatformAdapterError> {
        self.device
            .request_readback(request, target, expected_target_revision, region, format)
    }

    fn poll_readback(
        &mut self,
        request: ReadbackRequestId,
    ) -> Result<Option<RenderBackendReadback>, PlatformAdapterError> {
        self.device.poll_readback(request)
    }

    fn cancel_readback(&mut self, request: ReadbackRequestId) -> Result<(), PlatformAdapterError> {
        self.device.cancel_readback(request)
    }

    fn attach(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError> {
        if !metrics.is_valid()
            || self.bindings.contains_key(&surface)
            || self.bindings.len() == self.config.max_surfaces
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let binding = self.device.attach(surface, metrics)?;
        if binding.surface_token == 0 || binding.device_generation == 0 {
            return Err(PlatformAdapterError::DeviceLost);
        }
        self.bindings.insert(surface, binding);
        Ok(())
    }

    fn resize(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError> {
        let binding = *self
            .bindings
            .get(&surface)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        self.device.resize(binding, metrics)?;
        self.composition_outputs.remove(&surface);
        Ok(())
    }

    fn rebind(
        &mut self,
        surface: GameSurfaceId,
        metrics: SurfaceMetrics,
    ) -> Result<(), PlatformAdapterError> {
        let old = self
            .bindings
            .remove(&surface)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        self.device.detach(old)?;
        self.composition_outputs.remove(&surface);
        let binding = self.device.attach(surface, metrics)?;
        if binding.surface_token == 0
            || binding.device_generation == 0
            || binding.device_generation <= old.device_generation
        {
            return Err(PlatformAdapterError::DeviceLost);
        }
        self.bindings.insert(surface, binding);
        Ok(())
    }

    fn submit(
        &mut self,
        frame: &RenderFrameSubmission,
    ) -> Result<PlatformSubmission, PlatformAdapterError> {
        let binding = *self
            .bindings
            .get(&frame.surface)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        if frame.commands.len() > self.config.max_command_bytes
            || frame.device_generation != binding.device_generation
        {
            return Err(PlatformAdapterError::RejectedBeforeSubmit);
        }
        let submission = self.device.submit(binding, frame)?;
        if submission.fence_value == 0
            || submission.texture_token == 0
            || submission.device_generation != binding.device_generation
            || submission.content_revision != frame.required_render_revision
        {
            return Err(PlatformAdapterError::OutcomeUnknown);
        }
        self.composition_outputs.insert(frame.surface, submission);
        Ok(PlatformSubmission {
            fence_value: submission.fence_value,
            device_generation: binding.device_generation,
        })
    }

    fn present(
        &mut self,
        surface: GameSurfaceId,
        submission: PlatformSubmission,
        now_micros: u64,
        deadline_micros: u64,
    ) -> Result<PresentOutcome, PlatformAdapterError> {
        let binding = *self
            .bindings
            .get(&surface)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        if submission.device_generation != binding.device_generation {
            return Err(PlatformAdapterError::DeviceLost);
        }
        self.device
            .present(binding, submission.fence_value, now_micros, deadline_micros)
    }

    fn detach(&mut self, surface: GameSurfaceId) -> Result<(), PlatformAdapterError> {
        let binding = self
            .bindings
            .remove(&surface)
            .ok_or(PlatformAdapterError::SurfaceLost)?;
        self.composition_outputs.remove(&surface);
        self.device.detach(binding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vo_app_protocol::GenerationalHandle;
    use voplay_protocol::Handle;
    use voplay_runtime::outbox::PresentationDomainId;

    #[derive(Default)]
    struct RecordingDevice {
        next_device_generation: u64,
        invalid_submission: bool,
        detached: Vec<NativeSurfaceBinding>,
    }

    impl NativeRenderDevice for RecordingDevice {
        fn attach(
            &mut self,
            surface: GameSurfaceId,
            _metrics: SurfaceMetrics,
        ) -> Result<NativeSurfaceBinding, PlatformAdapterError> {
            self.next_device_generation += 1;
            Ok(NativeSurfaceBinding {
                surface_token: u64::from(surface.surface.index) + 1,
                device_generation: self.next_device_generation,
            })
        }

        fn resize(
            &mut self,
            _binding: NativeSurfaceBinding,
            _metrics: SurfaceMetrics,
        ) -> Result<(), PlatformAdapterError> {
            Ok(())
        }

        fn submit(
            &mut self,
            binding: NativeSurfaceBinding,
            frame: &RenderFrameSubmission,
        ) -> Result<NativeRenderSubmission, PlatformAdapterError> {
            Ok(NativeRenderSubmission {
                fence_value: frame.frame_id,
                texture_token: frame.frame_id + 100,
                device_generation: binding.device_generation,
                content_revision: frame.required_render_revision
                    + u64::from(self.invalid_submission),
            })
        }

        fn present(
            &mut self,
            _binding: NativeSurfaceBinding,
            _fence_value: u64,
            _now_micros: u64,
            _deadline_micros: u64,
        ) -> Result<PresentOutcome, PlatformAdapterError> {
            Ok(PresentOutcome::Presented)
        }

        fn detach(&mut self, binding: NativeSurfaceBinding) -> Result<(), PlatformAdapterError> {
            self.detached.push(binding);
            Ok(())
        }
    }

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn surface() -> GameSurfaceId {
        let engine = handle(1);
        GameSurfaceId {
            engine,
            surface: GenerationalHandle {
                index: 2,
                generation: 1,
            },
            domain: PresentationDomainId {
                engine,
                handle: handle(3),
            },
        }
    }

    fn metrics(width: u32) -> SurfaceMetrics {
        SurfaceMetrics {
            width,
            height: 480,
            scale_numerator: 1,
            scale_denominator: 1,
        }
    }

    fn frame(device_generation: u64, frame_id: u64) -> RenderFrameSubmission {
        RenderFrameSubmission {
            surface: surface(),
            pulse_id: frame_id,
            frame_id,
            render_endpoint: handle(4),
            device_generation,
            required_render_revision: frame_id + 10,
            required_control_revision: 1,
            graph_signature: 9,
            commands: vec![1, 2, 3],
        }
    }

    #[test]
    fn invalid_submission_does_not_replace_last_composition_output() {
        let mut owner = NativeRenderOwner::new(
            NativeRenderOwnerConfig::default(),
            RecordingDevice::default(),
        )
        .unwrap();
        owner.attach(surface(), metrics(640)).unwrap();
        let first = owner.submit(&frame(1, 1)).unwrap();
        assert_eq!(
            owner.present(surface(), first, 1, 2).unwrap(),
            PresentOutcome::Presented
        );
        let committed = owner.composition_output(surface()).unwrap();

        owner.device_mut().invalid_submission = true;
        assert_eq!(
            owner.submit(&frame(1, 2)),
            Err(PlatformAdapterError::OutcomeUnknown)
        );
        assert_eq!(owner.composition_output(surface()), Some(committed));
    }

    #[test]
    fn resize_invalidates_output_and_rebind_advances_device_generation() {
        let mut owner = NativeRenderOwner::new(
            NativeRenderOwnerConfig::default(),
            RecordingDevice::default(),
        )
        .unwrap();
        owner.attach(surface(), metrics(640)).unwrap();
        owner.submit(&frame(1, 1)).unwrap();
        owner.resize(surface(), metrics(800)).unwrap();
        assert_eq!(owner.composition_output(surface()), None);

        owner.rebind(surface(), metrics(800)).unwrap();
        assert_eq!(owner.device().detached.len(), 1);
        let next = owner.submit(&frame(2, 2)).unwrap();
        assert_eq!(next.device_generation, 2);
    }
}
