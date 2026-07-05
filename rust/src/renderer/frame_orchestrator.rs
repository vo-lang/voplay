use super::*;

pub(super) struct FrameSubmitOrchestrator;

impl FrameSubmitOrchestrator {
    pub(super) fn run(renderer: &mut Renderer, data: &[u8]) -> Result<(), String> {
        renderer.run_frame_orchestrator(data)
    }
}
