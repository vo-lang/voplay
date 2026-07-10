use super::{
    RenderBackingDescriptor, RenderBackingIdentity, RenderBackingSlot, RenderResource,
    RenderResourceBackingError, RenderResourceChurn, RenderResourceLifetime, RenderTargetStatus,
};
use crate::renderer_frame_resources::*;
use crate::renderer_targets::{
    create_depth_view, create_msaa_color_view, create_msaa_receiver_mask_view,
    create_msaa_surface_props_view, create_post_color_view, create_receiver_mask_view,
    create_surface_props_view, MAIN_DEPTH_FORMAT, MAIN_SAMPLE_COUNT, RECEIVER_MASK_FORMAT,
    SURFACE_PROPS_FORMAT,
};
mod target_store;
mod view_access;
use target_store::RenderTargetStore;

pub(crate) struct RenderResourceRegistry {
    targets: Vec<RenderTargetStatus>,
    pub(crate) churn: RenderResourceChurn,
    pub(crate) resize_generation: u32,
    store: RenderTargetStore,
    next_backing_serial: u64,
}

impl Default for RenderResourceRegistry {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            churn: RenderResourceChurn::default(),
            resize_generation: 0,
            store: RenderTargetStore::default(),
            next_backing_serial: 1,
        }
    }
}

impl RenderResourceRegistry {
    fn allocate_backing_identity(&mut self, slot: RenderBackingSlot) -> RenderBackingIdentity {
        let identity = RenderBackingIdentity {
            serial: self.next_backing_serial,
            generation: self.resize_generation,
            slot,
        };
        self.next_backing_serial = self.next_backing_serial.saturating_add(1);
        identity
    }

    pub(crate) fn new_render_targets(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let mut registry = Self::default();
        registry.recreate_render_targets(device, width, height, surface_format);
        registry
    }

    pub(crate) fn recreate_render_targets(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) {
        let next_generation = self.resize_generation.saturating_add(1);
        self.mark_resize_generation(next_generation);
        let depth_view = Some(create_depth_view(device, width, height, MAIN_SAMPLE_COUNT));
        let msaa_color_view =
            create_msaa_color_view(device, width, height, surface_format, MAIN_SAMPLE_COUNT);
        let post_color_view = Some(create_post_color_view(
            device,
            width,
            height,
            surface_format,
        ));
        let msaa_receiver_mask_view =
            create_msaa_receiver_mask_view(device, width, height, MAIN_SAMPLE_COUNT);
        let receiver_mask_view = Some(create_receiver_mask_view(
            device,
            width,
            height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_receiver_mask",
        ));
        let msaa_surface_props_view =
            create_msaa_surface_props_view(device, width, height, MAIN_SAMPLE_COUNT);
        let surface_props_view = Some(create_surface_props_view(
            device,
            width,
            height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "voplay_surface_props",
        ));
        let depth_identity = self.allocate_backing_identity(RenderBackingSlot::Depth);
        let msaa_color_identity = msaa_color_view
            .as_ref()
            .map(|_| self.allocate_backing_identity(RenderBackingSlot::MsaaColor));
        let post_color_identity = self.allocate_backing_identity(RenderBackingSlot::PostColor);
        let msaa_receiver_mask_identity = msaa_receiver_mask_view
            .as_ref()
            .map(|_| self.allocate_backing_identity(RenderBackingSlot::MsaaReceiverMask));
        let receiver_mask_identity =
            self.allocate_backing_identity(RenderBackingSlot::ReceiverMask);
        let msaa_surface_props_identity = msaa_surface_props_view
            .as_ref()
            .map(|_| self.allocate_backing_identity(RenderBackingSlot::MsaaSurfaceProps));
        let surface_props_identity =
            self.allocate_backing_identity(RenderBackingSlot::SurfaceProps);
        let descriptor = |format, sample_count| RenderBackingDescriptor {
            format,
            width: width.max(1),
            height: height.max(1),
            sample_count,
        };
        self.store = RenderTargetStore {
            depth_view,
            msaa_color_view,
            post_color_view,
            msaa_receiver_mask_view,
            receiver_mask_view,
            msaa_surface_props_view,
            surface_props_view,
            depth_identity: Some(depth_identity),
            msaa_color_identity,
            post_color_identity: Some(post_color_identity),
            msaa_receiver_mask_identity,
            receiver_mask_identity: Some(receiver_mask_identity),
            msaa_surface_props_identity,
            surface_props_identity: Some(surface_props_identity),
            depth_descriptor: Some(descriptor(MAIN_DEPTH_FORMAT, MAIN_SAMPLE_COUNT)),
            msaa_color_descriptor: msaa_color_identity
                .map(|_| descriptor(surface_format, MAIN_SAMPLE_COUNT)),
            post_color_descriptor: Some(descriptor(surface_format, 1)),
            msaa_receiver_mask_descriptor: msaa_receiver_mask_identity
                .map(|_| descriptor(RECEIVER_MASK_FORMAT, MAIN_SAMPLE_COUNT)),
            receiver_mask_descriptor: Some(descriptor(RECEIVER_MASK_FORMAT, 1)),
            msaa_surface_props_descriptor: msaa_surface_props_identity
                .map(|_| descriptor(SURFACE_PROPS_FORMAT, MAIN_SAMPLE_COUNT)),
            surface_props_descriptor: Some(descriptor(SURFACE_PROPS_FORMAT, 1)),
        };
        self.declare_target(
            RES_DEPTH,
            self.store.depth_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_MAIN_COLOR,
            self.main_color_ready(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_POST_COLOR,
            self.store.post_color_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_RECEIVER_MASK,
            self.store.receiver_mask_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(
            RES_SURFACE_PROPS,
            self.store.surface_props_view.is_some(),
            RenderResourceLifetime::Persistent,
        );
        self.declare_target(RES_SHADOW_MAP, false, RenderResourceLifetime::Persistent);
        self.declare_target(RES_WATER_COLOR, false, RenderResourceLifetime::Transient);
        self.declare_target(RES_OVERLAY, true, RenderResourceLifetime::External);
        self.declare_target_with_owner(
            RES_CAPTURE,
            false,
            RenderResourceLifetime::Transient,
            "capture",
            "declared-not-ready",
        );
        self.declare_target_with_owner(
            RES_READBACK,
            false,
            RenderResourceLifetime::External,
            "readback",
            "declared-not-ready",
        );
    }

    pub(crate) fn declare_target(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
    ) {
        self.declare_target_with_owner(
            resource,
            ready,
            lifetime,
            resource.name,
            if ready {
                "declared-ready"
            } else {
                "declared-not-ready"
            },
        );
    }

    pub(crate) fn declare_target_with_owner(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
        backing_owner: &'static str,
        ready_cause: &'static str,
    ) {
        let backing_identity = if ready {
            self.actual_backing_identity(resource)
        } else {
            None
        };
        let backing_descriptor = if ready {
            self.actual_backing_descriptor(resource)
        } else {
            None
        };
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            existing.ready = existing.ready || ready;
            existing.backing_owner = backing_owner;
            if ready {
                existing.ready_cause = ready_cause;
                existing.backing_generation = self.resize_generation;
                existing.backing_identity = backing_identity;
                existing.backing_descriptor = backing_descriptor;
            }
            if existing.lifetime == RenderResourceLifetime::Transient
                && lifetime == RenderResourceLifetime::Transient
            {
                self.churn.target_reuses = self.churn.target_reuses.saturating_add(1);
            } else if existing.lifetime != lifetime {
                existing.lifetime = lifetime;
                existing.revision = existing.revision.saturating_add(1);
                self.churn.target_recreates = self.churn.target_recreates.saturating_add(1);
            } else {
                self.churn.alias_reuses = self.churn.alias_reuses.saturating_add(1);
            }
            return;
        }
        self.targets.push(RenderTargetStatus {
            resource,
            ready,
            lifetime,
            revision: self.resize_generation,
            backing_generation: if ready { self.resize_generation } else { 0 },
            backing_identity,
            backing_descriptor,
            backing_owner,
            ready_cause,
        });
        self.churn.target_creates = self.churn.target_creates.saturating_add(1);
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource && target.ready)
    }

    #[cfg(test)]
    pub(crate) fn targets(&self) -> &[RenderTargetStatus] {
        &self.targets
    }

    pub(crate) fn mark_resize_generation(&mut self, generation: u32) {
        if generation == self.resize_generation {
            return;
        }
        self.resize_generation = generation;
        for target in &mut self.targets {
            target.revision = generation;
            target.backing_generation = 0;
            target.backing_identity = None;
            target.backing_descriptor = None;
            target.ready = false;
        }
        self.churn.target_recreates = self
            .churn
            .target_recreates
            .saturating_add(self.targets.len().min(u32::MAX as usize) as u32);
    }
}
