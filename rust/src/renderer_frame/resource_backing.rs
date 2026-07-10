use super::{RenderResource, RenderResourceLifetime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RenderBackingSlot {
    Depth,
    MsaaColor,
    PostColor,
    MsaaReceiverMask,
    ReceiverMask,
    MsaaSurfaceProps,
    SurfaceProps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RenderBackingIdentity {
    pub(crate) serial: u64,
    pub(crate) generation: u32,
    pub(crate) slot: RenderBackingSlot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RenderBackingDescriptor {
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) sample_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderTargetStatus {
    pub(crate) resource: RenderResource,
    pub(crate) ready: bool,
    pub(crate) lifetime: RenderResourceLifetime,
    pub(crate) revision: u32,
    pub(crate) backing_generation: u32,
    pub(crate) backing_identity: Option<RenderBackingIdentity>,
    pub(crate) backing_descriptor: Option<RenderBackingDescriptor>,
    pub(crate) backing_owner: &'static str,
    pub(crate) ready_cause: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderResourceBackingError {
    pub(crate) resource: RenderResource,
    pub(crate) code: &'static str,
    pub(crate) expected_generation: u32,
    pub(crate) actual_generation: u32,
    pub(crate) detail: &'static str,
}

impl RenderResourceBackingError {
    pub(crate) fn structured_message(&self) -> String {
        format!(
            "voplay.render.failure stage=frame_graph_resource code={} resource={} expected_generation={} actual_generation={} detail={}",
            self.code,
            self.resource.name,
            self.expected_generation,
            self.actual_generation,
            self.detail
        )
    }
}

impl RenderTargetStatus {
    pub(super) fn validate_backing(
        &self,
        generation: u32,
        identity: Option<RenderBackingIdentity>,
        descriptor: Option<RenderBackingDescriptor>,
        view_exists: bool,
    ) -> Result<(), RenderResourceBackingError> {
        let failure = |code, detail| RenderResourceBackingError {
            resource: self.resource,
            code,
            expected_generation: generation,
            actual_generation: self.backing_generation,
            detail,
        };
        if !self.ready {
            return Err(failure("target_not_ready", "registry_target_is_not_ready"));
        }
        if self.backing_generation != generation {
            return Err(failure(
                "generation_mismatch",
                "registry_generation_differs_from_live_generation",
            ));
        }
        if self.backing_identity.is_none() || identity.is_none() {
            return Err(failure(
                "identity_missing",
                "physical_backing_identity_is_missing",
            ));
        }
        if self.backing_identity != identity {
            return Err(failure(
                "identity_mismatch",
                "registry_identity_differs_from_live_backing",
            ));
        }
        if self.backing_descriptor.is_none() || descriptor.is_none() {
            return Err(failure(
                "descriptor_missing",
                "physical_backing_descriptor_is_missing",
            ));
        }
        if self.backing_descriptor != descriptor {
            return Err(failure(
                "descriptor_mismatch",
                "registry_descriptor_differs_from_live_backing",
            ));
        }
        if !view_exists {
            return Err(failure("view_missing", "physical_texture_view_is_missing"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn matches_backing(
        &self,
        generation: u32,
        identity: Option<RenderBackingIdentity>,
        descriptor: Option<RenderBackingDescriptor>,
        view_exists: bool,
    ) -> bool {
        self.validate_backing(generation, identity, descriptor, view_exists)
            .is_ok()
    }
}
