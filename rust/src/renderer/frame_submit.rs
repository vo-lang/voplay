use super::*;

impl Renderer {
    pub(super) fn submit_frame_inner(&mut self, data: &[u8]) -> Result<(), String> {
        self.run_frame_orchestrator(data)
    }
}
