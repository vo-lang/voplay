use super::*;

impl RenderResourceRegistry {
    pub(crate) fn main_color_ready(&self) -> bool {
        self.actual_texture_view(RES_MAIN_COLOR).is_some()
    }

    pub(crate) fn actual_texture_view(
        &self,
        resource: RenderResource,
    ) -> Option<&wgpu::TextureView> {
        self.store.texture_view(resource)
    }

    pub(crate) fn actual_backing_identity(
        &self,
        resource: RenderResource,
    ) -> Option<RenderBackingIdentity> {
        self.store.backing_identity(resource)
    }

    pub(crate) fn actual_backing_descriptor(
        &self,
        resource: RenderResource,
    ) -> Option<RenderBackingDescriptor> {
        self.store.backing_descriptor(resource)
    }

    pub(crate) fn validate_backing(
        &self,
        resource: RenderResource,
    ) -> Result<(), RenderResourceBackingError> {
        let Some(target) = self
            .targets
            .iter()
            .find(|target| target.resource == resource)
        else {
            return Err(RenderResourceBackingError {
                resource,
                code: "target_missing",
                expected_generation: self.resize_generation,
                actual_generation: 0,
                detail: "registry_target_is_not_declared",
            });
        };
        target.validate_backing(
            self.resize_generation,
            self.actual_backing_identity(resource),
            self.actual_backing_descriptor(resource),
            self.actual_texture_view(resource).is_some(),
        )
    }

    pub(crate) fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.store.depth_view.as_ref()
    }

    pub(crate) fn msaa_color_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_color_view.as_ref()
    }

    pub(crate) fn post_color_view(&self) -> Option<&wgpu::TextureView> {
        self.store.post_color_view.as_ref()
    }

    pub(crate) fn msaa_receiver_mask_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_receiver_mask_view.as_ref()
    }

    pub(crate) fn receiver_mask_view(&self) -> Option<&wgpu::TextureView> {
        self.store.receiver_mask_view.as_ref()
    }

    pub(crate) fn msaa_surface_props_view(&self) -> Option<&wgpu::TextureView> {
        self.store.msaa_surface_props_view.as_ref()
    }

    pub(crate) fn surface_props_view(&self) -> Option<&wgpu::TextureView> {
        self.store.surface_props_view.as_ref()
    }

    #[allow(dead_code)] // owner: voplay/render; expiry: 2026-07-12; exposed for resize/recreate stress probes.
    pub(crate) fn resize_generation(&self) -> u32 {
        self.resize_generation
    }
}
