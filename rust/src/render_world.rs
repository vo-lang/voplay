use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::pipeline3d::{ModelDraw, ModelUniform};

pub struct RenderObjectUpdate {
    pub scene_id: u32,
    pub object_id: u32,
    pub model_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub tint: [f32; 4],
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
    tint: [f32; 4],
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
                tint: update.tint,
                visible: update.visible,
                animation_world_id: update.animation_world_id,
                animation_target_id: update.animation_target_id,
            },
        );
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
    }

    pub fn collect_scene_draws(&self, scene_id: u32, out: &mut Vec<ModelDraw>) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for object_id in &scene.order {
            let Some(object) = scene.objects.get(object_id) else {
                continue;
            };
            if !object.visible || object.model_id == 0 {
                continue;
            }
            let model_mat = math3d::model_matrix(object.pos, object.rot, object.scale);
            let normal_mat = math3d::transpose_upper3x3(&model_mat);
            out.push(ModelDraw {
                model_id: object.model_id,
                model_uniform: ModelUniform {
                    model: model_mat,
                    normal_matrix: normal_mat,
                    base_color: [1.0, 1.0, 1.0, 1.0],
                    material_params: [1.0, 1.0, 1.0, 1.0],
                },
                tint: object.tint,
                animation_world_id: object.animation_world_id,
                animation_target_id: object.animation_target_id,
            });
        }
    }
}
