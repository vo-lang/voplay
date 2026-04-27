use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelDraw, ModelUniform};
use crate::primitive_scene::{
    PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate, PrimitiveRenderWorld,
};

pub struct RenderObjectUpdate {
    pub scene_id: u32,
    pub object_id: u32,
    pub model_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub material: MaterialOverride,
    pub visible: bool,
    pub animation_world_id: u32,
    pub animation_target_id: u32,
}

#[derive(Clone, Copy)]
struct RenderObject {
    model_id: u32,
    pos: Vec3,
    rot: Quat,
    scale: Vec3,
    material: MaterialOverride,
    visible: bool,
    animation_world_id: u32,
    animation_target_id: u32,
}

#[derive(Default)]
struct RenderScene {
    objects: HashMap<u32, RenderObject>,
    order: Vec<u32>,
}

#[derive(Default)]
pub struct RenderWorld {
    scenes: HashMap<u32, RenderScene>,
    primitive_scenes: PrimitiveRenderWorld,
}

impl RenderWorld {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_object(&mut self, update: RenderObjectUpdate) {
        let scene = self.scenes.entry(update.scene_id).or_default();
        if !scene.objects.contains_key(&update.object_id) {
            scene.order.push(update.object_id);
        }
        scene.objects.insert(
            update.object_id,
            RenderObject {
                model_id: update.model_id,
                pos: update.pos,
                rot: update.rot,
                scale: update.scale,
                material: update.material,
                visible: update.visible,
                animation_world_id: update.animation_world_id,
                animation_target_id: update.animation_target_id,
            },
        );
    }

    pub fn upsert_primitive_instance(&mut self, update: PrimitiveObjectUpdate) {
        self.primitive_scenes.upsert_instance(update);
    }

    pub fn replace_primitive_chunk(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: Vec<PrimitiveObjectUpdate>,
    ) {
        self.primitive_scenes
            .replace_chunk(scene_id, layer_id, chunk_id, updates);
    }

    pub fn set_primitive_chunk_visible(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        visible: bool,
    ) {
        self.primitive_scenes
            .set_chunk_visible(scene_id, layer_id, chunk_id, visible);
    }

    pub fn destroy_primitive_instance(&mut self, scene_id: u32, layer_id: u32, object_id: u32) {
        self.primitive_scenes
            .destroy_instance(scene_id, layer_id, object_id);
    }

    pub fn clear_primitive_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.primitive_scenes.clear_layer(scene_id, layer_id);
    }

    pub fn destroy_primitive_layer(&mut self, scene_id: u32, layer_id: u32) {
        self.primitive_scenes.destroy_layer(scene_id, layer_id);
    }

    pub fn destroy_object(&mut self, scene_id: u32, object_id: u32) {
        let Some(scene) = self.scenes.get_mut(&scene_id) else {
            return;
        };
        scene.objects.remove(&object_id);
        scene.order.retain(|id| *id != object_id);
    }

    pub fn clear_scene(&mut self, scene_id: u32) {
        let scene = self.scenes.entry(scene_id).or_default();
        scene.objects.clear();
        scene.order.clear();
        self.primitive_scenes.clear_scene(scene_id);
    }

    pub fn collect_scene_draws(&self, scene_id: u32, out: &mut Vec<ModelDraw>) {
        if let Some(scene) = self.scenes.get(&scene_id) {
            for object_id in &scene.order {
                let Some(object) = scene.objects.get(object_id) else {
                    continue;
                };
                if !object.visible || object.model_id == 0 {
                    continue;
                }
                let model_mat = math3d::model_matrix(object.pos, object.rot, object.scale);
                let normal_mat = math3d::normal_matrix(&model_mat);
                out.push(ModelDraw {
                    model_id: object.model_id,
                    model_uniform: ModelUniform {
                        model: model_mat,
                        normal_matrix: normal_mat,
                        base_color: [1.0, 1.0, 1.0, 1.0],
                        material_params: [1.0, 1.0, 1.0, 1.0],
                        emissive_color: [0.0, 0.0, 0.0, 0.0],
                        texture_flags: [0.0, 0.0, 0.0, 0.0],
                    },
                    material: object.material,
                    animation_world_id: object.animation_world_id,
                    animation_target_id: object.animation_target_id,
                });
            }
        }
    }

    pub fn collect_scene_primitive_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.primitive_scenes
            .collect_draws(scene_id, camera, draws, chunks);
    }
}
