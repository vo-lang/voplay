use std::collections::HashMap;

use crate::math3d::{self, Quat, Vec3};
use crate::pipeline3d::{Camera3DUniform, MaterialOverride, ModelDraw, ModelUniform};
use crate::primitive_scene::{
    PrimitiveChunkRef, PrimitiveDraw, PrimitiveObjectUpdate, PrimitiveRenderWorld,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderChunkBounds {
    pub center: Vec3,
    pub radius: f32,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderBatchPlan {
    pub frame_id: u32,
    pub visible_objects: u32,
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
}

pub struct RenderBatchPlanner;

impl RenderBatchPlanner {
    pub fn build(
        frame_id: u32,
        scene_id: u32,
        model_draws: &[ModelDraw],
        primitive_draws: &[PrimitiveDraw],
        primitive_chunks: &[PrimitiveChunkRef],
    ) -> RenderBatchPlan {
        let mut plan = RenderBatchPlan {
            frame_id,
            visible_objects: (model_draws.len() + primitive_draws.len()) as u32,
            ..Default::default()
        };
        for (index, draw) in model_draws.iter().enumerate() {
            plan.push_chunk(RenderWorldChunk {
                scene_id,
                chunk_id: draw.model_id,
                kind: RenderBatchKind::Mesh,
                bounds: RenderChunkBounds {
                    center: Vec3::ZERO,
                    radius: 0.0,
                },
                lod_level: Self::select_lod(index as u32, 1),
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
            plan.push_chunk(RenderWorldChunk {
                scene_id: chunk.scene_id,
                chunk_id: chunk.chunk_id,
                kind: RenderBatchKind::Primitive,
                bounds: RenderChunkBounds {
                    center: Vec3::ZERO,
                    radius: 0.0,
                },
                lod_level: Self::select_lod(chunk.chunk_id, primitive_draws.len() as u32),
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
            if !draw.is_water_surface() {
                continue;
            }
            plan.push_chunk(RenderWorldChunk {
                scene_id,
                chunk_id: 0x8000_0000 | index as u32,
                kind: RenderBatchKind::Water,
                bounds: RenderChunkBounds {
                    center: Vec3::ZERO,
                    radius: 0.0,
                },
                lod_level: 0,
                material_group: draw.material.id,
                instance_start: index as u32,
                instance_count: 1,
                dirty_start: 0,
                dirty_count: 0,
                resident_state: RenderWorldChunkResidentState::Resident,
                last_upload_frame: frame_id,
            });
        }
        plan
    }

    fn select_lod(seed: u32, workload: u32) -> u8 {
        if workload > 4096 && seed % 3 != 0 {
            1
        } else {
            0
        }
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
        self.collect_scene_draws(scene_id, &mut model_draws);
        self.collect_scene_primitive_draws(
            scene_id,
            camera,
            &mut primitive_draws,
            &mut primitive_chunks,
        );
        RenderBatchPlanner::build(
            frame_id,
            scene_id,
            &model_draws,
            &primitive_draws,
            &primitive_chunks,
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
            PrimitiveDraw::from_update(primitive_update(7, 3, 101, 78, PRIMITIVE_FLAG_WATER_SURFACE)),
        ];
        let primitive_chunks = vec![PrimitiveChunkRef {
            scene_id: 7,
            layer_id: 3,
            chunk_id: 9,
        }];

        let plan = RenderBatchPlanner::build(120, 7, &model_draws, &primitive_draws, &primitive_chunks);

        assert_eq!(plan.frame_id, 120);
        assert_eq!(plan.visible_objects, 3);
        assert_eq!(plan.mesh_batches, 1);
        assert_eq!(plan.primitive_batches, 1);
        assert_eq!(plan.water_batches, 1);
        assert_eq!(plan.total_batches(), 3);
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

        let world_plan = world.build_batch_plan(7, 121, None);
        assert_eq!(world_plan.frame_id, 121);
        assert_eq!(world_plan.mesh_batches, 1);
        assert_eq!(world_plan.visible_objects, 1);
    }

    #[test]
    fn batch_planner_selects_lod_for_large_chunk_workloads() {
        let primitive_chunks = vec![
            PrimitiveChunkRef {
                scene_id: 1,
                layer_id: 2,
                chunk_id: 1,
            },
            PrimitiveChunkRef {
                scene_id: 1,
                layer_id: 2,
                chunk_id: 2,
            },
        ];
        let primitive_draw = PrimitiveDraw::from_update(primitive_update(1, 2, 3, 4, 0));
        let primitive_draws = vec![primitive_draw; 4097];

        let plan = RenderBatchPlanner::build(9, 1, &[], &primitive_draws, &primitive_chunks);

        assert_eq!(plan.primitive_batches, 2);
        assert_eq!(plan.lod1_chunks, 2);
        assert_eq!(plan.lod0_chunks, 0);
    }
}
