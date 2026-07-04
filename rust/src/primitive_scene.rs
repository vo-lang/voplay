use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelUniform};

pub const PRIMITIVE_FLAG_NO_SHADOW: u32 = 1;
pub const PRIMITIVE_FLAG_BILLBOARD: u32 = 4;
pub const PRIMITIVE_FLAG_Y_BILLBOARD: u32 = 8;
pub const PRIMITIVE_FLAG_ATLAS_UV: u32 = 16;
pub const PRIMITIVE_FLAG_WATER_SURFACE: u32 = 32;

#[derive(Clone, Copy)]
pub struct PrimitiveDraw {
    pub model_id: u32,
    pub model_uniform: ModelUniform,
    pub material: MaterialOverride,
    pub instance_params: [f32; 4],
    pub instance_params2: [f32; 4],
}

impl PrimitiveDraw {
    pub fn from_update(update: PrimitiveObjectUpdate) -> Self {
        Self {
            model_id: update.model_id,
            model_uniform: primitive_model_uniform(update.pos, update.rot, update.scale),
            material: update.material,
            instance_params: primitive_instance_params(
                update.flags,
                update.lod_near,
                update.lod_far,
                update.wind_strength,
            ),
            instance_params2: primitive_instance_params2(update.atlas_uv),
        }
    }

    pub fn flags(&self) -> u32 {
        (self.instance_params[0].max(0.0) + 0.5) as u32
    }

    pub fn is_water_surface(&self) -> bool {
        (self.flags() & PRIMITIVE_FLAG_WATER_SURFACE) != 0
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PrimitiveChunkRef {
    pub scene_id: u32,
    pub layer_id: u32,
    pub chunk_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrimitiveChunkBatchInfo {
    pub chunk: PrimitiveChunkRef,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub min_lod_near: f32,
    pub max_lod_far: f32,
    pub all_have_far_lod: bool,
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
    pub flags: u32,
    pub lod_near: f32,
    pub lod_far: f32,
    pub wind_strength: f32,
    pub atlas_uv: [f32; 4],
}

#[derive(Clone, Copy)]
struct PrimitiveObject {
    model_id: u32,
    model_uniform: ModelUniform,
    material: MaterialOverride,
    pos: Vec3,
    scale: Vec3,
    flags: u32,
    lod_near: f32,
    lod_far: f32,
    wind_strength: f32,
    atlas_uv: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct PrimitiveBounds {
    min: Vec3,
    max: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct PrimitiveChunkMeta {
    has_main_draws: bool,
    has_depth_draws: bool,
    has_shadow_draws: bool,
    min_lod_near: f32,
    max_lod_far: f32,
    all_have_far_lod: bool,
}

#[derive(Clone, Copy, Debug)]
struct PrimitiveChunkRuntime {
    chunk_id: u32,
    visible: bool,
    bounds: PrimitiveBounds,
    meta: PrimitiveChunkMeta,
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
    chunk_meta: HashMap<u32, PrimitiveChunkMeta>,
    chunk_runtime: Vec<PrimitiveChunkRuntime>,
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
        layer.rebuild_chunk_metadata(chunk_id);
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
        layer.chunk_meta.clear();
        layer.chunk_runtime.clear();
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
        let mut chunk_info = Vec::new();
        self.collect_draws_with_chunk_info(scene_id, camera, draws, chunks, &mut chunk_info);
    }

    pub fn collect_draws_with_chunk_info(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
        chunk_info: &mut Vec<PrimitiveChunkBatchInfo>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for (layer_id, layer) in &scene.layers {
            for chunk in &layer.chunk_runtime {
                if !chunk.visible {
                    continue;
                }
                let meta = chunk.meta;
                if !meta.has_main_draws {
                    continue;
                }
                if let Some(camera) = camera {
                    if !primitive_chunk_lod_visible(camera, chunk.bounds, meta) {
                        continue;
                    }
                    if !primitive_bounds_visible(camera, chunk.bounds) {
                        continue;
                    }
                }
                let chunk_ref = PrimitiveChunkRef {
                    scene_id,
                    layer_id: *layer_id,
                    chunk_id: chunk.chunk_id,
                };
                chunks.push(chunk_ref);
                chunk_info.push(PrimitiveChunkBatchInfo {
                    chunk: chunk_ref,
                    bounds_min: chunk.bounds.min,
                    bounds_max: chunk.bounds.max,
                    min_lod_near: meta.min_lod_near,
                    max_lod_far: meta.max_lod_far,
                    all_have_far_lod: meta.all_have_far_lod,
                });
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
                    if !primitive_lod_visible(camera, object) {
                        continue;
                    }
                    let bounds = primitive_object_bounds(object.pos, object.scale);
                    if !primitive_bounds_visible(camera, bounds) {
                        continue;
                    }
                }
                draws.push(PrimitiveDraw {
                    model_id: object.model_id,
                    model_uniform: object.model_uniform,
                    material: object.material,
                    instance_params: primitive_instance_params(
                        object.flags,
                        object.lod_near,
                        object.lod_far,
                        object.wind_strength,
                    ),
                    instance_params2: primitive_instance_params2(object.atlas_uv),
                });
            }
        }
    }

    pub fn collect_depth_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.collect_shadow_like_draws(scene_id, camera, None, draws, chunks, false);
    }

    #[cfg(test)]
    pub fn collect_shadow_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.collect_shadow_like_draws(scene_id, camera, None, draws, chunks, true);
    }

    pub fn collect_shadow_objects(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for layer in scene.layers.values() {
            layer.collect_shadow_like_objects(camera, None, draws, true);
        }
    }

    pub fn collect_shadow_chunks_from_candidates(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        candidate_chunks: &[PrimitiveChunkRef],
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for chunk_ref in candidate_chunks {
            if chunk_ref.scene_id != scene_id {
                continue;
            }
            let Some(layer) = scene.layers.get(&chunk_ref.layer_id) else {
                continue;
            };
            if !layer.shadow_chunk_candidate_visible(chunk_ref.chunk_id, camera, None) {
                continue;
            }
            chunks.push(*chunk_ref);
        }
    }

    pub fn collect_shadow_objects_for_light_view(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: &Camera3DUniform,
        draws: &mut Vec<PrimitiveDraw>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for layer in scene.layers.values() {
            layer.collect_shadow_like_objects(camera, Some(light_camera), draws, true);
        }
    }

    pub fn collect_shadow_chunks_for_light_view(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: &Camera3DUniform,
        candidate_chunks: &[PrimitiveChunkRef],
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for chunk_ref in candidate_chunks {
            if chunk_ref.scene_id != scene_id {
                continue;
            }
            let Some(layer) = scene.layers.get(&chunk_ref.layer_id) else {
                continue;
            };
            if !layer.shadow_chunk_candidate_visible(chunk_ref.chunk_id, camera, Some(light_camera))
            {
                continue;
            }
            chunks.push(*chunk_ref);
        }
    }

    fn collect_shadow_like_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
        shadow_only: bool,
    ) {
        let Some(scene) = self.scenes.get(&scene_id) else {
            return;
        };
        for (layer_id, layer) in &scene.layers {
            for chunk in &layer.chunk_runtime {
                if !chunk.visible {
                    continue;
                }
                let meta = chunk.meta;
                let has_pass_draws = if shadow_only {
                    meta.has_shadow_draws
                } else {
                    meta.has_depth_draws
                };
                if !has_pass_draws {
                    continue;
                }
                if let Some(camera) = camera {
                    if !primitive_bounds_visible(camera, chunk.bounds) {
                        continue;
                    }
                    if !primitive_chunk_lod_visible(camera, chunk.bounds, meta) {
                        continue;
                    }
                }
                if let Some(light_camera) = light_camera {
                    if !primitive_bounds_visible(light_camera, chunk.bounds) {
                        continue;
                    }
                }
                chunks.push(PrimitiveChunkRef {
                    scene_id,
                    layer_id: *layer_id,
                    chunk_id: chunk.chunk_id,
                });
            }
            layer.collect_shadow_like_objects(camera, light_camera, draws, shadow_only);
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
                flags: update.flags,
                lod_near: update.lod_near.max(0.0),
                lod_far: update.lod_far.max(0.0),
                wind_strength: update.wind_strength.max(0.0),
                atlas_uv: primitive_instance_params2(update.atlas_uv),
            },
        );
        if let Some(chunk_id) = self.object_chunks.get(&update.object_id).copied() {
            if self.chunks.contains_key(&chunk_id) {
                self.rebuild_chunk_metadata(chunk_id);
            }
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
                    self.chunk_meta.remove(&chunk_id);
                    self.chunk_order.retain(|id| *id != chunk_id);
                    self.rebuild_chunk_runtime();
                } else {
                    self.rebuild_chunk_metadata(chunk_id);
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
                self.rebuild_chunk_metadata(chunk_id);
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
        self.rebuild_chunk_runtime();
    }

    fn rebuild_chunk_metadata(&mut self, chunk_id: u32) {
        let Some(objects) = self.chunks.get(&chunk_id) else {
            self.chunk_bounds.remove(&chunk_id);
            self.chunk_meta.remove(&chunk_id);
            self.rebuild_chunk_runtime();
            return;
        };
        let mut bounds: Option<PrimitiveBounds> = None;
        let mut meta = PrimitiveChunkMeta {
            has_main_draws: false,
            has_depth_draws: false,
            has_shadow_draws: false,
            min_lod_near: f32::INFINITY,
            max_lod_far: 0.0,
            all_have_far_lod: true,
        };
        for object_id in objects {
            let Some(object) = self.objects.get(object_id) else {
                continue;
            };
            if !self.visible.get(object_id).copied().unwrap_or(false) || object.model_id == 0 {
                continue;
            }
            let object_bounds = primitive_object_bounds(object.pos, object.scale);
            bounds = Some(match bounds {
                Some(existing) => merge_primitive_bounds(existing, object_bounds),
                None => object_bounds,
            });
            meta.has_main_draws = true;
            if !primitive_has_alpha_card_flags(object.flags)
                && (object.flags & PRIMITIVE_FLAG_WATER_SURFACE) == 0
            {
                meta.has_depth_draws = true;
                if (object.flags & PRIMITIVE_FLAG_NO_SHADOW) == 0 {
                    meta.has_shadow_draws = true;
                }
            }
            if object.lod_near > 0.0 {
                meta.min_lod_near = meta.min_lod_near.min(object.lod_near);
            } else {
                meta.min_lod_near = 0.0;
            }
            if object.lod_far > 0.0 {
                meta.max_lod_far = meta.max_lod_far.max(object.lod_far);
            } else {
                meta.all_have_far_lod = false;
            }
        }
        if let Some(bounds) = bounds {
            if meta.min_lod_near == f32::INFINITY {
                meta.min_lod_near = 0.0;
            }
            self.chunk_bounds.insert(chunk_id, bounds);
            self.chunk_meta.insert(chunk_id, meta);
        } else {
            self.chunk_bounds.remove(&chunk_id);
            self.chunk_meta.remove(&chunk_id);
        }
        self.rebuild_chunk_runtime();
    }

    fn rebuild_chunk_runtime(&mut self) {
        self.chunk_runtime.clear();
        self.chunk_runtime.reserve(self.chunk_order.len());
        for chunk_id in &self.chunk_order {
            if !self.chunks.contains_key(chunk_id) {
                continue;
            }
            let Some(bounds) = self.chunk_bounds.get(chunk_id).copied() else {
                continue;
            };
            let Some(meta) = self.chunk_meta.get(chunk_id).copied() else {
                continue;
            };
            self.chunk_runtime.push(PrimitiveChunkRuntime {
                chunk_id: *chunk_id,
                visible: self.chunk_visible.get(chunk_id).copied().unwrap_or(false),
                bounds,
                meta,
            });
        }
    }

    fn shadow_chunk_candidate_visible(
        &self,
        chunk_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: Option<&Camera3DUniform>,
    ) -> bool {
        if !self.chunk_visible.get(&chunk_id).copied().unwrap_or(false) {
            return false;
        }
        let Some(bounds) = self.chunk_bounds.get(&chunk_id).copied() else {
            return false;
        };
        let Some(meta) = self.chunk_meta.get(&chunk_id).copied() else {
            return false;
        };
        if !meta.has_shadow_draws {
            return false;
        }
        if let Some(camera) = camera {
            if !primitive_bounds_visible(camera, bounds) {
                return false;
            }
            if !primitive_chunk_lod_visible(camera, bounds, meta) {
                return false;
            }
        }
        if let Some(light_camera) = light_camera {
            if !primitive_bounds_visible(light_camera, bounds) {
                return false;
            }
        }
        true
    }

    fn collect_shadow_like_objects(
        &self,
        camera: Option<&Camera3DUniform>,
        light_camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        shadow_only: bool,
    ) {
        for object_id in &self.visible_order {
            if self.object_chunks.contains_key(object_id) {
                continue;
            }
            let Some(object) = self.objects.get(object_id) else {
                continue;
            };
            if object.model_id == 0 {
                continue;
            }
            if shadow_only && (object.flags & PRIMITIVE_FLAG_NO_SHADOW) != 0 {
                continue;
            }
            if primitive_has_alpha_card_flags(object.flags) {
                continue;
            }
            if let Some(camera) = camera {
                if !primitive_lod_visible(camera, object) {
                    continue;
                }
                let bounds = primitive_object_bounds(object.pos, object.scale);
                if !primitive_bounds_visible(camera, bounds) {
                    continue;
                }
            }
            if let Some(light_camera) = light_camera {
                let bounds = primitive_object_bounds(object.pos, object.scale);
                if !primitive_bounds_visible(light_camera, bounds) {
                    continue;
                }
            }
            draws.push(PrimitiveDraw {
                model_id: object.model_id,
                model_uniform: object.model_uniform,
                material: object.material,
                instance_params: primitive_instance_params(
                    object.flags,
                    object.lod_near,
                    object.lod_far,
                    object.wind_strength,
                ),
                instance_params2: primitive_instance_params2(object.atlas_uv),
            });
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
        material_response: [1.0, 0.0, 1.0, 1.0],
        texture_flags2: [0.0, 0.0, 0.0, 0.0],
    }
}

fn primitive_instance_params(
    flags: u32,
    lod_near: f32,
    lod_far: f32,
    wind_strength: f32,
) -> [f32; 4] {
    [
        flags as f32,
        lod_near.max(0.0),
        lod_far.max(0.0),
        wind_strength.max(0.0),
    ]
}

fn primitive_instance_params2(atlas_uv: [f32; 4]) -> [f32; 4] {
    if atlas_uv[2] > 0.0 && atlas_uv[3] > 0.0 {
        [
            atlas_uv[0].clamp(0.0, 1.0),
            atlas_uv[1].clamp(0.0, 1.0),
            atlas_uv[2].clamp(0.0, 1.0),
            atlas_uv[3].clamp(0.0, 1.0),
        ]
    } else {
        [0.0, 0.0, 1.0, 1.0]
    }
}

fn primitive_has_alpha_card_flags(flags: u32) -> bool {
    (flags & (PRIMITIVE_FLAG_BILLBOARD | PRIMITIVE_FLAG_Y_BILLBOARD | PRIMITIVE_FLAG_ATLAS_UV)) != 0
}

fn primitive_chunk_lod_visible(
    camera: &Camera3DUniform,
    bounds: PrimitiveBounds,
    meta: PrimitiveChunkMeta,
) -> bool {
    let camera_pos = primitive_camera_position(camera);
    if meta.all_have_far_lod && meta.max_lod_far > 0.0 {
        let far = meta.max_lod_far;
        if primitive_distance_to_bounds_sq(camera_pos, bounds) > far * far {
            return false;
        }
    }
    if meta.min_lod_near > 0.0 {
        let near = meta.min_lod_near;
        if primitive_max_distance_to_bounds_sq(camera_pos, bounds) < near * near {
            return false;
        }
    }
    true
}

fn primitive_lod_visible(camera: &Camera3DUniform, object: &PrimitiveObject) -> bool {
    let camera_pos = primitive_camera_position(camera);
    let offset = object.pos - camera_pos;
    let dist_sq = offset.dot(offset);
    if object.lod_near > 0.0 && dist_sq < object.lod_near * object.lod_near {
        return false;
    }
    if object.lod_far > 0.0 && dist_sq > object.lod_far * object.lod_far {
        return false;
    }
    true
}

fn primitive_camera_position(camera: &Camera3DUniform) -> Vec3 {
    Vec3::new(
        camera.camera_pos[0],
        camera.camera_pos[1],
        camera.camera_pos[2],
    )
}

fn primitive_distance_to_bounds_sq(pos: Vec3, bounds: PrimitiveBounds) -> f32 {
    let dx = if pos.x < bounds.min.x {
        bounds.min.x - pos.x
    } else if pos.x > bounds.max.x {
        pos.x - bounds.max.x
    } else {
        0.0
    };
    let dy = if pos.y < bounds.min.y {
        bounds.min.y - pos.y
    } else if pos.y > bounds.max.y {
        pos.y - bounds.max.y
    } else {
        0.0
    };
    let dz = if pos.z < bounds.min.z {
        bounds.min.z - pos.z
    } else if pos.z > bounds.max.z {
        pos.z - bounds.max.z
    } else {
        0.0
    };
    dx * dx + dy * dy + dz * dz
}

fn primitive_max_distance_to_bounds_sq(pos: Vec3, bounds: PrimitiveBounds) -> f32 {
    let dx = (pos.x - bounds.min.x)
        .abs()
        .max((pos.x - bounds.max.x).abs());
    let dy = (pos.y - bounds.min.y)
        .abs()
        .max((pos.y - bounds.max.y).abs());
    let dz = (pos.z - bounds.min.z)
        .abs()
        .max((pos.z - bounds.max.z).abs());
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::{
        PrimitiveObjectUpdate, PrimitiveRenderWorld, PRIMITIVE_FLAG_ATLAS_UV,
        PRIMITIVE_FLAG_NO_SHADOW,
    };
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.25, 0.5, 0.125, 0.25],
        });
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(7, None, &mut draws, &mut chunks);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model_id, 99);
        assert_eq!(draws[0].instance_params2, [0.25, 0.5, 0.125, 0.25]);
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
                    flags: 0,
                    lod_near: 0.0,
                    lod_far: 0.0,
                    wind_strength: 0.0,
                    atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
                    flags: 0,
                    lod_near: 0.0,
                    lod_far: 0.0,
                    wind_strength: 0.0,
                    atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
                flags: 0,
                lod_near: 0.0,
                lod_far: 0.0,
                wind_strength: 0.0,
                atlas_uv: [0.0, 0.0, 1.0, 1.0],
            }],
        );
        draws.clear();
        chunks.clear();
        world.collect_draws(3, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert_eq!(chunks.len(), 1);

        let mut shadow_chunks_from_candidates = Vec::new();
        world.collect_shadow_chunks_from_candidates(
            3,
            None,
            &chunks,
            &mut shadow_chunks_from_candidates,
        );
        assert_eq!(shadow_chunks_from_candidates.len(), 1);
        assert_eq!(shadow_chunks_from_candidates[0].chunk_id, 10);

        let mut shadow_draws = Vec::new();
        let mut shadow_chunks = Vec::new();
        world.collect_shadow_draws(3, None, &mut shadow_draws, &mut shadow_chunks);
        assert!(shadow_draws.is_empty());
        assert_eq!(shadow_chunks.len(), 1);
        assert_eq!(shadow_chunks[0].chunk_id, 10);
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
                flags: 0,
                lod_near: 0.0,
                lod_far: 0.0,
                wind_strength: 0.0,
                atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
                flags: 0,
                lod_near: 0.0,
                lod_far: 0.0,
                wind_strength: 0.0,
                atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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
            flags: 0,
            lod_near: 0.0,
            lod_far: 0.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
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

    #[test]
    fn primitive_world_filters_lod_and_shadow_participation() {
        let mut world = PrimitiveRenderWorld::default();
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 11,
            layer_id: 1,
            object_id: 1,
            model_id: 61,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
            flags: PRIMITIVE_FLAG_NO_SHADOW,
            lod_near: 0.0,
            lod_far: 5.0,
            wind_strength: 0.4,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
        });
        world.upsert_instance(PrimitiveObjectUpdate {
            scene_id: 11,
            layer_id: 1,
            object_id: 2,
            model_id: 62,
            pos: Vec3::new(10.0, 0.0, 0.0),
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
            flags: 0,
            lod_near: 0.0,
            lod_far: 5.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
        });
        let camera = Camera3DUniform {
            view_proj: crate::math3d::MAT4_IDENTITY,
            camera_pos: [0.0, 0.0, 0.0],
            _pad: 0.0,
        };
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(11, Some(&camera), &mut draws, &mut chunks);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model_id, 61);
        assert_eq!(draws[0].instance_params, [1.0, 0.0, 5.0, 0.4]);
        let mut depth_draws = Vec::new();
        let mut depth_chunks = Vec::new();
        world.collect_depth_draws(11, Some(&camera), &mut depth_draws, &mut depth_chunks);
        assert_eq!(depth_draws.len(), 1);
        assert!(depth_chunks.is_empty());
        let mut shadow_draws = Vec::new();
        let mut shadow_chunks = Vec::new();
        world.collect_shadow_draws(11, Some(&camera), &mut shadow_draws, &mut shadow_chunks);
        assert!(shadow_draws.is_empty());
        assert!(shadow_chunks.is_empty());
    }

    #[test]
    fn primitive_world_filters_chunk_lod_from_metadata() {
        let mut world = PrimitiveRenderWorld::default();
        world.replace_chunk(
            12,
            1,
            1,
            vec![PrimitiveObjectUpdate {
                scene_id: 12,
                layer_id: 1,
                object_id: 1,
                model_id: 71,
                pos: Vec3::new(20.0, 0.0, 0.0),
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
                material: MaterialOverride::default(),
                visible: true,
                flags: 0,
                lod_near: 0.0,
                lod_far: 5.0,
                wind_strength: 0.0,
                atlas_uv: [0.0, 0.0, 1.0, 1.0],
            }],
        );
        let camera = Camera3DUniform {
            view_proj: [[0.0; 4]; 4],
            camera_pos: [0.0, 0.0, 0.0],
            _pad: 0.0,
        };
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(12, Some(&camera), &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert!(chunks.is_empty());
    }

    #[test]
    fn primitive_world_skips_alpha_card_chunks_for_depth_and_shadow() {
        let mut world = PrimitiveRenderWorld::default();
        world.replace_chunk(
            13,
            1,
            1,
            vec![PrimitiveObjectUpdate {
                scene_id: 13,
                layer_id: 1,
                object_id: 1,
                model_id: 81,
                pos: Vec3::ZERO,
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
                material: MaterialOverride::default(),
                visible: true,
                flags: PRIMITIVE_FLAG_ATLAS_UV,
                lod_near: 0.0,
                lod_far: 0.0,
                wind_strength: 0.0,
                atlas_uv: [0.0, 0.0, 1.0, 1.0],
            }],
        );
        let mut draws = Vec::new();
        let mut chunks = Vec::new();
        world.collect_draws(13, None, &mut draws, &mut chunks);
        assert!(draws.is_empty());
        assert_eq!(chunks.len(), 1);

        let mut depth_draws = Vec::new();
        let mut depth_chunks = Vec::new();
        world.collect_depth_draws(13, None, &mut depth_draws, &mut depth_chunks);
        assert!(depth_draws.is_empty());
        assert!(depth_chunks.is_empty());
        let mut shadow_draws = Vec::new();
        let mut shadow_chunks = Vec::new();
        world.collect_shadow_draws(13, None, &mut shadow_draws, &mut shadow_chunks);
        assert!(shadow_draws.is_empty());
        assert!(shadow_chunks.is_empty());
    }
}
