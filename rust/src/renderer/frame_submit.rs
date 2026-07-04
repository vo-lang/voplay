use super::frame_orchestrator::FrameSubmitOrchestrator;
use super::*;

impl Renderer {
    pub(super) fn submit_frame_inner(&mut self, data: &[u8]) -> Result<(), String> {
        FrameSubmitOrchestrator::run(self, data)
    }
}
