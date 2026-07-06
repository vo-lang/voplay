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

    let mut plan = RenderBatchPlanner::build(
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
