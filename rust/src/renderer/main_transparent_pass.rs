use super::*;

pub(super) struct MainTransparentPassExecutor;

impl MainTransparentPassExecutor {
    pub(super) fn execute() -> Result<f64, String> {
        Ok(0.0)
    }

    pub(super) fn workload() -> RenderPassWorkload {
        RenderPassWorkload::default()
    }
}
