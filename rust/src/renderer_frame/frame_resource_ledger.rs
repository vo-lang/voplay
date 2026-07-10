use super::{RenderResource, RenderResourceChurn, RenderResourceLifetime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameResourceStatus {
    resource: RenderResource,
    ready: bool,
    lifetime: RenderResourceLifetime,
}

#[derive(Default)]
pub(super) struct FrameResourceLedger {
    targets: Vec<FrameResourceStatus>,
    pub(super) churn: RenderResourceChurn,
    pub(super) resize_generation: u32,
}

impl FrameResourceLedger {
    pub(super) fn declare_target(
        &mut self,
        resource: RenderResource,
        ready: bool,
        lifetime: RenderResourceLifetime,
    ) {
        if let Some(existing) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            existing.ready = existing.ready || ready;
            if existing.lifetime == RenderResourceLifetime::Transient
                && lifetime == RenderResourceLifetime::Transient
            {
                self.churn.target_reuses = self.churn.target_reuses.saturating_add(1);
            } else if existing.lifetime != lifetime {
                existing.lifetime = lifetime;
                self.churn.target_recreates = self.churn.target_recreates.saturating_add(1);
            } else {
                self.churn.alias_reuses = self.churn.alias_reuses.saturating_add(1);
            }
            return;
        }
        self.targets.push(FrameResourceStatus {
            resource,
            ready,
            lifetime,
        });
        self.churn.target_creates = self.churn.target_creates.saturating_add(1);
    }

    pub(super) fn mark_ready(&mut self, resource: RenderResource) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.resource == resource)
        {
            target.ready = true;
            return;
        }
        self.declare_target(resource, true, RenderResourceLifetime::Persistent);
    }

    pub(super) fn is_ready(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource && target.ready)
    }

    pub(super) fn is_declared(&self, resource: RenderResource) -> bool {
        self.targets
            .iter()
            .any(|target| target.resource == resource)
    }

    pub(super) fn declared_resources(&self) -> impl Iterator<Item = RenderResource> + '_ {
        self.targets.iter().map(|target| target.resource)
    }

    pub(super) fn count_lifetime(&self, lifetime: RenderResourceLifetime) -> u32 {
        self.targets
            .iter()
            .filter(|target| target.lifetime == lifetime)
            .count()
            .min(u32::MAX as usize) as u32
    }

    #[cfg(test)]
    pub(super) fn mark_resize_generation(&mut self, generation: u32) {
        if generation == self.resize_generation {
            return;
        }
        self.resize_generation = generation;
        for target in &mut self.targets {
            target.ready = false;
        }
        self.churn.target_recreates = self
            .churn
            .target_recreates
            .saturating_add(self.targets.len().min(u32::MAX as usize) as u32);
    }
}
