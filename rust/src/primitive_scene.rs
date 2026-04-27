use std::collections::{HashMap, HashSet};

use crate::math3d::{self, Quat, Vec3};
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelUniform};

#[derive(Clone, Copy)]
pub struct PrimitiveDraw {
    pub model_id: u32,
    pub model_uniform: ModelUniform,
    pub material: MaterialOverride,
}

impl PrimitiveDraw {
    pub fn from_update(update: PrimitiveObjectUpdate) -> Self {
        Self {
            model_id: update.model_id,
            model_uniform: primitive_model_uniform(update.pos, update.rot, update.scale),
            material: update.material,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PrimitiveChunkRef {
    pub scene_id: u32,
    pub layer_id: u32,
    pub chunk_id: u32,
}

#[derive(Clone, Copy)]
pub struct PrimitiveObjectUpdate {
    pub scene_id: u32,
    pub layer_id: u32,
    pub object_id: u32,
    pub model_id: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub material: MaterialOverride,
    pub visible: bool,
}

#[derive(Clone, Copy)]
struct PrimitiveObject {
    model_id: u32,
    model_uniform: ModelUniform,
    material: MaterialOverride,
    pos: Vec3,
    scale: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct PrimitiveBounds {
    min: Vec3,
    max: Vec3,
}

#[derive(Default)]
struct PrimitiveLayer {
    objects: HashMap<u32, PrimitiveObject>,
    order: Vec<u32>,
    visible: HashMap<u32, bool>,
    visible_order: Vec<u32>,
    chunks: HashMap<u32, Vec<u32>>,
    chunk_order: Vec<u32>,
    chunk_visible: HashMap<u32, bool>,
    chunk_bounds: HashMap<u32, PrimitiveBounds>,
    object_chunks: HashMap<u32, u32>,
}

#[derive(Default)]
struct PrimitiveScene {
    layers: HashMap<u32, PrimitiveLayer>,
}

#[derive(Default)]
pub struct PrimitiveRenderWorld {
    scenes: HashMap<u32, PrimitiveScene>,
}

impl PrimitiveRenderWorld {
    pub fn upsert_instance(&mut self, update: PrimitiveObjectUpdate) {
        let scene = self.scenes.entry(update.scene_id).or_default();
        let layer = scene.layers.entry(update.layer_id).or_default();
        layer.upsert_instance(update);
    }

    pub fn replace_chunk(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        updates: Vec<PrimitiveObjectUpdate>,
    ) {
        let scene = self.scenes.entry(scene_id).or_default();
        let layer = scene.layers.entry(layer_id).or_default();
        if let Some(previous) = layer.chunks.remove(&chunk_id) {
            for object_id in previous {
                layer.remove_instance(object_id);
            }
        }
        let mut object_ids = Vec::with_capacity(updates.len());
        if !layer.chunk_order.contains(&chunk_id) {
            layer.chunk_order.push(chunk_id);
        }
        let chunk_visible = updates.iter().any(|update| update.visible);
        for update in updates {
            layer.remove_instance(update.object_id);
            object_ids.push(update.object_id);
            layer.object_chunks.insert(update.object_id, chunk_id);
            layer.upsert_instance(update);
        }
        layer.chunks.insert(chunk_id, object_ids);
        layer.chunk_visible.insert(chunk_id, chunk_visible);
        layer.rebuild_chunk_bounds(chunk_id);
    }

    pub fn set_chunk_visible(
        &mut self,
        scene_id: u32,
        layer_id: u32,
        chunk_id: u32,
        visible: bool,
    ) {
        let Some(scene) = self.scenes.get_mut(&scene_id) else {
            return;
        };
        let Some(layer) = scene.layers.get_mut(&layer_id) else {
            return;
        };
        layer.set_chunk_visible(chunk_id, visible);
    }

    pub fn destroy_instance(&mut self, scene_id: u32, layer_id: u32, object_id: u32) {
        let Some(scene) = self.scenes.get_mut(&scene_id) else {
            return;
        };
        let Some(layer) = scene.layers.get_mut(&layer_id) else {
            return;
        };
        layer.remove_instance(object_id);
    }

    pub fn clear_layer(&mut self, scene_id: u32, layer_id: u32) {
        let Some(scene) = self.scenes.get_mut(&scene_id) else {
            return;
        };
        let layer = scene.layers.entry(layer_id).or_default();
        layer.objects.clear();
        layer.order.clear();
        layer.visible.clear();
        layer.visible_order.clear();
        layer.chunks.clear();
        layer.chunk_order.clear();
        layer.chunk_visible.clear();
        layer.chunk_bounds.clear();
        layer.object_chunks.clear();
    }

    pub fn destroy_layer(&mut self, scene_id: u32, layer_id: u32) {
        let Some(scene) = self.scenes.get_mut(&scene_id) else {
            return;
        };
        scene.layers.remove(&layer_id);
    }

    pub fn clear_scene(&mut self, scene_id: u32) {
        let scene = self.scenes.entry(scene_id).or_default();
        scene.layers.clear();
    }

    pub fn collect_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for (layer_id, layer) in &scene.layers {
            let mut emitted_chunks = HashSet::new();
            for chunk_id in &layer.chunk_order {
                if !layer.chunk_visible.get(chunk_id).copied().unwrap_or(false) {
                    continue;
                }
                if !layer.chunks.contains_key(chunk_id) {
                    continue;
                }
                if let (Some(camera), Some(bounds)) = (camera, layer.chunk_bounds.get(chunk_id)) {
                    if !primitive_bounds_visible(camera, *bounds) {
                        continue;
                    }
                }
                if emitted_chunks.insert(*chunk_id) {
                    chunks.push(PrimitiveChunkRef {
                        scene_id,
                        layer_id: *layer_id,
                        chunk_id: *chunk_id,
                    });
                }
            }
            for object_id in &layer.visible_order {
                let Some(object) = layer.objects.get(object_id) else {
                    continue;
                };
                if object.model_id == 0 {
                    continue;
                }
                if layer.object_chunks.contains_key(object_id) {
                    continue;
                }
                if let Some(camera) = camera {
                    let bounds = primitive_object_bounds(object.pos, object.scale);
                    if !primitive_bounds_visible(camera, bounds) {
                        continue;
                    }
                }
                draws.push(PrimitiveDraw {
                    model_id: object.model_id,
                    model_uniform: object.model_uniform,
                    material: object.material,
                });
            }
        }
    }
}

impl PrimitiveLayer {
    fn upsert_instance(&mut self, update: PrimitiveObjectUpdate) {
        if !self.objects.contains_key(&update.object_id) {
            self.order.push(update.object_id);
        }
        let was_visible = self
            .visible
            .get(&update.object_id)
            .copied()
            .unwrap_or(false);
        if update.visible && !was_visible {
            self.visible_order.push(update.object_id);
        }
        if !update.visible && was_visible {
            self.visible_order.retain(|id| *id != update.object_id);
        }
        self.visible.insert(update.object_id, update.visible);
        self.objects.insert(
            update.object_id,
            PrimitiveObject {
                model_id: update.model_id,
                model_uniform: primitive_model_uniform(update.pos, update.rot, update.scale),
                material: update.material,
                pos: update.pos,
                scale: update.scale,
            },
        );
        if let Some(chunk_id) = self.object_chunks.get(&update.object_id).copied() {
            self.rebuild_chunk_bounds(chunk_id);
        }
    }

    fn remove_instance(&mut self, object_id: u32) {
        self.objects.remove(&object_id);
        self.visible.remove(&object_id);
        let previous_chunk = self.object_chunks.remove(&object_id);
        self.order.retain(|id| *id != object_id);
        self.visible_order.retain(|id| *id != object_id);
        if let Some(chunk_id) = previous_chunk {
            if let Some(objects) = self.chunks.get_mut(&chunk_id) {
                objects.retain(|id| *id != object_id);
                if objects.is_empty() {
                    self.chunks.remove(&chunk_id);
                    self.chunk_visible.remove(&chunk_id);
                    self.chunk_bounds.remove(&chunk_id);
                    self.chunk_order.retain(|id| *id != chunk_id);
                } else {
                    self.rebuild_chunk_bounds(chunk_id);
                }
            }
        } else {
            let mut touched_chunks = Vec::new();
            for (chunk_id, objects) in self.chunks.iter_mut() {
                let before = objects.len();
                objects.retain(|id| *id != object_id);
                if objects.len() != before {
                    touched_chunks.push(*chunk_id);
                }
            }
            for chunk_id in touched_chunks {
                self.rebuild_chunk_bounds(chunk_id);
            }
        }
    }

    fn set_chunk_visible(&mut self, chunk_id: u32, visible: bool) {
        if !self.chunks.contains_key(&chunk_id) {
            return;
        }
        if !self.chunk_order.contains(&chunk_id) {
            self.chunk_order.push(chunk_id);
        }
        self.chunk_visible.insert(chunk_id, visible);
    }

    fn rebuild_chunk_bounds(&mut self, chunk_id: u32) {
        let Some(objects) = self.chunks.get(&chunk_id) else {
            self.chunk_bounds.remove(&chunk_id);
            return;
        };
        let mut bounds: Option<PrimitiveBounds> = None;
        for object_id in objects {
            let Some(object) = self.objects.get(object_id) else {
                continue;
            };
            let object_bounds = primitive_object_bounds(object.pos, object.scale);
            bounds = Some(match bounds {
                Some(existing) => merge_primitive_bounds(existing, object_bounds),
                None => object_bounds,
            });
        }
        if let Some(bounds) = bounds {
            self.chunk_bounds.insert(chunk_id, bounds);
        } else {
            self.chunk_bounds.remove(&chunk_id);
        }
    }
}

fn primitive_object_bounds(pos: Vec3, scale: Vec3) -> PrimitiveBounds {
    let normalized_scale = Vec3::new(
        scale.x.abs().max(1.0),
        scale.y.abs().max(1.0),
        scale.z.abs().max(1.0),
    );
    let radius = (normalized_scale * 0.5).length();
    let half = Vec3::new(radius, radius, radius);
    PrimitiveBounds {
        min: Vec3::new(pos.x - half.x, pos.y - half.y, pos.z - half.z),
        max: Vec3::new(pos.x + half.x, pos.y + half.y, pos.z + half.z),
    }
}

fn merge_primitive_bounds(a: PrimitiveBounds, b: PrimitiveBounds) -> PrimitiveBounds {
    PrimitiveBounds {
        min: Vec3::new(
            a.min.x.min(b.min.x),
            a.min.y.min(b.min.y),
            a.min.z.min(b.min.z),
        ),
        max: Vec3::new(
            a.max.x.max(b.max.x),
            a.max.y.max(b.max.y),
            a.max.z.max(b.max.z),
        ),
    }
}

fn primitive_bounds_visible(camera: &Camera3DUniform, bounds: PrimitiveBounds) -> bool {
    let corners = [
        [bounds.min.x, bounds.min.y, bounds.min.z, 1.0],
        [bounds.max.x, bounds.min.y, bounds.min.z, 1.0],
        [bounds.min.x, bounds.max.y, bounds.min.z, 1.0],
        [bounds.max.x, bounds.max.y, bounds.min.z, 1.0],
        [bounds.min.x, bounds.min.y, bounds.max.z, 1.0],
        [bounds.max.x, bounds.min.y, bounds.max.z, 1.0],
        [bounds.min.x, bounds.max.y, bounds.max.z, 1.0],
        [bounds.max.x, bounds.max.y, bounds.max.z, 1.0],
    ];
    let mut outside_left = 0;
    let mut outside_right = 0;
    let mut outside_bottom = 0;
    let mut outside_top = 0;
    let mut outside_near = 0;
    let mut outside_far = 0;
    for corner in corners {
        let clip = math3d::mat4_mul_vec4(&camera.view_proj, corner);
        let w = clip[3];
        if clip[0] < -w {
            outside_left += 1;
        }
        if clip[0] > w {
            outside_right += 1;
        }
        if clip[1] < -w {
            outside_bottom += 1;
        }
        if clip[1] > w {
            outside_top += 1;
        }
        if clip[2] < 0.0 {
            outside_near += 1;
        }
        if clip[2] > w {
            outside_far += 1;
        }
    }
    outside_left < 8
        && outside_right < 8
        && outside_bottom < 8
        && outside_top < 8
        && outside_near < 8
        && outside_far < 8
}

fn primitive_model_uniform(pos: Vec3, rot: Quat, scale: Vec3) -> ModelUniform {
    let model_mat = math3d::model_matrix(pos, rot, scale);
    let normal_mat = math3d::normal_matrix(&model_mat);
    ModelUniform {
        model: model_mat,
        normal_matrix: normal_mat,
        base_color: [1.0, 1.0, 1.0, 1.0],
        material_params: [1.0, 1.0, 1.0, 1.0],
        emissive_color: [0.0, 0.0, 0.0, 0.0],
        texture_flags: [0.0, 0.0, 0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::{PrimitiveObjectUpdate, PrimitiveRenderWorld};
    use crate::math3d::{Quat, Vec3};
    use crate::pipeline3d::{Camera3DUniform, MaterialOverride};

    #[test]
    fn primitive_world_collects_without_model_scene() {
        let mut world = PrimitiveRenderWorld::default();
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 7,
            layer_id: 2,
            object_id: 11,
            model_id: 99,
            pos: Vec3::new(1.0, 2.0, 3.0),
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(7, None, &mut draws, &mut chunks);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model_id, 99);
        assert!(chunks.is_empty());
    }

    #[test]
    fn primitive_world_layer_lifecycle() {
        let mut world = PrimitiveRenderWorld::default();
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 1,
            layer_id: 3,
            object_id: 4,
            model_id: 10,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        world.destroy_instance(1, 3, 4);
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(1, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());

        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 1,
            layer_id: 3,
            object_id: 5,
            model_id: 11,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        world.clear_layer(1, 3);
        world.collect_draws(1, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
    }

    #[test]
    fn primitive_world_collects_visible_instances_only() {
        let mut world = PrimitiveRenderWorld::default();
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 2,
            layer_id: 1,
            object_id: 1,
            model_id: 21,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 2,
            layer_id: 1,
            object_id: 2,
            model_id: 22,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: false,
        });
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(2, None, &mut draws, &mut chunks);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model_id, 21);

        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 2,
            layer_id: 1,
            object_id: 1,
            model_id: 21,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: false,
        });
        draws.clear();
        world.collect_draws(2, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
    }

    #[test]
    fn primitive_world_replaces_static_chunks() {
        let mut world = PrimitiveRenderWorld::default();
        world.replace_chunk(
            3,
            4,
            10,
            vec![
                PrimitiveObjectUpdate {
                    scene_id: 3,
                    layer_id: 4,
                    object_id: 1,
                    model_id: 31,
                    pos: Vec3::ZERO,
                    rot: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    material: MaterialOverride::default(),
                    visible: true,
                },
                PrimitiveObjectUpdate {
                    scene_id: 3,
                    layer_id: 4,
                    object_id: 2,
                    model_id: 32,
                    pos: Vec3::ZERO,
                    rot: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    material: MaterialOverride::default(),
                    visible: true,
                },
            ],
        );
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(3, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_id, 10);

        world.set_chunk_visible(3, 4, 10, false);
        chunks.clear();
        world.collect_draws(3, None, &mut draws, &mut chunks);
        assert!(chunks.is_empty());

        world.set_chunk_visible(3, 4, 10, true);
        chunks.clear();
        world.collect_draws(3, None, &mut draws, &mut chunks);
        assert_eq!(chunks.len(), 1);

        world.replace_chunk(
            3,
            4,
            10,
            vec![PrimitiveObjectUpdate {
                scene_id: 3,
                layer_id: 4,
                object_id: 2,
                model_id: 42,
                pos: Vec3::ZERO,
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
                material: MaterialOverride::default(),
                visible: true,
            }],
        );
        draws.clear();
        chunks.clear();
        world.collect_draws(3, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn primitive_world_culls_chunks_against_camera_in_rust() {
        let mut world = PrimitiveRenderWorld::default();
        world.replace_chunk(
            9,
            1,
            1,
            vec![PrimitiveObjectUpdate {
                scene_id: 9,
                layer_id: 1,
                object_id: 1,
                model_id: 31,
                pos: Vec3::ZERO,
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
                material: MaterialOverride::default(),
                visible: true,
            }],
        );
        world.replace_chunk(
            9,
            1,
            2,
            vec![PrimitiveObjectUpdate {
                scene_id: 9,
                layer_id: 1,
                object_id: 2,
                model_id: 31,
                pos: Vec3::new(10.0, 0.0, 0.0),
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
                material: MaterialOverride::default(),
                visible: true,
            }],
        );
        let camera = Camera3DUniform {
            view_proj: crate::math3d::MAT4_IDENTITY,
            camera_pos: [0.0, 0.0, 0.0],
            _pad: 0.0,
        };
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(9, Some(&camera), &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_id, 1);
    }

    #[test]
    fn primitive_world_culls_non_chunk_instances_against_camera_in_rust() {
        let mut world = PrimitiveRenderWorld::default();
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 10,
            layer_id: 1,
            object_id: 1,
            model_id: 51,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 10,
            layer_id: 1,
            object_id: 2,
            model_id: 52,
            pos: Vec3::new(10.0, 0.0, 0.0),
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
        });
        let camera = Camera3DUniform {
            view_proj: crate::math3d::MAT4_IDENTITY,
            camera_pos: [0.0, 0.0, 0.0],
            _pad: 0.0,
        };
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(10, Some(&camera), &mut draws, &mut chunks);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model_id, 51);
        assert!(chunks.is_empty());
    }
}
