use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::model_loader::ModelManager;
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelDraw, ModelUniform};
use crate::pipeline_post::PostDecalGpu;
use crate::primitive_scene::{
    PrimitiveChunkBatchInfo, PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate,
    PrimitiveRenderWorld,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderChunkBounds {
    pub center: Vec3,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderBatchQualityProfile {
    pub lod1_distance: f32,
    pub lod1_projected_radius: f32,
}

impl Default for RenderBatchQualityProfile {
    fn default() -> Self {
        Self {
            lod1_distance: 160.0,
            lod1_projected_radius: 0.012,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RenderLodRange {
    near: f32,
    far: f32,
    far_applies_to_all: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchKind {
    Mesh,
    Primitive,
    Terrain,
    Water,
    Decal,
}

impl Default for RenderBatchKind {
    fn default() -> Self {
        Self::Primitive
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderWorldChunkResidentState {
    Missing,
    Resident,
    Dirty,
}

impl Default for RenderWorldChunkResidentState {
    fn default() -> Self {
        Self::Missing
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderWorldChunk {
    pub scene_id: u32,
    pub chunk_id: u32,
    pub kind: RenderBatchKind,
    pub bounds: RenderChunkBounds,
    pub lod_level: u8,
    pub material_group: u32,
    pub instance_start: u32,
    pub instance_count: u32,
    pub dirty_start: u32,
    pub dirty_count: u32,
    pub resident_state: RenderWorldChunkResidentState,
    pub last_upload_frame: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderTerrainBatchInput {
    pub draw_index: usize,
    pub bounds: RenderChunkBounds,
    pub material_group: u32,
    pub dirty_start: u32,
    pub dirty_count: u32,
    pub resident_state: RenderWorldChunkResidentState,
    pub last_upload_frame: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderDecalBatchInput {
    pub decal_index: usize,
    pub bounds: RenderChunkBounds,
    pub material_group: u32,
    pub dirty_start: u32,
    pub dirty_count: u32,
    pub resident_state: RenderWorldChunkResidentState,
    pub last_upload_frame: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderBatchPlan {
    pub frame_id: u32,
    pub visible_objects: u32,
    pub model_batch_indices: Vec<usize>,
    pub terrain_batch_indices: Vec<usize>,
    pub primitive_draw_indices: Vec<usize>,
    pub primitive_chunk_indices: Vec<usize>,
    pub water_draw_indices: Vec<usize>,
    pub water_chunk_indices: Vec<usize>,
    pub decal_batch_indices: Vec<usize>,
    pub visible_chunks: Vec<RenderWorldChunk>,
    pub frustum_culled_chunks: u32,
    pub distance_culled_chunks: u32,
    pub lod0_chunks: u32,
    pub lod1_chunks: u32,
    pub dirty_uploads: u32,
    pub resident_rebuilds: u32,
    pub mesh_batches: u32,
    pub primitive_batches: u32,
    pub terrain_batches: u32,
    pub water_batches: u32,
    pub decal_batches: u32,
}

impl RenderBatchPlan {
    pub fn total_batches(&self) -> u32 {
        self.mesh_batches
            + self.primitive_batches
            + self.terrain_batches
            + self.water_batches
            + self.decal_batches
    }

    fn push_chunk(&mut self, chunk: RenderWorldChunk) {
        match chunk.kind {
            RenderBatchKind::Mesh => self.mesh_batches += 1,
            RenderBatchKind::Primitive => self.primitive_batches += 1,
            RenderBatchKind::Terrain => self.terrain_batches += 1,
            RenderBatchKind::Water => self.water_batches += 1,
            RenderBatchKind::Decal => self.decal_batches += 1,
        }
        if chunk.lod_level == 0 {
            self.lod0_chunks += 1;
        } else {
            self.lod1_chunks += 1;
        }
        if chunk.dirty_count > 0 {
            self.dirty_uploads += 1;
        }
        if chunk.resident_state == RenderWorldChunkResidentState::Missing {
            self.resident_rebuilds += 1;
        }
        self.visible_chunks.push(chunk);
    }

    pub fn model_batches(&self, draws: &[ModelDraw]) -> Vec<ModelDraw> {
        self.model_batch_indices
            .iter()
            .filter_map(|index| draws.get(*index).copied())
            .collect()
    }

    pub fn terrain_batches(&self, draws: &[ModelDraw]) -> Vec<ModelDraw> {
        self.terrain_batch_indices
            .iter()
            .filter_map(|index| draws.get(*index).copied())
            .collect()
    }

    pub fn primitive_draw_batches(&self, draws: &[PrimitiveDraw]) -> Vec<PrimitiveDraw> {
        self.primitive_draw_indices
            .iter()
            .filter_map(|index| draws.get(*index).copied())
            .collect()
    }

    pub fn primitive_chunk_batches(&self, chunks: &[PrimitiveChunkRef]) -> Vec<PrimitiveChunkRef> {
        self.primitive_chunk_indices
            .iter()
            .filter_map(|index| chunks.get(*index).copied())
            .collect()
    }

    pub fn water_draw_batches(&self, draws: &[PrimitiveDraw]) -> Vec<PrimitiveDraw> {
        self.water_draw_indices
            .iter()
            .filter_map(|index| draws.get(*index).copied())
            .collect()
    }

    pub fn water_chunk_batches(&self, chunks: &[PrimitiveChunkRef]) -> Vec<PrimitiveChunkRef> {
        self.water_chunk_indices
            .iter()
            .filter_map(|index| chunks.get(*index).copied())
            .collect()
    }

    pub fn decal_batches(&self, decals: &[PostDecalGpu]) -> Vec<PostDecalGpu> {
        self.decal_batch_indices
            .iter()
            .filter_map(|index| decals.get(*index).copied())
            .collect()
    }
}

pub struct RenderBatchPlanner;

impl RenderBatchPlanner {
    pub fn build(
        frame_id: u32,
        scene_id: u32,
        model_draws: &[ModelDraw],
        terrain_inputs: &[RenderTerrainBatchInput],
        primitive_draws: &[PrimitiveDraw],
        primitive_chunks: &[PrimitiveChunkRef],
        primitive_chunk_info: &[PrimitiveChunkBatchInfo],
        decal_inputs: &[RenderDecalBatchInput],
        camera: Option<&Camera3DUniform>,
        quality: RenderBatchQualityProfile,
    ) -> RenderBatchPlan {
        let mut plan = RenderBatchPlan {
            frame_id,
            ..Default::default()
        };
        let terrain_by_draw: HashMap<usize, RenderTerrainBatchInput> = terrain_inputs
            .iter()
            .map(|input| (input.draw_index, *input))
            .collect();
        for (index, draw) in model_draws.iter().enumerate() {
            if let Some(input) = terrain_by_draw.get(&index) {
                let bounds = input.bounds;
                if Self::outside_camera(camera, bounds) {
                    plan.frustum_culled_chunks += 1;
                    continue;
                }
                plan.model_batch_indices.push(index);
                plan.terrain_batch_indices.push(index);
                plan.visible_objects = plan.visible_objects.saturating_add(1);
                plan.push_chunk(RenderWorldChunk {
                    scene_id,
                    chunk_id: draw.model_id,
                    kind: RenderBatchKind::Terrain,
                    bounds,
                    lod_level: Self::select_lod(camera, bounds, RenderLodRange::default(), quality),
                    material_group: input.material_group,
                    instance_start: index as u32,
                    instance_count: 1,
                    dirty_start: input.dirty_start,
                    dirty_count: input.dirty_count,
                    resident_state: input.resident_state,
                    last_upload_frame: input.last_upload_frame,
                });
                continue;
            }
            let bounds = Self::model_draw_bounds(draw);
            if Self::outside_camera(camera, bounds) {
                plan.frustum_culled_chunks += 1;
                continue;
            }
            plan.model_batch_indices.push(index);
            plan.visible_objects = plan.visible_objects.saturating_add(1);
            plan.push_chunk(RenderWorldChunk {
                scene_id,
                chunk_id: draw.model_id,
                kind: RenderBatchKind::Mesh,
                bounds,
                lod_level: Self::select_lod(camera, bounds, RenderLodRange::default(), quality),
                material_group: draw.material.id,
                instance_start: index as u32,
                instance_count: 1,
                dirty_start: 0,
                dirty_count: 0,
                resident_state: RenderWorldChunkResidentState::Resident,
                last_upload_frame: frame_id,
            });
        }
        for (index, chunk) in primitive_chunks.iter().enumerate() {
            let Some(info) = primitive_chunk_info
                .get(index)
                .filter(|info| info.chunk == *chunk)
                .or_else(|| {
                    primitive_chunk_info
                        .iter()
                        .find(|info| info.chunk == *chunk)
                })
            else {
                continue;
            };
            let bounds = Self::primitive_chunk_bounds(*info);
            if Self::outside_camera(camera, bounds) {
                plan.frustum_culled_chunks += 1;
                continue;
            }
            let lod_range = RenderLodRange {
                near: info.min_lod_near,
                far: info.max_lod_far,
                far_applies_to_all: info.all_have_far_lod,
            };
            if Self::outside_distance(camera, bounds, lod_range) {
                plan.distance_culled_chunks += 1;
                continue;
            }
            plan.primitive_chunk_indices.push(index);
            plan.water_chunk_indices.push(index);
            plan.push_chunk(RenderWorldChunk {
                scene_id: chunk.scene_id,
                chunk_id: chunk.chunk_id,
                kind: RenderBatchKind::Primitive,
                bounds,
                lod_level: Self::select_lod(camera, bounds, lod_range, quality),
                material_group: chunk.layer_id,
                instance_start: index as u32,
                instance_count: 1,
                dirty_start: 0,
                dirty_count: 0,
                resident_state: RenderWorldChunkResidentState::Resident,
                last_upload_frame: frame_id,
            });
        }
        for (index, draw) in primitive_draws.iter().enumerate() {
            let bounds = Self::primitive_draw_bounds(draw);
            if Self::outside_camera(camera, bounds) {
                plan.frustum_culled_chunks += 1;
                continue;
            }
            let lod_range = Self::primitive_draw_lod_range(draw);
            if Self::outside_distance(camera, bounds, lod_range) {
                plan.distance_culled_chunks += 1;
                continue;
            }
            let lod_level = Self::select_lod(camera, bounds, lod_range, quality);
            plan.visible_objects = plan.visible_objects.saturating_add(1);
            if draw.is_water_surface() {
                plan.water_draw_indices.push(index);
                plan.push_chunk(RenderWorldChunk {
                    scene_id,
                    chunk_id: 0x8000_0000 | index as u32,
                    kind: RenderBatchKind::Water,
                    bounds,
                    lod_level,
                    material_group: draw.material.id,
                    instance_start: index as u32,
                    instance_count: 1,
                    dirty_start: 0,
                    dirty_count: 0,
                    resident_state: RenderWorldChunkResidentState::Resident,
                    last_upload_frame: frame_id,
                });
            } else {
                plan.primitive_draw_indices.push(index);
                plan.push_chunk(RenderWorldChunk {
                    scene_id,
                    chunk_id: 0x4000_0000 | index as u32,
                    kind: RenderBatchKind::Primitive,
                    bounds,
                    lod_level,
                    material_group: draw.material.id,
                    instance_start: index as u32,
                    instance_count: 1,
                    dirty_start: 0,
                    dirty_count: 0,
                    resident_state: RenderWorldChunkResidentState::Resident,
                    last_upload_frame: frame_id,
                });
            }
        }
        for input in decal_inputs {
            let bounds = input.bounds;
            if Self::outside_camera(camera, bounds) {
                plan.frustum_culled_chunks += 1;
                continue;
            }
            plan.decal_batch_indices.push(input.decal_index);
            plan.visible_objects = plan.visible_objects.saturating_add(1);
            plan.push_chunk(RenderWorldChunk {
                scene_id,
                chunk_id: 0xC000_0000 | input.decal_index as u32,
                kind: RenderBatchKind::Decal,
                bounds,
                lod_level: Self::select_lod(camera, bounds, RenderLodRange::default(), quality),
                material_group: input.material_group,
                instance_start: input.decal_index as u32,
                instance_count: 1,
                dirty_start: input.dirty_start,
                dirty_count: input.dirty_count,
                resident_state: input.resident_state,
                last_upload_frame: input.last_upload_frame,
            });
        }
        plan
    }

    pub fn terrain_inputs(
        frame_id: u32,
        model_draws: &[ModelDraw],
        models: &ModelManager,
    ) -> Vec<RenderTerrainBatchInput> {
        model_draws
            .iter()
            .enumerate()
            .filter_map(|(draw_index, draw)| {
                let model = models.get(draw.model_id)?;
                let has_terrain = model
                    .meshes
                    .iter()
                    .any(|mesh| !mesh.skinned && mesh.material.control_texture_id.is_some());
                if !has_terrain {
                    return None;
                }
                Some(RenderTerrainBatchInput {
                    draw_index,
                    bounds: Self::model_bounds(draw, models),
                    material_group: draw.material.id,
                    dirty_start: 0,
                    dirty_count: 0,
                    resident_state: RenderWorldChunkResidentState::Resident,
                    last_upload_frame: frame_id,
                })
            })
            .collect()
    }

    pub fn decal_inputs(frame_id: u32, decals: &[PostDecalGpu]) -> Vec<RenderDecalBatchInput> {
        decals
            .iter()
            .enumerate()
            .map(|(decal_index, decal)| {
                let (center, radius) = decal.render_batch_bounds();
                RenderDecalBatchInput {
                    decal_index,
                    bounds: RenderChunkBounds {
                        center: Vec3::new(center[0], center[1], center[2]),
                        radius,
                    },
                    material_group: decal.render_batch_material_group(),
                    dirty_start: decal_index as u32,
                    dirty_count: 1,
                    resident_state: RenderWorldChunkResidentState::Dirty,
                    last_upload_frame: frame_id,
                }
            })
            .collect()
    }

    fn select_lod(
        camera: Option<&Camera3DUniform>,
        bounds: RenderChunkBounds,
        range: RenderLodRange,
        quality: RenderBatchQualityProfile,
    ) -> u8 {
        let Some(camera) = camera else {
            return 0;
        };
        let camera_pos = Self::camera_position(camera);
        let distance = (bounds.center - camera_pos).length().max(0.001);
        let projected_radius = bounds.radius / distance;
        if range.far > 0.0 && distance > range.far * 0.72 {
            return 1;
        }
        if distance >= quality.lod1_distance || projected_radius <= quality.lod1_projected_radius {
            1
        } else {
            0
        }
    }

    fn model_draw_bounds(draw: &ModelDraw) -> RenderChunkBounds {
        Self::bounds_from_model_matrix(&draw.model_uniform.model)
    }

    fn model_bounds(draw: &ModelDraw, models: &ModelManager) -> RenderChunkBounds {
        models
            .get(draw.model_id)
            .map(|model| {
                Self::bounds_from_model_aabb(
                    &draw.model_uniform.model,
                    model.aabb_min,
                    model.aabb_max,
                )
            })
            .unwrap_or_else(|| Self::model_draw_bounds(draw))
    }

    fn primitive_draw_bounds(draw: &PrimitiveDraw) -> RenderChunkBounds {
        Self::bounds_from_model_matrix(&draw.model_uniform.model)
    }

    fn primitive_chunk_bounds(info: PrimitiveChunkBatchInfo) -> RenderChunkBounds {
        let center = Vec3::new(
            (info.bounds_min.x + info.bounds_max.x) * 0.5,
            (info.bounds_min.y + info.bounds_max.y) * 0.5,
            (info.bounds_min.z + info.bounds_max.z) * 0.5,
        );
        let radius = (info.bounds_max - center).length().max(0.001);
        RenderChunkBounds { center, radius }
    }

    fn bounds_from_model_matrix(model: &math3d::Mat4) -> RenderChunkBounds {
        let center = Vec3::new(model[3][0], model[3][1], model[3][2]);
        let axis_x = Vec3::new(model[0][0], model[0][1], model[0][2]);
        let axis_y = Vec3::new(model[1][0], model[1][1], model[1][2]);
        let axis_z = Vec3::new(model[2][0], model[2][1], model[2][2]);
        let radius = ((axis_x.length() + axis_y.length() + axis_z.length()) * 0.5).max(0.001);
        RenderChunkBounds { center, radius }
    }

    fn bounds_from_model_aabb(
        model: &math3d::Mat4,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
    ) -> RenderChunkBounds {
        let corners = [
            [aabb_min[0], aabb_min[1], aabb_min[2], 1.0],
            [aabb_max[0], aabb_min[1], aabb_min[2], 1.0],
            [aabb_min[0], aabb_max[1], aabb_min[2], 1.0],
            [aabb_max[0], aabb_max[1], aabb_min[2], 1.0],
            [aabb_min[0], aabb_min[1], aabb_max[2], 1.0],
            [aabb_max[0], aabb_min[1], aabb_max[2], 1.0],
            [aabb_min[0], aabb_max[1], aabb_max[2], 1.0],
            [aabb_max[0], aabb_max[1], aabb_max[2], 1.0],
        ];
        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for corner in corners {
            let world = math3d::mat4_mul_vec4(model, corner);
            min.x = min.x.min(world[0]);
            min.y = min.y.min(world[1]);
            min.z = min.z.min(world[2]);
            max.x = max.x.max(world[0]);
            max.y = max.y.max(world[1]);
            max.z = max.z.max(world[2]);
        }
        let center = (min + max) * 0.5;
        let radius = (max - center).length().max(0.001);
        RenderChunkBounds { center, radius }
    }

    fn primitive_draw_lod_range(draw: &PrimitiveDraw) -> RenderLodRange {
        RenderLodRange {
            near: draw.instance_params[1].max(0.0),
            far: draw.instance_params[2].max(0.0),
            far_applies_to_all: draw.instance_params[2] > 0.0,
        }
    }

    fn outside_camera(camera: Option<&Camera3DUniform>, bounds: RenderChunkBounds) -> bool {
        let Some(camera) = camera else {
            return false;
        };
        !Self::bounds_visible(camera, bounds)
    }

    fn outside_distance(
        camera: Option<&Camera3DUniform>,
        bounds: RenderChunkBounds,
        range: RenderLodRange,
    ) -> bool {
        let Some(camera) = camera else {
            return false;
        };
        let camera_pos = Self::camera_position(camera);
        if range.far_applies_to_all && range.far > 0.0 {
            let far = range.far;
            if Self::distance_to_bounds_sq(camera_pos, bounds) > far * far {
                return true;
            }
        }
        if range.near > 0.0 {
            let near = range.near;
            if Self::max_distance_to_bounds_sq(camera_pos, bounds) < near * near {
                return true;
            }
        }
        false
    }

    fn bounds_visible(camera: &Camera3DUniform, bounds: RenderChunkBounds) -> bool {
        let half = Vec3::new(bounds.radius, bounds.radius, bounds.radius);
        let min = bounds.center - half;
        let max = bounds.center + half;
        let corners = [
            [min.x, min.y, min.z, 1.0],
            [max.x, min.y, min.z, 1.0],
            [min.x, max.y, min.z, 1.0],
            [max.x, max.y, min.z, 1.0],
            [min.x, min.y, max.z, 1.0],
            [max.x, min.y, max.z, 1.0],
            [min.x, max.y, max.z, 1.0],
            [max.x, max.y, max.z, 1.0],
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

    fn camera_position(camera: &Camera3DUniform) -> Vec3 {
        Vec3::new(
            camera.camera_pos[0],
            camera.camera_pos[1],
            camera.camera_pos[2],
        )
    }

    fn distance_to_bounds_sq(pos: Vec3, bounds: RenderChunkBounds) -> f32 {
        let d = (bounds.center - pos).length() - bounds.radius;
        d.max(0.0) * d.max(0.0)
    }

    fn max_distance_to_bounds_sq(pos: Vec3, bounds: RenderChunkBounds) -> f32 {
        let d = (bounds.center - pos).length() + bounds.radius;
        d * d
    }
}

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
                        material_response: [1.0, 0.0, 1.0, 1.0],
                        texture_flags2: [0.0, 0.0, 0.0, 0.0],
                    },
                    material: object.material,
                    animation_world_id: object.animation_world_id,
                    animation_target_id: object.animation_target_id,
                });
            }
        }
    }

    #[allow(dead_code)] // owner: voplay/render-world; expiry: 2026-07-12; public scene owner API kept for non-renderer callers.
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

    pub fn collect_scene_primitive_draws_with_chunk_info(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
        chunk_info: &mut Vec<PrimitiveChunkBatchInfo>,
    ) {
        self.primitive_scenes
            .collect_draws_with_chunk_info(scene_id, camera, draws, chunks, chunk_info);
    }

    pub fn collect_scene_primitive_shadow_objects(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
    ) {
        self.primitive_scenes
            .collect_shadow_objects(scene_id, camera, draws);
    }

    pub fn collect_scene_primitive_shadow_chunks_from_candidates(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        candidate_chunks: &[PrimitiveChunkRef],
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.primitive_scenes.collect_shadow_chunks_from_candidates(
            scene_id,
            camera,
            candidate_chunks,
            chunks,
        );
    }

    pub fn collect_scene_primitive_shadow_objects_for_light_view(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: &Camera3DUniform,
        draws: &mut Vec<PrimitiveDraw>,
    ) {
        self.primitive_scenes.collect_shadow_objects_for_light_view(
            scene_id,
            camera,
            light_camera,
            draws,
        );
    }

    pub fn collect_scene_primitive_shadow_chunks_for_light_view(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        light_camera: &Camera3DUniform,
        candidate_chunks: &[PrimitiveChunkRef],
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.primitive_scenes.collect_shadow_chunks_for_light_view(
            scene_id,
            camera,
            light_camera,
            candidate_chunks,
            chunks,
        );
    }

    pub fn collect_scene_primitive_depth_draws(
        &self,
        scene_id: u32,
        camera: Option<&Camera3DUniform>,
        draws: &mut Vec<PrimitiveDraw>,
        chunks: &mut Vec<PrimitiveChunkRef>,
    ) {
        self.primitive_scenes
            .collect_depth_draws(scene_id, camera, draws, chunks);
    }

    pub fn build_batch_plan(
        &self,
        scene_id: u32,
        frame_id: u32,
        camera: Option<&Camera3DUniform>,
    ) -> RenderBatchPlan {
        let mut model_draws = Vec::new();
        let mut primitive_draws = Vec::new();
        let mut primitive_chunks = Vec::new();
        let mut primitive_chunk_info = Vec::new();
        self.collect_scene_draws(scene_id, &mut model_draws);
        self.collect_scene_primitive_draws_with_chunk_info(
            scene_id,
            None,
            &mut primitive_draws,
            &mut primitive_chunks,
            &mut primitive_chunk_info,
        );
        RenderBatchPlanner::build(
            frame_id,
            scene_id,
            &model_draws,
            &[],
            &primitive_draws,
            &primitive_chunks,
            &primitive_chunk_info,
            &[],
            camera,
            RenderBatchQualityProfile::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive_scene::PRIMITIVE_FLAG_WATER_SURFACE;

    fn primitive_update(
        scene_id: u32,
        layer_id: u32,
        object_id: u32,
        model_id: u32,
        flags: u32,
    ) -> PrimitiveObjectUpdate {
        PrimitiveObjectUpdate {
            scene_id,
            layer_id,
            object_id,
            model_id,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride::default(),
            visible: true,
            flags,
            lod_near: 0.0,
            lod_far: 1000.0,
            wind_strength: 0.0,
            atlas_uv: [0.0, 0.0, 1.0, 1.0],
        }
    }

    fn primitive_chunk_info(
        chunk: PrimitiveChunkRef,
        min: Vec3,
        max: Vec3,
        near: f32,
        far: f32,
    ) -> PrimitiveChunkBatchInfo {
        PrimitiveChunkBatchInfo {
            chunk,
            bounds_min: min,
            bounds_max: max,
            min_lod_near: near,
            max_lod_far: far,
            all_have_far_lod: far > 0.0,
        }
    }

    fn model_draw(model_id: u32, position: Vec3, material_id: u32) -> ModelDraw {
        ModelDraw {
            model_id,
            model_uniform: ModelUniform {
                model: math3d::model_matrix(position, Quat::IDENTITY, Vec3::ONE),
                normal_matrix: math3d::MAT4_IDENTITY,
                base_color: [1.0, 1.0, 1.0, 1.0],
                material_params: [1.0, 1.0, 1.0, 1.0],
                emissive_color: [0.0, 0.0, 0.0, 0.0],
                texture_flags: [0.0, 0.0, 0.0, 0.0],
                material_response: [1.0, 0.0, 1.0, 1.0],
                texture_flags2: [0.0, 0.0, 0.0, 0.0],
            },
            material: MaterialOverride {
                id: material_id,
                ..Default::default()
            },
            animation_world_id: 0,
            animation_target_id: 0,
        }
    }

    #[test]
    fn render_world_builds_unified_batch_plan() {
        let mut world = RenderWorld::new();
        world.upsert_object(RenderObjectUpdate {
            scene_id: 7,
            object_id: 11,
            model_id: 42,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            material: MaterialOverride {
                id: 5,
                ..Default::default()
            },
            visible: true,
            animation_world_id: 0,
            animation_target_id: 0,
        });
        let mut model_draws = Vec::new();
        world.collect_scene_draws(7, &mut model_draws);
        let primitive_draws = vec![
            PrimitiveDraw::from_update(primitive_update(7, 3, 100, 77, 0)),
            PrimitiveDraw::from_update(primitive_update(
                7,
                3,
                101,
                78,
                PRIMITIVE_FLAG_WATER_SURFACE,
            )),
        ];
        let primitive_chunks = vec![PrimitiveChunkRef {
            scene_id: 7,
            layer_id: 3,
            chunk_id: 9,
        }];
        let primitive_chunk_info = vec![primitive_chunk_info(
            primitive_chunks[0],
            Vec3::new(-2.0, -0.5, -2.0),
            Vec3::new(2.0, 0.5, 2.0),
            0.0,
            0.0,
        )];

        let plan = RenderBatchPlanner::build(
            120,
            7,
            &model_draws,
            &[],
            &primitive_draws,
            &primitive_chunks,
            &primitive_chunk_info,
            &[],
            None,
            RenderBatchQualityProfile::default(),
        );

        assert_eq!(plan.frame_id, 120);
        assert_eq!(plan.visible_objects, 3);
        assert_eq!(plan.mesh_batches, 1);
        assert_eq!(plan.primitive_batches, 2);
        assert_eq!(plan.water_batches, 1);
        assert_eq!(plan.total_batches(), 4);
        assert!(plan
            .visible_chunks
            .iter()
            .any(|chunk| chunk.kind == RenderBatchKind::Mesh && chunk.material_group == 5));
        assert!(plan
            .visible_chunks
            .iter()
            .any(|chunk| chunk.kind == RenderBatchKind::Primitive && chunk.material_group == 3));
        assert!(plan
            .visible_chunks
            .iter()
            .any(|chunk| chunk.kind == RenderBatchKind::Water));
        assert!(plan
            .visible_chunks
            .iter()
            .all(|chunk| chunk.bounds.radius > 0.0));

        let world_plan = world.build_batch_plan(7, 121, None);
        assert_eq!(world_plan.frame_id, 121);
        assert_eq!(world_plan.mesh_batches, 1);
        assert_eq!(world_plan.visible_objects, 1);
    }

    #[test]
    fn batch_planner_constructs_terrain_and_decal_entries() {
        let terrain_draw = model_draw(42, Vec3::new(3.0, 0.0, 0.0), 9);
        let terrain_inputs = vec![RenderTerrainBatchInput {
            draw_index: 0,
            bounds: RenderChunkBounds {
                center: Vec3::new(3.0, 0.0, 0.0),
                radius: 4.0,
            },
            material_group: 77,
            dirty_start: 4,
            dirty_count: 2,
            resident_state: RenderWorldChunkResidentState::Dirty,
            last_upload_frame: 41,
        }];
        let decals = vec![PostDecalGpu::new(
            [2.0, 0.5, 1.0],
            0.25,
            3.0,
            5.0,
            1.0,
            [1.0, 0.8, 0.6, 0.75],
        )];
        let decal_inputs = RenderBatchPlanner::decal_inputs(41, &decals);

        let plan = RenderBatchPlanner::build(
            41,
            3,
            &[terrain_draw],
            &terrain_inputs,
            &[],
            &[],
            &[],
            &decal_inputs,
            None,
            RenderBatchQualityProfile::default(),
        );

        assert_eq!(plan.mesh_batches, 0);
        assert_eq!(plan.terrain_batches, 1);
        assert_eq!(plan.decal_batches, 1);
        assert_eq!(plan.total_batches(), 2);
        assert_eq!(plan.model_batch_indices, vec![0]);
        assert_eq!(plan.terrain_batch_indices, vec![0]);
        assert_eq!(plan.decal_batch_indices, vec![0]);
        assert_eq!(plan.terrain_batches(&[terrain_draw]).len(), 1);
        assert_eq!(plan.decal_batches(&decals).len(), 1);
        assert_eq!(plan.dirty_uploads, 2);
        assert!(plan
            .visible_chunks
            .iter()
            .any(|chunk| chunk.kind == RenderBatchKind::Terrain && chunk.material_group == 77));
        assert!(plan
            .visible_chunks
            .iter()
            .any(|chunk| chunk.kind == RenderBatchKind::Decal && chunk.dirty_count == 1));
    }

    #[test]
    fn batch_planner_selects_lod_from_distance_and_metadata() {
        let primitive_chunks = vec![PrimitiveChunkRef {
            scene_id: 1,
            layer_id: 2,
            chunk_id: 1,
        }];
        let primitive_chunk_info = vec![primitive_chunk_info(
            primitive_chunks[0],
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, 1.0),
            0.0,
            400.0,
        )];
        let primitive_draw = PrimitiveDraw::from_update(primitive_update(1, 2, 3, 4, 0));
        let primitive_draws = vec![primitive_draw];
        let camera = Camera3DUniform {
            view_proj: math3d::MAT4_IDENTITY,
            camera_pos: [300.0, 0.0, 0.0],
            _pad: 0.0,
        };

        let plan = RenderBatchPlanner::build(
            9,
            1,
            &[],
            &[],
            &primitive_draws,
            &primitive_chunks,
            &primitive_chunk_info,
            &[],
            Some(&camera),
            RenderBatchQualityProfile::default(),
        );

        assert_eq!(plan.primitive_batches, 2);
        assert_eq!(plan.lod1_chunks, 2);
        assert_eq!(plan.lod0_chunks, 0);
    }

    #[test]
    fn batch_planner_counts_frustum_and_distance_culls() {
        let primitive_draws = vec![PrimitiveDraw::from_update(PrimitiveObjectUpdate {
            pos: Vec3::ZERO,
            lod_far: 5.0,
            ..primitive_update(2, 1, 1, 9, 0)
        })];
        let far_model = ModelDraw {
            model_id: 3,
            model_uniform: ModelUniform {
                model: math3d::model_matrix(Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE),
                normal_matrix: math3d::MAT4_IDENTITY,
                base_color: [1.0, 1.0, 1.0, 1.0],
                material_params: [1.0, 1.0, 1.0, 1.0],
                emissive_color: [0.0, 0.0, 0.0, 0.0],
                texture_flags: [0.0, 0.0, 0.0, 0.0],
                material_response: [1.0, 0.0, 1.0, 1.0],
                texture_flags2: [0.0, 0.0, 0.0, 0.0],
            },
            material: MaterialOverride::default(),
            animation_world_id: 0,
            animation_target_id: 0,
        };
        let camera = Camera3DUniform {
            view_proj: math3d::MAT4_IDENTITY,
            camera_pos: [100.0, 0.0, 0.0],
            _pad: 0.0,
        };

        let plan = RenderBatchPlanner::build(
            10,
            2,
            &[far_model],
            &[],
            &primitive_draws,
            &[],
            &[],
            &[],
            Some(&camera),
            RenderBatchQualityProfile::default(),
        );

        assert_eq!(plan.frustum_culled_chunks, 1);
        assert_eq!(plan.distance_culled_chunks, 1);
        assert_eq!(plan.visible_objects, 0);
    }
}
