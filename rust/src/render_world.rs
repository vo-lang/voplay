use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::model_loader::ModelManager;
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelDraw, ModelUniform};
use crate::pipeline_post::PostDecalGpu;
use crate::primitive_scene::{
    PrimitiveChunkBatchInfo, PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate,
    PrimitiveRenderWorld,
};

mod store;
pub use store::{RenderObjectUpdate, RenderWorld};

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
        collect_indexed_batches(&self.model_batch_indices, draws)
    }

    pub fn terrain_batches(&self, draws: &[ModelDraw]) -> Vec<ModelDraw> {
        collect_indexed_batches(&self.terrain_batch_indices, draws)
    }

    pub fn primitive_draw_batches(&self, draws: &[PrimitiveDraw]) -> Vec<PrimitiveDraw> {
        collect_indexed_batches(&self.primitive_draw_indices, draws)
    }

    pub fn primitive_chunk_batches(&self, chunks: &[PrimitiveChunkRef]) -> Vec<PrimitiveChunkRef> {
        collect_indexed_batches(&self.primitive_chunk_indices, chunks)
    }

    pub fn water_draw_batches(&self, draws: &[PrimitiveDraw]) -> Vec<PrimitiveDraw> {
        collect_indexed_batches(&self.water_draw_indices, draws)
    }

    pub fn water_chunk_batches(&self, chunks: &[PrimitiveChunkRef]) -> Vec<PrimitiveChunkRef> {
        collect_indexed_batches(&self.water_chunk_indices, chunks)
    }

    pub fn decal_batches(&self, decals: &[PostDecalGpu]) -> Vec<PostDecalGpu> {
        collect_indexed_batches(&self.decal_batch_indices, decals)
    }
}

fn collect_indexed_batches<T: Copy>(indices: &[usize], values: &[T]) -> Vec<T> {
    let mut batches = Vec::with_capacity(indices.len());
    for index in indices {
        if let Some(value) = values.get(*index).copied() {
            batches.push(value);
        }
    }
    batches
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
        let mut terrain = Vec::new();
        for (draw_index, draw) in model_draws.iter().enumerate() {
            let Some(model) = models.get(draw.model_id) else {
                continue;
            };
            let has_terrain = model
                .meshes
                .iter()
                .any(|mesh| !mesh.skinned && mesh.material.control_texture_id.is_some());
            if has_terrain {
                terrain.push(RenderTerrainBatchInput {
                    draw_index,
                    bounds: Self::model_bounds(draw, models),
                    material_group: draw.material.id,
                    dirty_start: 0,
                    dirty_count: 0,
                    resident_state: RenderWorldChunkResidentState::Resident,
                    last_upload_frame: frame_id,
                });
            }
        }
        terrain
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

#[cfg(test)]
mod tests;
