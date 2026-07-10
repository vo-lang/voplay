use super::*;
use crate::stream::{
    Primitive3DInstanceCommand, Primitive3DInstanceKeyCommand, Primitive3DInstanceRefCommand,
    Primitive3DMaterialCommand, Primitive3DShapeCommand,
};

pub(super) struct FrameTransaction {
    screen_w: f32,
    screen_h: f32,
    overlay_ops: Vec<FrameOverlayOp>,
    owner_mutations: Vec<FrameOwnerMutation>,
}

pub(super) struct FrameTransactionApplyContext<'a> {
    pub(super) draw_list: &'a mut DrawList2D,
    pub(super) font_manager: &'a mut FontManager,
    pub(super) render_world: &'a mut RenderWorld,
    pub(super) primitive_pipeline: &'a mut PrimitivePipeline,
    pub(super) primitive_shapes: &'a mut HashMap<(u32, u32, u32), u32>,
    pub(super) primitive_materials:
        &'a mut HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) model_manager: &'a ModelManager,
    pub(super) texture_manager: &'a TextureManager,
}

pub(super) struct FrameTransactionApplyReport {
    pub(super) resident_chunk_rebuild_count: u32,
}

impl Renderer {
    pub(super) fn apply_frame_transaction(
        &mut self,
        transaction: FrameTransaction,
    ) -> FrameTransactionApplyReport {
        transaction.apply(FrameTransactionApplyContext {
            draw_list: &mut self.draw_list,
            font_manager: &mut self.font_manager,
            render_world: &mut self.render_world,
            primitive_pipeline: &mut self.primitive_pipeline,
            primitive_shapes: &mut self.primitive_shapes,
            primitive_materials: &mut self.primitive_materials,
            device: &self.device,
            queue: &self.queue,
            model_manager: &self.model_manager,
            texture_manager: &self.texture_manager,
        })
    }
}

enum FrameOverlayOp {
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

enum FrameOwnerMutation {
    UpsertObject(RenderObjectUpdate),
    DestroyObject {
        scene_id: u32,
        object_id: u32,
    },
    ClearScene {
        scene_id: u32,
    },
    PrimitiveUpsertInstance(PrimitiveObjectUpdate),
    PrimitiveDestroyInstance {
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
    },
    PrimitiveClearLayer {
        scene_id: u32,
        layer_id: u32,
    },
    PrimitiveDestroyLayer {
        scene_id: u32,
        layer_id: u32,
    },
    PrimitiveReplaceChunk {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: Vec<PrimitiveObjectUpdate>,
    },
    PrimitiveReplaceChunkRefs {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceRefCommand>,
    },
    PrimitiveReplaceChunkKeys {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceKeyCommand>,
    },
    PrimitiveUpsertMaterials {
        scene_id: u32,
        layer_id: u32,
        materials: Vec<Primitive3DMaterialCommand>,
    },
    PrimitiveUpsertShapes {
        scene_id: u32,
        layer_id: u32,
        shapes: Vec<Primitive3DShapeCommand>,
    },
    PrimitiveSetChunkVisible {
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        visible: bool,
    },
}

impl FrameTransaction {
    pub(super) fn new(screen_w: f32, screen_h: f32) -> Self {
        Self {
            screen_w,
            screen_h,
            overlay_ops: Vec::new(),
            owner_mutations: Vec::new(),
        }
    }

    pub(super) fn set_camera_2d(&mut self, x: f32, y: f32, zoom: f32, rotation: f32) {
        self.overlay_ops.push(FrameOverlayOp::SetCamera2D {
            x,
            y,
            zoom,
            rotation,
        });
    }

    pub(super) fn reset_camera(&mut self) {
        self.overlay_ops.push(FrameOverlayOp::ResetCamera);
    }

    pub(super) fn set_layer(&mut self, z: u16) {
        self.overlay_ops.push(FrameOverlayOp::SetLayer { z });
    }

    pub(super) fn push_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.overlay_ops
            .push(FrameOverlayOp::PushRect { x, y, w, h, color });
    }

    pub(super) fn push_circle(&mut self, cx: f32, cy: f32, radius: f32, color: [f32; 4]) {
        self.overlay_ops.push(FrameOverlayOp::PushCircle {
            cx,
            cy,
            radius,
            color,
        });
    }

    pub(super) fn push_line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: [f32; 4],
    ) {
        self.overlay_ops.push(FrameOverlayOp::PushLine {
            x1,
            y1,
            x2,
            y2,
            thickness,
            color,
        });
    }

    pub(super) fn push_sprite(&mut self, texture_id: TextureId, instance: SpriteInstance) {
        self.overlay_ops.push(FrameOverlayOp::PushSprite {
            texture_id,
            instance,
        });
    }

    pub(super) fn draw_text(
        &mut self,
        font_id: u32,
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
    ) {
        self.overlay_ops.push(FrameOverlayOp::DrawText {
            font_id,
            text,
            x,
            y,
            size,
            color,
        });
    }

    pub(super) fn upsert_object(&mut self, update: RenderObjectUpdate) {
        self.owner_mutations
            .push(FrameOwnerMutation::UpsertObject(update));
    }

    pub(super) fn destroy_object(&mut self, scene_id: u32, object_id: u32) {
        self.owner_mutations
            .push(FrameOwnerMutation::DestroyObject {
                scene_id,
                object_id,
            });
    }

    pub(super) fn clear_scene(&mut self, scene_id: u32) {
        self.owner_mutations
            .push(FrameOwnerMutation::ClearScene { scene_id });
    }

    pub(super) fn upsert_primitive_instance(&mut self, update: PrimitiveObjectUpdate) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveUpsertInstance(update));
    }

    pub(super) fn destroy_primitive_instance(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveDestroyInstance {
                scene_id,
                layer_id,
                object_id,
            });
    }

    pub(super) fn clear_primitive_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveClearLayer { scene_id, layer_id });
    }

    pub(super) fn destroy_primitive_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveDestroyLayer { scene_id, layer_id });
    }

    pub(super) fn replace_primitive_chunk(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceCommand>,
    ) {
        let updates = instances
            .into_iter()
            .map(|instance| PrimitiveObjectUpdate {
                scene_id,
                layer_id,
                object_id: instance.object_id,
                model_id: instance.model_id,
                pos: instance.pos,
                rot: instance.rot,
                scale: instance.scale,
                material: instance.material,
                visible: instance.visible,
                flags: instance.flags,
                lod_near: instance.lod_near,
                lod_far: instance.lod_far,
                wind_strength: instance.wind_strength,
                atlas_uv: instance.atlas_uv,
            })
            .collect();
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveReplaceChunk {
                scene_id,
                layer_id,
                chunk_id,
                updates,
            });
    }

    pub(super) fn replace_primitive_chunk_refs(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceRefCommand>,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveReplaceChunkRefs {
                scene_id,
                layer_id,
                chunk_id,
                instances,
            });
    }

    pub(super) fn replace_primitive_chunk_keys(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        instances: Vec<Primitive3DInstanceKeyCommand>,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveReplaceChunkKeys {
                scene_id,
                layer_id,
                chunk_id,
                instances,
            });
    }

    pub(super) fn upsert_primitive_materials(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        materials: Vec<Primitive3DMaterialCommand>,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveUpsertMaterials {
                scene_id,
                layer_id,
                materials,
            });
    }

    pub(super) fn upsert_primitive_shapes(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        shapes: Vec<Primitive3DShapeCommand>,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveUpsertShapes {
                scene_id,
                layer_id,
                shapes,
            });
    }

    pub(super) fn set_primitive_chunk_visible(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        visible: bool,
    ) {
        self.owner_mutations
            .push(FrameOwnerMutation::PrimitiveSetChunkVisible {
                scene_id,
                layer_id,
                chunk_id,
                visible,
            });
    }

    pub(super) fn apply(
        self,
        mut ctx: FrameTransactionApplyContext<'_>,
    ) -> FrameTransactionApplyReport {
        ctx.draw_list.clear();
        ctx.draw_list.set_screen_space(self.screen_w, self.screen_h);
        for op in self.overlay_ops {
            apply_overlay_op(
                ctx.draw_list,
                ctx.font_manager,
                self.screen_w,
                self.screen_h,
                op,
            );
        }

        let mut resident_chunk_rebuild_count = 0u32;
        for mutation in self.owner_mutations {
            resident_chunk_rebuild_count = resident_chunk_rebuild_count
                .saturating_add(apply_owner_mutation(&mut ctx, mutation));
        }
        resident_chunk_rebuild_count = resident_chunk_rebuild_count.saturating_add(
            ctx.primitive_pipeline.flush_resident_rebuild_queue(
                ctx.device,
                ctx.queue,
                ctx.model_manager,
            ),
        );
        FrameTransactionApplyReport {
            resident_chunk_rebuild_count,
        }
    }
}

fn apply_overlay_op(
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
            for draw in
                font_manager.layout_text(&text, x, y, size, color[0], color[1], color[2], color[3])
            {
                draw_list.push_sprite(draw.texture_id, draw.instance);
            }
        }
    }
}

fn apply_owner_mutation(
    ctx: &mut FrameTransactionApplyContext<'_>,
    mutation: FrameOwnerMutation,
) -> u32 {
    match mutation {
        FrameOwnerMutation::UpsertObject(update) => {
            ctx.render_world.upsert_object(update);
            0
        }
        FrameOwnerMutation::DestroyObject {
            scene_id,
            object_id,
        } => {
            ctx.render_world.destroy_object(scene_id, object_id);
            0
        }
        FrameOwnerMutation::ClearScene { scene_id } => {
            ctx.render_world.clear_scene(scene_id);
            ctx.primitive_pipeline.clear_scene(scene_id);
            ctx.primitive_shapes
                .retain(|(shape_scene, _, _), _| *shape_scene != scene_id);
            ctx.primitive_materials
                .retain(|(material_scene, _, _), _| *material_scene != scene_id);
            0
        }
        FrameOwnerMutation::PrimitiveUpsertInstance(update) => {
            ctx.primitive_pipeline.upsert_instance(
                ctx.device,
                ctx.queue,
                update,
                ctx.model_manager,
                ctx.texture_manager,
            );
            ctx.render_world.upsert_primitive_instance(update);
            0
        }
        FrameOwnerMutation::PrimitiveDestroyInstance {
            scene_id,
            layer_id,
            object_id,
        } => {
            ctx.primitive_pipeline.destroy_instance(
                ctx.device,
                ctx.queue,
                scene_id,
                layer_id,
                object_id,
                ctx.model_manager,
                ctx.texture_manager,
            );
            ctx.render_world
                .destroy_primitive_instance(scene_id, layer_id, object_id);
            0
        }
        FrameOwnerMutation::PrimitiveClearLayer { scene_id, layer_id } => {
            ctx.primitive_pipeline.clear_layer(scene_id, layer_id);
            ctx.render_world.clear_primitive_layer(scene_id, layer_id);
            retain_primitive_tables_for_layer(
                ctx.primitive_shapes,
                ctx.primitive_materials,
                scene_id,
                layer_id,
            );
            0
        }
        FrameOwnerMutation::PrimitiveDestroyLayer { scene_id, layer_id } => {
            ctx.primitive_pipeline.clear_layer(scene_id, layer_id);
            ctx.render_world.destroy_primitive_layer(scene_id, layer_id);
            retain_primitive_tables_for_layer(
                ctx.primitive_shapes,
                ctx.primitive_materials,
                scene_id,
                layer_id,
            );
            0
        }
        FrameOwnerMutation::PrimitiveReplaceChunk {
            scene_id,
            layer_id,
            chunk_id,
            updates,
        } => {
            replace_primitive_chunk(ctx, scene_id, layer_id, chunk_id, updates);
            1
        }
        FrameOwnerMutation::PrimitiveReplaceChunkRefs {
            scene_id,
            layer_id,
            chunk_id,
            instances,
        } => {
            let updates =
                primitive_updates_from_refs(scene_id, layer_id, instances, ctx.primitive_materials);
            replace_primitive_chunk(ctx, scene_id, layer_id, chunk_id, updates);
            1
        }
        FrameOwnerMutation::PrimitiveReplaceChunkKeys {
            scene_id,
            layer_id,
            chunk_id,
            instances,
        } => {
            let updates = primitive_updates_from_keys(
                scene_id,
                layer_id,
                instances,
                ctx.primitive_shapes,
                ctx.primitive_materials,
            );
            replace_primitive_chunk(ctx, scene_id, layer_id, chunk_id, updates);
            1
        }
        FrameOwnerMutation::PrimitiveUpsertMaterials {
            scene_id,
            layer_id,
            materials,
        } => {
            for material in materials {
                ctx.primitive_materials.insert(
                    (scene_id, layer_id, material.material_id),
                    material.material,
                );
            }
            0
        }
        FrameOwnerMutation::PrimitiveUpsertShapes {
            scene_id,
            layer_id,
            shapes,
        } => {
            for shape in shapes {
                ctx.primitive_shapes
                    .insert((scene_id, layer_id, shape.shape_id), shape.model_id);
            }
            0
        }
        FrameOwnerMutation::PrimitiveSetChunkVisible {
            scene_id,
            layer_id,
            chunk_id,
            visible,
        } => {
            ctx.render_world
                .set_primitive_chunk_visible(scene_id, layer_id, chunk_id, visible);
            1
        }
    }
}

fn retain_primitive_tables_for_layer(
    shapes: &mut HashMap<(u32, u32, u32), u32>,
    materials: &mut HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
    scene_id: u32,
    layer_id: u32,
) {
    shapes.retain(|(shape_scene, shape_layer, _), _| {
        *shape_scene != scene_id || *shape_layer != layer_id
    });
    materials.retain(|(material_scene, material_layer, _), _| {
        *material_scene != scene_id || *material_layer != layer_id
    });
}

fn replace_primitive_chunk(
    ctx: &mut FrameTransactionApplyContext<'_>,
    scene_id: u32,
    layer_id: u32,
    chunk_id: u32,
    updates: Vec<PrimitiveObjectUpdate>,
) {
    ctx.primitive_pipeline.replace_chunk(
        ctx.device,
        ctx.queue,
        scene_id,
        layer_id,
        chunk_id,
        &updates,
        ctx.model_manager,
        ctx.texture_manager,
    );
    ctx.render_world
        .replace_primitive_chunk(scene_id, layer_id, chunk_id, updates);
}

fn primitive_updates_from_refs(
    scene_id: u32,
    layer_id: u32,
    instances: Vec<Primitive3DInstanceRefCommand>,
    primitive_materials: &HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
) -> Vec<PrimitiveObjectUpdate> {
    instances
        .into_iter()
        .map(|instance| {
            let material = primitive_materials
                .get(&(scene_id, layer_id, instance.material_id))
                .copied()
                .unwrap_or_default();
            PrimitiveObjectUpdate {
                scene_id,
                layer_id,
                object_id: instance.object_id,
                model_id: instance.model_id,
                pos: instance.pos,
                rot: instance.rot,
                scale: instance.scale,
                material,
                visible: instance.visible,
                flags: instance.flags,
                lod_near: instance.lod_near,
                lod_far: instance.lod_far,
                wind_strength: instance.wind_strength,
                atlas_uv: instance.atlas_uv,
            }
        })
        .collect()
}

fn primitive_updates_from_keys(
    scene_id: u32,
    layer_id: u32,
    instances: Vec<Primitive3DInstanceKeyCommand>,
    primitive_shapes: &HashMap<(u32, u32, u32), u32>,
    primitive_materials: &HashMap<(u32, u32, u32), crate::pipeline3d::MaterialOverride>,
) -> Vec<PrimitiveObjectUpdate> {
    instances
        .into_iter()
        .map(|instance| {
            let model_id = primitive_shapes
                .get(&(scene_id, layer_id, instance.shape_id))
                .copied()
                .unwrap_or_default();
            let mut material = primitive_materials
                .get(&(scene_id, layer_id, instance.material_id))
                .copied()
                .unwrap_or_default();
            if instance.tint != [0.0, 0.0, 0.0, 0.0] {
                material.base_color[0] *= instance.tint[0];
                material.base_color[1] *= instance.tint[1];
                material.base_color[2] *= instance.tint[2];
                material.base_color[3] *= instance.tint[3];
            }
            PrimitiveObjectUpdate {
                scene_id,
                layer_id,
                object_id: instance.object_id,
                model_id,
                pos: instance.pos,
                rot: instance.rot,
                scale: instance.scale,
                material,
                visible: instance.visible,
                flags: instance.flags,
                lod_near: instance.lod_near,
                lod_far: instance.lod_far,
                wind_strength: instance.wind_strength,
                atlas_uv: instance.atlas_uv,
            }
        })
        .collect()
}
