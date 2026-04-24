//! Unified 2D draw list with layer-based z-ordering.
//!
//! Collects all 2D draw items (shapes, sprites, text glyphs) tagged with a layer
//! and camera state, then sorts by (layer, submission order) and produces an
//! optimized sequence of draw calls that correctly interleaves shapes and sprites.

use crate::pipeline2d::{CameraUniform, ShapeInstance};
use crate::pipeline_sprite::SpriteInstance;
use crate::texture::TextureId;

// Shape type constants (must match pipeline2d / shape2d.wgsl).
const SHAPE_RECT: f32 = 0.0;
const SHAPE_CIRCLE: f32 = 1.0;
const SHAPE_LINE: f32 = 2.0;

// ─── Draw item ──────────────────────────────────────────────────────────────

/// Tag distinguishing shape vs sprite in the unified list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Shape,
    Sprite,
}

/// A single 2D draw item in the unified list.
struct DrawItem {
    layer: u16,
    order: u32,
    camera_idx: u16,
    kind: ItemKind,
    /// Index into `shapes` or `sprites` vec (depending on `kind`).
    data_idx: u32,
}

/// Sprite data stored alongside its texture id for batching.
pub struct SpriteEntry {
    pub texture_id: TextureId,
    pub instance: SpriteInstance,
}

// ─── Draw call output ───────────────────────────────────────────────────────

/// A single GPU draw call produced by resolving the draw list.
pub struct DrawCall2D {
    /// Index into the cameras vec — used to select dynamic uniform offset.
    pub camera_idx: u16,
    pub kind: DrawCallKind,
}

pub enum DrawCallKind {
    /// Draw shapes[start..start+count] from the sorted shape buffer.
    Shapes { start: u32, count: u32 },
    /// Draw sprites[start..start+count] with the given texture.
    Sprites {
        texture_id: TextureId,
        start: u32,
        count: u32,
    },
}

/// Resolved frame data ready for GPU upload and rendering.
pub struct Frame2D {
    pub cameras: Vec<CameraUniform>,
    pub shapes: Vec<ShapeInstance>,
    pub sprites: Vec<SpriteInstance>,
    pub draw_calls: Vec<DrawCall2D>,
}

// ─── DrawList2D ─────────────────────────────────────────────────────────────

/// Collects all 2D draw commands for one frame, then resolves them into
/// layer-sorted, batched draw calls.
pub struct DrawList2D {
    items: Vec<DrawItem>,
    shapes: Vec<ShapeInstance>,
    sprite_entries: Vec<SpriteEntry>,
    cameras: Vec<CameraUniform>,
    current_layer: u16,
    current_camera_idx: u16,
    next_order: u32,
}

impl DrawList2D {
    pub fn new(screen_w: f32, screen_h: f32) -> Self {
        let cam = CameraUniform::screen_space(screen_w, screen_h);
        Self {
            items: Vec::with_capacity(512),
            shapes: Vec::with_capacity(256),
            sprite_entries: Vec::with_capacity(256),
            cameras: vec![cam],
            current_layer: 0,
            current_camera_idx: 0,
            next_order: 0,
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.shapes.clear();
        self.sprite_entries.clear();
        self.cameras.truncate(1); // keep slot 0 (screen-space)
        self.current_layer = 0;
        self.current_camera_idx = 0;
        self.next_order = 0;
    }

    // ── State commands ──────────────────────────────────────────────────

    pub fn set_layer(&mut self, z: u16) {
        self.current_layer = z;
    }

    pub fn set_screen_space(&mut self, w: f32, h: f32) {
        self.cameras[0] = CameraUniform::screen_space(w, h);
        self.current_camera_idx = 0;
    }

    pub fn set_camera_2d(&mut self, w: f32, h: f32, x: f32, y: f32, zoom: f32, rotation: f32) {
        let cam = CameraUniform::with_camera(w, h, x, y, zoom, rotation);
        self.cameras.push(cam);
        self.current_camera_idx = (self.cameras.len() - 1) as u16;
    }

    pub fn reset_camera(&mut self) {
        self.current_camera_idx = 0;
    }

    // ── Shape commands ──────────────────────────────────────────────────

    pub fn push_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let idx = self.shapes.len() as u32;
        self.shapes.push(ShapeInstance {
            rect: [x, y, w, h],
            color,
            params: [SHAPE_RECT, 0.0, 0.0, 0.0],
        });
        self.push_item(ItemKind::Shape, idx);
    }

    pub fn push_circle(&mut self, cx: f32, cy: f32, radius: f32, color: [f32; 4]) {
        let d = radius * 2.0;
        let idx = self.shapes.len() as u32;
        self.shapes.push(ShapeInstance {
            rect: [cx - radius, cy - radius, d, d],
            color,
            params: [SHAPE_CIRCLE, 0.0, 0.0, 0.0],
        });
        self.push_item(ItemKind::Shape, idx);
    }

    pub fn push_line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: [f32; 4],
    ) {
        let dx = x2 - x1;
        let dy = y2 - y1;
        let length = (dx * dx + dy * dy).sqrt();
        let angle = dy.atan2(dx);
        let cx = (x1 + x2) * 0.5;
        let cy = (y1 + y2) * 0.5;
        let half_l = length * 0.5;
        let half_t = thickness * 0.5;
        let idx = self.shapes.len() as u32;
        self.shapes.push(ShapeInstance {
            rect: [cx - half_l, cy - half_t, length, thickness],
            color,
            params: [SHAPE_LINE, angle, 0.0, 0.0],
        });
        self.push_item(ItemKind::Shape, idx);
    }

    // ── Sprite commands ─────────────────────────────────────────────────

    pub fn push_sprite(&mut self, texture_id: TextureId, instance: SpriteInstance) {
        let idx = self.sprite_entries.len() as u32;
        self.sprite_entries.push(SpriteEntry {
            texture_id,
            instance,
        });
        self.push_item(ItemKind::Sprite, idx);
    }

    // ── Internal ────────────────────────────────────────────────────────

    fn push_item(&mut self, kind: ItemKind, data_idx: u32) {
        let order = self.next_order;
        self.next_order += 1;
        self.items.push(DrawItem {
            layer: self.current_layer,
            order,
            camera_idx: self.current_camera_idx,
            kind,
            data_idx,
        });
    }

    // ── Resolve ─────────────────────────────────────────────────────────

    /// Sort items by (layer, order), build sorted instance arrays, and
    /// produce a minimal sequence of draw calls.
    pub fn resolve(&mut self) -> Frame2D {
        // Stable sort by (layer, order). Since order is monotonically
        // increasing, items within the same layer keep submission order.
        self.items
            .sort_by(|a, b| a.layer.cmp(&b.layer).then(a.order.cmp(&b.order)));

        let mut sorted_shapes: Vec<ShapeInstance> = Vec::with_capacity(self.shapes.len());
        let mut sorted_sprites: Vec<SpriteInstance> = Vec::with_capacity(self.sprite_entries.len());
        let mut draw_calls: Vec<DrawCall2D> = Vec::with_capacity(64);

        // Walk sorted items, accumulate contiguous batches of same
        // (camera, kind, texture) into draw calls.
        let mut i = 0;
        while i < self.items.len() {
            let item = &self.items[i];
            let cam = item.camera_idx;
            let kind = item.kind;

            match kind {
                ItemKind::Shape => {
                    let start = sorted_shapes.len() as u32;
                    // Batch consecutive shapes with same camera
                    while i < self.items.len()
                        && self.items[i].kind == ItemKind::Shape
                        && self.items[i].camera_idx == cam
                    {
                        sorted_shapes.push(self.shapes[self.items[i].data_idx as usize]);
                        i += 1;
                    }
                    let count = sorted_shapes.len() as u32 - start;
                    draw_calls.push(DrawCall2D {
                        camera_idx: cam,
                        kind: DrawCallKind::Shapes { start, count },
                    });
                }
                ItemKind::Sprite => {
                    let tex = self.sprite_entries[item.data_idx as usize].texture_id;
                    let start = sorted_sprites.len() as u32;
                    // Batch consecutive sprites with same camera AND same texture
                    while i < self.items.len()
                        && self.items[i].kind == ItemKind::Sprite
                        && self.items[i].camera_idx == cam
                        && self.sprite_entries[self.items[i].data_idx as usize].texture_id == tex
                    {
                        sorted_sprites
                            .push(self.sprite_entries[self.items[i].data_idx as usize].instance);
                        i += 1;
                    }
                    let count = sorted_sprites.len() as u32 - start;
                    draw_calls.push(DrawCall2D {
                        camera_idx: cam,
                        kind: DrawCallKind::Sprites {
                            texture_id: tex,
                            start,
                            count,
                        },
                    });
                }
            }
        }

        Frame2D {
            cameras: self.cameras.clone(),
            shapes: sorted_shapes,
            sprites: sorted_sprites,
            draw_calls,
        }
    }
}
