use super::*;
use crate::draw_list::Frame2D;

pub(super) struct Frame2DUploadContext<'a> {
    pub(super) frame: &'a Frame2D,
    pub(super) debug_frame_count: u64,
    pub(super) data_len: usize,
    pub(super) command_count: u32,
    pub(super) camera3d_enabled: bool,
    pub(super) model_command_count: u32,
    pub(super) scene_upsert_count: u32,
    pub(super) scene_draw_count: u32,
    pub(super) planned_model_draw_count: usize,
    pub(super) planned_primitive_draw_count: usize,
    pub(super) planned_primitive_chunk_count: usize,
    pub(super) skybox_count: u32,
    pub(super) planned_projected_decal_count: usize,
    pub(super) diagnostic_flags: u32,
    pub(super) rect_count: u32,
    pub(super) circle_count: u32,
    pub(super) line_count: u32,
    pub(super) text_count: u32,
    pub(super) sprite_count: u32,
    pub(super) clear_color: wgpu::Color,
}

impl Renderer {
    pub(super) fn upload_frame_2d_instances(&mut self, context: Frame2DUploadContext<'_>) -> u32 {
        let frame = context.frame;
        Self::debug_submit_status(
            context.debug_frame_count,
            &format!(
                "voplay submit #{} bytes={} cmds={} cam3d={} modelCmds={} sceneUpserts={} sceneDraws={} models={} primitives={} primitiveChunks={} skybox={} projectedDecals={} diagFlags=0x{:x} 2d(rect/circ/line/text/sprite)={}/{}/{}/{}/{} resolved(shapes/sprites/calls/cams)={}/{}/{}/{} clear={:.2},{:.2},{:.2}",
                context.debug_frame_count,
                context.data_len,
                context.command_count,
                context.camera3d_enabled,
                context.model_command_count,
                context.scene_upsert_count,
                context.scene_draw_count,
                context.planned_model_draw_count,
                context.planned_primitive_draw_count,
                context.planned_primitive_chunk_count,
                context.skybox_count,
                context.planned_projected_decal_count,
                context.diagnostic_flags,
                context.rect_count,
                context.circle_count,
                context.line_count,
                context.text_count,
                context.sprite_count,
                frame.shapes.len(),
                frame.sprites.len(),
                frame.draw_calls.len(),
                frame.cameras.len(),
                context.clear_color.r,
                context.clear_color.g,
                context.clear_color.b,
            ),
        );

        let align = self.camera_alignment;
        let cam_count = frame.cameras.len();
        if cam_count > self.camera_slot_capacity {
            let new_cap = cam_count.next_power_of_two();
            let (buf, bg) =
                Self::create_camera_buffer_and_bg(&self.device, &self.camera_bgl, new_cap, align);
            self.camera_buffer = buf;
            self.camera_bind_group = bg;
            self.camera_slot_capacity = new_cap;
        }
        for (i, cam) in frame.cameras.iter().enumerate() {
            let offset = i as u64 * align as u64;
            self.queue
                .write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(cam));
        }

        self.pipeline2d
            .upload_instances(&self.device, &self.queue, &frame.shapes);
        self.pipeline_sprite
            .upload_instances(&self.device, &self.queue, &frame.sprites);
        align
    }
}
