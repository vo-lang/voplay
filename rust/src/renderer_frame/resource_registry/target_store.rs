use super::super::{
    RenderBackingDescriptor, RenderBackingIdentity, RenderResource, RenderResourceKind,
};

#[derive(Default)]
pub(super) struct RenderTargetStore {
    pub(super) depth_view: Option<wgpu::TextureView>,
    pub(super) msaa_color_view: Option<wgpu::TextureView>,
    pub(super) post_color_view: Option<wgpu::TextureView>,
    pub(super) msaa_receiver_mask_view: Option<wgpu::TextureView>,
    pub(super) receiver_mask_view: Option<wgpu::TextureView>,
    pub(super) msaa_surface_props_view: Option<wgpu::TextureView>,
    pub(super) surface_props_view: Option<wgpu::TextureView>,
    pub(super) depth_identity: Option<RenderBackingIdentity>,
    pub(super) msaa_color_identity: Option<RenderBackingIdentity>,
    pub(super) post_color_identity: Option<RenderBackingIdentity>,
    pub(super) msaa_receiver_mask_identity: Option<RenderBackingIdentity>,
    pub(super) receiver_mask_identity: Option<RenderBackingIdentity>,
    pub(super) msaa_surface_props_identity: Option<RenderBackingIdentity>,
    pub(super) surface_props_identity: Option<RenderBackingIdentity>,
    pub(super) depth_descriptor: Option<RenderBackingDescriptor>,
    pub(super) msaa_color_descriptor: Option<RenderBackingDescriptor>,
    pub(super) post_color_descriptor: Option<RenderBackingDescriptor>,
    pub(super) msaa_receiver_mask_descriptor: Option<RenderBackingDescriptor>,
    pub(super) receiver_mask_descriptor: Option<RenderBackingDescriptor>,
    pub(super) msaa_surface_props_descriptor: Option<RenderBackingDescriptor>,
    pub(super) surface_props_descriptor: Option<RenderBackingDescriptor>,
}

impl RenderTargetStore {
    pub(super) fn texture_view(&self, resource: RenderResource) -> Option<&wgpu::TextureView> {
        match resource.kind {
            RenderResourceKind::MainColor => self
                .msaa_color_view
                .as_ref()
                .or(self.post_color_view.as_ref()),
            RenderResourceKind::Depth => self.depth_view.as_ref(),
            RenderResourceKind::ReceiverMask => self
                .msaa_receiver_mask_view
                .as_ref()
                .or(self.receiver_mask_view.as_ref()),
            RenderResourceKind::SurfaceProps => self
                .msaa_surface_props_view
                .as_ref()
                .or(self.surface_props_view.as_ref()),
            RenderResourceKind::PostColor => self.post_color_view.as_ref(),
            _ => None,
        }
    }

    pub(super) fn backing_identity(
        &self,
        resource: RenderResource,
    ) -> Option<RenderBackingIdentity> {
        match resource.kind {
            RenderResourceKind::MainColor => self.msaa_color_identity.or(self.post_color_identity),
            RenderResourceKind::Depth => self.depth_identity,
            RenderResourceKind::ReceiverMask => self
                .msaa_receiver_mask_identity
                .or(self.receiver_mask_identity),
            RenderResourceKind::SurfaceProps => self
                .msaa_surface_props_identity
                .or(self.surface_props_identity),
            RenderResourceKind::PostColor => self.post_color_identity,
            _ => None,
        }
    }

    pub(super) fn backing_descriptor(
        &self,
        resource: RenderResource,
    ) -> Option<RenderBackingDescriptor> {
        match resource.kind {
            RenderResourceKind::MainColor => {
                self.msaa_color_descriptor.or(self.post_color_descriptor)
            }
            RenderResourceKind::Depth => self.depth_descriptor,
            RenderResourceKind::ReceiverMask => self
                .msaa_receiver_mask_descriptor
                .or(self.receiver_mask_descriptor),
            RenderResourceKind::SurfaceProps => self
                .msaa_surface_props_descriptor
                .or(self.surface_props_descriptor),
            RenderResourceKind::PostColor => self.post_color_descriptor,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backing_identity_selects_the_same_physical_slot_as_the_view_policy() {
        let post = RenderBackingIdentity {
            serial: 11,
            generation: 2,
            slot: super::super::RenderBackingSlot::PostColor,
        };
        let msaa = RenderBackingIdentity {
            serial: 12,
            generation: 2,
            slot: super::super::RenderBackingSlot::MsaaColor,
        };
        let post_descriptor = RenderBackingDescriptor {
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: 1280,
            height: 720,
            sample_count: 1,
        };
        let msaa_descriptor = RenderBackingDescriptor {
            sample_count: 4,
            ..post_descriptor
        };
        let mut store = RenderTargetStore {
            post_color_identity: Some(post),
            post_color_descriptor: Some(post_descriptor),
            ..RenderTargetStore::default()
        };
        assert_eq!(
            store.backing_identity(super::super::RES_MAIN_COLOR),
            Some(post)
        );
        assert_eq!(
            store.backing_descriptor(super::super::RES_MAIN_COLOR),
            Some(post_descriptor)
        );
        store.msaa_color_identity = Some(msaa);
        store.msaa_color_descriptor = Some(msaa_descriptor);
        assert_eq!(
            store.backing_identity(super::super::RES_MAIN_COLOR),
            Some(msaa)
        );
        assert_eq!(
            store.backing_descriptor(super::super::RES_MAIN_COLOR),
            Some(msaa_descriptor)
        );
    }
}
