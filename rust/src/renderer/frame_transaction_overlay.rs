use super::*;

pub(super) enum FrameOverlayOp {
    SetCamera2D {
        x: f32,
        y: f32,
        zoom: f32,
        rotation: f32,
    },
    ResetCamera,
    SetLayer {
        z: u16,
    },
    PushRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    PushCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: [f32; 4],
    },
    PushLine {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: [f32; 4],
    },
    PushSprite {
        texture_id: TextureId,
        instance: SpriteInstance,
    },
    DrawText {
        font_id: u32,
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
    },
}

pub(super) fn apply_overlay_op(
    draw_list: &mut DrawList2D,
    font_manager: &mut FontManager,
    screen_w: f32,
    screen_h: f32,
    op: FrameOverlayOp,
) {
    match op {
        FrameOverlayOp::SetCamera2D {
            x,
            y,
            zoom,
            rotation,
        } => draw_list.set_camera_2d(screen_w, screen_h, x, y, zoom, rotation),
        FrameOverlayOp::ResetCamera => draw_list.reset_camera(),
        FrameOverlayOp::SetLayer { z } => draw_list.set_layer(z),
        FrameOverlayOp::PushRect { x, y, w, h, color } => draw_list.push_rect(x, y, w, h, color),
        FrameOverlayOp::PushCircle {
            cx,
            cy,
            radius,
            color,
        } => draw_list.push_circle(cx, cy, radius, color),
        FrameOverlayOp::PushLine {
            x1,
            y1,
            x2,
            y2,
            thickness,
            color,
        } => draw_list.push_line(x1, y1, x2, y2, thickness, color),
        FrameOverlayOp::PushSprite {
            texture_id,
            instance,
        } => draw_list.push_sprite(texture_id, instance),
        FrameOverlayOp::DrawText {
            font_id,
            text,
            x,
            y,
            size,
            color,
        } => {
            font_manager.set_current(font_id);
            for draw in font_manager.layout_text(&text, x, y, size, color) {
                draw_list.push_sprite(draw.texture_id, draw.instance);
            }
        }
    }
}
