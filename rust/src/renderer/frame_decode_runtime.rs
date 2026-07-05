use super::frame_decode::FrameDecodeOutput;
use super::frame_transaction_builder::FrameTransactionBuilder;
use super::*;

impl Renderer {
    pub(super) fn decode_frame_commands(
        &mut self,
        data: &[u8],
        screen_w: f32,
        screen_h: f32,
        aspect: f32,
        debug_frame_count: u64,
        perf_enabled: bool,
    ) -> FrameDecodeOutput {
        FrameTransactionBuilder::new(
            data,
            screen_w,
            screen_h,
            aspect,
            debug_frame_count,
            perf_enabled,
            &self.texture_manager,
            &mut self.font_manager,
        )
        .decode()
    }
}
