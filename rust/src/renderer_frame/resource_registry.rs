use super::{
    RenderResource, RenderResourceChurn, RenderResourceKind, RenderResourceLifetime,
    RenderTargetStatus,
};
use crate::renderer_frame_resources::*;
use crate::renderer_targets::{
    create_depth_view, create_msaa_color_view, create_msaa_receiver_mask_view,
    create_msaa_surface_props_view, create_post_color_view, create_receiver_mask_view,
    create_surface_props_view, MAIN_SAMPLE_COUNT,
};

#[derive(Default)]
struct RenderTargetStore {
    depth_view: Option<wgpu::TextureView>,
    msaa_color_view: Option<wgpu::TextureView>,
    post_color_view: Option<wgpu::TextureView>,
    msaa_receiver_mask_view: Option<wgpu::TextureView>,
    receiver_mask_view: Option<wgpu::TextureView>,
    msaa_surface_props_view: Option<wgpu::TextureView>,
    surface_props_view: Option<wgpu::TextureView>,
}

pub(crate) struct RenderResourceRegistry {
    targets: Vec<RenderTargetStatus>,
    pub(crate) churn: RenderResourceChurn,
    pub(crate) resize_generation: u32,
    store: RenderTargetStore,
}

impl Default for RenderResourceRegistry {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            churn: RenderResourceChurn::default(),
            resize_generation: 0,
            store: RenderTargetStore::default(),
        }
    }
}

impl RenderResourceRegistry {
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
        self.store = RenderTargetStore {
            depth_view: Some(create_depth_view(device, width, height, MAIN_SAMPLE_COUNT)),
            msaa_color_view: create_msaa_color_view(
                device,
                width,
                height,
                surface_format,
                MAIN_SAMPLE_COUNT,
            ),
            post_color_view: Some(create_post_color_view(
                device,
                width,
                height,
                surface_format,
            )),
            msaa_receiver_mask_view: create_msaa_receiver_mask_view(
                device,
                width,
                height,
                MAIN_SAMPLE_COUNT,
            ),
            receiver_mask_view: Some(create_receiver_mask_view(
                device,
                width,
                height,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                "voplay_receiver_mask",
            )),
            msaa_surface_props_view: create_msaa_surface_props_view(
                device,
                width,
                height,
                MAIN_SAMPLE_COUNT,
            ),
            surface_props_view: Some(create_surface_props_view(
                device,
                width,
                height,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                "voplay_surface_props",
            )),
        };
        let next_generation = self.resize_generation.saturating_add(1);
        self.mark_resize_generation(next_generation);
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

    pub(crate) fn main_color_ready(&self) -> bool {
        self.actual_texture_view(RES_MAIN_COLOR).is_some()
    }

    pub(crate) fn actual_texture_view(
        &self,
        resource: RenderResource,
    ) -> Option<&wgpu::TextureView> {
        match resource.kind {
            RenderResourceKind::MainColor => self
                .store
                .msaa_color_view
                .as_ref()
                .or(self.store.post_color_view.as_ref()),
            RenderResourceKind::Depth => self.store.depth_view.as_ref(),
            RenderResourceKind::ReceiverMask => self
                .store
                .msaa_receiver_mask_view
                .as_ref()
                .or(self.store.receiver_mask_view.as_ref()),
            RenderResourceKind::SurfaceProps => self
                .store
                .msaa_surface_props_view
                .as_ref()
                .or(self.store.surface_props_view.as_ref()),
            RenderResourceKind::PostColor => self.store.post_color_view.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn validate_backing_generation(&self, resource: RenderResource) -> bool {
        let Some(target) = self
            .targets
            .iter()
            .find(|target| target.resource == resource)
        else {
            return false;
        };
        target.ready
            && target.backing_generation == self.resize_generation
            && self.actual_texture_view(resource).is_some()
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
            backing_owner,
            ready_cause,
        });
        self.churn.target_creates = self.churn.target_creates.saturating_add(1);
    }

    pub(crate) fn mark_ready_with_cause(
        &mut self,
        resource: RenderResource,
        ready_cause: &'static str,
    ) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            target.ready = true;
            target.ready_cause = ready_cause;
            target.backing_generation = self.resize_generation;
            return;
        }
        self.declare_target_with_owner(
            resource,
            true,
            RenderResourceLifetime::Persistent,
            resource.name,
            ready_cause,
        );
    }

    pub(crate) fn is_ready(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource && target.ready)
    }

    pub(crate) fn is_declared(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource)
    }

    pub(crate) fn targets(&self) -> &[RenderTargetStatus] {
        &self.targets
    }

    pub(crate) fn count_lifetime(&self, lifetime: RenderResourceLifetime) -> u32 {
        self.targets
            .iter()
            .filter(|target| target.lifetime == lifetime)
            .count()
            .min(u32::MAX as usize) as u32
    }

    pub(crate) fn mark_resize_generation(&mut self, generation: u32) {
        if generation == self.resize_generation {
            return;
        }
        self.resize_generation = generation;
        for target in &mut self.targets {
            target.revision = generation;
            target.backing_generation = 0;
            target.ready = false;
        }
        self.churn.target_recreates = self
            .churn
            .target_recreates
            .saturating_add(self.targets.len().min(u32::MAX as usize) as u32);
    }
}
